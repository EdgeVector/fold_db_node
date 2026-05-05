import {
  createListenerMiddleware,
  type Action,
  type ForkedTask,
} from "@reduxjs/toolkit";
import { ingestionClient } from "../api/clients";
import {
  appleJobCompleted,
  appleJobFailed,
  appleJobProgressed,
  appleJobReset,
  appleJobStarted,
  type AppleSourceKey,
  type ImportResult,
} from "./ingestionSlice";
import type { AppDispatch, RootState } from "./store";

const POLL_INTERVAL_MS = 2000;

interface JobProgressShape {
  progress_percentage?: number;
  status_message?: string;
  message?: string;
  is_complete?: boolean;
  is_failed?: boolean;
  error_message?: string;
  results?: ImportResult;
  result?: ImportResult;
}

export const appleJobsListener = createListenerMiddleware();

const startAppleListening = appleJobsListener.startListening.withTypes<
  RootState,
  AppDispatch
>();

// Module-scope per-source registry. Polling lifecycle lives here so it
// survives component unmounts; nothing in the React tree owns a timer.
const activeForks = new Map<AppleSourceKey, ForkedTask<void>>();

const isCancelActionForKey = (action: Action, key: AppleSourceKey): boolean => {
  if (
    !appleJobReset.match(action) &&
    !appleJobCompleted.match(action) &&
    !appleJobFailed.match(action)
  ) {
    return false;
  }
  return action.payload.key === key;
};

startAppleListening({
  actionCreator: appleJobStarted,
  effect: async (action, listenerApi) => {
    const { key, progressId } = action.payload;

    const prior = activeForks.get(key);
    if (prior) {
      prior.cancel();
      activeForks.delete(key);
    }

    const task = listenerApi.fork(async (forkApi) => {
      while (true) {
        await forkApi.delay(POLL_INTERVAL_MS);
        const resp = await ingestionClient.getJobProgress(progressId);
        if (!resp.success || !resp.data) {
          continue;
        }
        const job = resp.data as JobProgressShape;
        const progress = job.progress_percentage ?? 0;
        const message = job.status_message ?? job.message ?? "";

        // Check is_failed BEFORE is_complete: when a job terminates in
        // failure the backend stamps both flags (is_complete = "no longer
        // running", is_failed = "ended in error"). Reading is_complete
        // first would render the failed job as a green ✓.
        if (job.is_failed) {
          listenerApi.dispatch(
            appleJobFailed({
              key,
              message: job.error_message ?? job.message ?? "Import failed",
            }),
          );
          return;
        }
        if (job.is_complete) {
          listenerApi.dispatch(
            appleJobCompleted({
              key,
              result: job.results ?? job.result ?? null,
              message,
            }),
          );
          return;
        }
        listenerApi.dispatch(appleJobProgressed({ key, progress, message }));
      }
    });

    activeForks.set(key, task);

    try {
      await Promise.race([
        task.result,
        listenerApi.condition((act): boolean =>
          isCancelActionForKey(act, key),
        ),
      ]);
    } finally {
      task.cancel();
      if (activeForks.get(key) === task) {
        activeForks.delete(key);
      }
    }
  },
});
