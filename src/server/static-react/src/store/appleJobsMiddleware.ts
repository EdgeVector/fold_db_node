import {
  createListenerMiddleware,
  type Action,
  type ForkedTaskAPI,
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

// Last dispatched (progress, message) per key. Skipping a no-op
// `appleJobProgressed` here avoids waking every selector subscribed to
// the ingestion slice — the bulk of the SPA freeze during a 5-source
// import was 5 redundant React commits per 2s window once the backend
// settled into its slow phases (HEIC convert, schema embedding) where
// progress and message rarely change between ticks.
const lastDispatched = new Map<
  AppleSourceKey,
  { progress: number; message: string }
>();

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

// Wait until the document becomes visible. Used to gate the next poll
// while the user is on another tab so we don't fire `getJobProgress`
// every 2s into the void. Resolves immediately if not in a browser, if
// already visible, or if the fork's signal aborts.
const waitUntilVisible = (forkApi: ForkedTaskAPI): Promise<void> => {
  if (typeof document === "undefined" || !document.hidden) {
    return Promise.resolve();
  }
  return new Promise<void>((resolve) => {
    const onVisibility = () => {
      if (!document.hidden) {
        cleanup();
        resolve();
      }
    };
    const onAbort = () => {
      cleanup();
      resolve();
    };
    const cleanup = () => {
      document.removeEventListener("visibilitychange", onVisibility);
      forkApi.signal.removeEventListener("abort", onAbort);
    };
    document.addEventListener("visibilitychange", onVisibility);
    forkApi.signal.addEventListener("abort", onAbort);
  });
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
    lastDispatched.delete(key);

    const task = listenerApi.fork(async (forkApi) => {
      while (true) {
        await forkApi.delay(POLL_INTERVAL_MS);
        await waitUntilVisible(forkApi);
        if (forkApi.signal.aborted) return;
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
        const last = lastDispatched.get(key);
        if (last && last.progress === progress && last.message === message) {
          continue;
        }
        lastDispatched.set(key, { progress, message });
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
      lastDispatched.delete(key);
    }
  },
});
