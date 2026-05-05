import {
  afterEach,
  beforeEach,
  describe,
  expect,
  it,
  vi,
} from "vitest";
import {
  combineReducers,
  configureStore,
  type Action,
  type Middleware,
} from "@reduxjs/toolkit";

vi.mock("../../api/clients", () => ({
  ingestionClient: {
    getJobProgress: vi.fn(),
  },
}));

import { ingestionClient } from "../../api/clients";
import ingestionReducer, {
  appleJobStarted,
  appleJobReset,
  type AppleSourceKey,
} from "../ingestionSlice";
import { appleJobsListener } from "../appleJobsMiddleware";

const mockedGetJobProgress = vi.mocked(ingestionClient.getJobProgress);

type ProgressShape = {
  progress_percentage?: number;
  status_message?: string;
  message?: string;
  is_complete?: boolean;
  is_failed?: boolean;
  error_message?: string;
  results?: { total?: number; ingested?: number };
  result?: { total?: number; ingested?: number };
};

type GetJobProgressResponse = Awaited<
  ReturnType<typeof ingestionClient.getJobProgress>
>;

const ok = (data: ProgressShape): GetJobProgressResponse =>
  ({
    status: 200,
    success: true,
    data,
  }) as unknown as GetJobProgressResponse;

const transientFailure = (): GetJobProgressResponse =>
  ({
    status: 503,
    success: false,
    error: "transient",
  }) as unknown as GetJobProgressResponse;

function buildStore() {
  return configureStore({
    reducer: combineReducers({ ingestion: ingestionReducer }),
    middleware: (getDefault) =>
      getDefault({ serializableCheck: false }).concat(
        appleJobsListener.middleware,
      ),
  });
}

const ADVANCE = async (ms: number) => {
  await vi.advanceTimersByTimeAsync(ms);
};

const getJob = (
  store: ReturnType<typeof buildStore>,
  key: AppleSourceKey,
) => store.getState().ingestion.appleJobs[key];

describe("appleJobsMiddleware", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("dispatches appleJobProgressed after a 2s tick when the job is still running", async () => {
    mockedGetJobProgress.mockResolvedValue(
      ok({
        progress_percentage: 42,
        status_message: "halfway",
        is_complete: false,
        is_failed: false,
      }),
    );

    const store = buildStore();
    store.dispatch(appleJobStarted({ key: "notes", progressId: "job-1" }));

    expect(mockedGetJobProgress).not.toHaveBeenCalled();

    await ADVANCE(2000);

    expect(mockedGetJobProgress).toHaveBeenCalledWith("job-1");
    expect(mockedGetJobProgress).toHaveBeenCalledTimes(1);

    const job = getJob(store, "notes");
    expect(job.status).toBe("running");
    expect(job.progress).toBe(42);
    expect(job.message).toBe("halfway");
  });

  it("dispatches appleJobCompleted when is_complete is true and stops polling", async () => {
    mockedGetJobProgress
      .mockResolvedValueOnce(
        ok({ progress_percentage: 50, status_message: "half" }),
      )
      .mockResolvedValueOnce(
        ok({
          progress_percentage: 100,
          status_message: "done",
          is_complete: true,
          results: { total: 12, ingested: 11 },
        }),
      )
      .mockResolvedValue(
        ok({ progress_percentage: 100, is_complete: true }),
      );

    const store = buildStore();
    store.dispatch(appleJobStarted({ key: "photos", progressId: "p-1" }));

    await ADVANCE(2000); // first tick → progressed
    await ADVANCE(2000); // second tick → complete

    const job = getJob(store, "photos");
    expect(job.status).toBe("done");
    expect(job.result).toEqual({ total: 12, ingested: 11 });
    expect(job.message).toBe("done");

    const callsAfterComplete = mockedGetJobProgress.mock.calls.length;

    await ADVANCE(4000); // drift past two more polling windows

    expect(mockedGetJobProgress.mock.calls.length).toBe(callsAfterComplete);
  });

  it("dispatches appleJobFailed when is_failed is true and stops polling", async () => {
    mockedGetJobProgress.mockResolvedValueOnce(
      ok({
        is_failed: true,
        error_message: "kaboom",
      }),
    );
    mockedGetJobProgress.mockResolvedValue(
      ok({ is_failed: true, error_message: "kaboom" }),
    );

    const store = buildStore();
    store.dispatch(appleJobStarted({ key: "calendar", progressId: "c-1" }));

    await ADVANCE(2000);

    const job = getJob(store, "calendar");
    expect(job.status).toBe("error");
    expect(job.message).toBe("kaboom");

    const callsAfterFail = mockedGetJobProgress.mock.calls.length;
    await ADVANCE(4000);
    expect(mockedGetJobProgress.mock.calls.length).toBe(callsAfterFail);
  });

  it("dispatches appleJobFailed (not Completed) when both is_failed and is_complete are true", async () => {
    // Backend stamps is_complete = "no longer running" and is_failed =
    // "ended in error" when a job fails terminally (e.g. osascript
    // timeout for contacts permission). Failure must win — otherwise
    // the SourceCard renders a green ✓ for a job that errored out.
    mockedGetJobProgress.mockResolvedValueOnce(
      ok({
        is_complete: true,
        is_failed: true,
        status_message:
          "Failed to extract contacts: osascript timed out after 300 seconds",
        error_message: "osascript timeout",
      }),
    );
    mockedGetJobProgress.mockResolvedValue(
      ok({ is_complete: true, is_failed: true, error_message: "osascript timeout" }),
    );

    const store = buildStore();
    store.dispatch(appleJobStarted({ key: "contacts", progressId: "ct-1" }));

    await ADVANCE(2000);

    const job = getJob(store, "contacts");
    expect(job.status).toBe("error");
    expect(job.message).toBe("osascript timeout");

    // Polling stops on terminal state — same contract as the success path.
    const callsAfterFail = mockedGetJobProgress.mock.calls.length;
    await ADVANCE(4000);
    expect(mockedGetJobProgress.mock.calls.length).toBe(callsAfterFail);
  });

  it("stops polling when appleJobReset is dispatched mid-flight", async () => {
    mockedGetJobProgress.mockResolvedValue(
      ok({
        progress_percentage: 30,
        status_message: "working",
        is_complete: false,
      }),
    );

    const store = buildStore();
    store.dispatch(appleJobStarted({ key: "reminders", progressId: "r-1" }));

    await ADVANCE(2000); // one tick has happened
    expect(mockedGetJobProgress).toHaveBeenCalledTimes(1);

    store.dispatch(appleJobReset({ key: "reminders" }));

    const callsAfterReset = mockedGetJobProgress.mock.calls.length;
    await ADVANCE(6000);
    expect(mockedGetJobProgress.mock.calls.length).toBe(callsAfterReset);

    const job = getJob(store, "reminders");
    expect(job.status).toBe("idle");
    expect(job.progressId).toBeNull();
  });

  it("keeps a single poll loop when appleJobStarted fires twice for the same key", async () => {
    mockedGetJobProgress.mockResolvedValue(
      ok({
        progress_percentage: 10,
        status_message: "working",
        is_complete: false,
      }),
    );

    const store = buildStore();
    store.dispatch(appleJobStarted({ key: "contacts", progressId: "x-1" }));
    store.dispatch(appleJobStarted({ key: "contacts", progressId: "x-2" }));

    await ADVANCE(2000);
    await ADVANCE(2000);

    // With two ticks under a single live loop we expect ≤ 2 calls — never
    // 4 (which would mean both forks survived). All calls must reference
    // the most recent progressId; the prior fork's progressId is stale.
    expect(mockedGetJobProgress.mock.calls.length).toBeLessThanOrEqual(2);
    for (const [arg] of mockedGetJobProgress.mock.calls) {
      expect(arg).toBe("x-2");
    }
  });

  it("skips redundant appleJobProgressed dispatches when progress and message are unchanged", async () => {
    mockedGetJobProgress.mockResolvedValue(
      ok({
        progress_percentage: 30,
        status_message: "still working",
        is_complete: false,
      }),
    );

    const store = buildStore();
    const observed: string[] = [];
    store.subscribe(() => {
      observed.push(getJob(store, "notes").message);
    });
    store.dispatch(appleJobStarted({ key: "notes", progressId: "job-dedup" }));

    // First tick: dispatches the new (30, "still working").
    await ADVANCE(2000);
    expect(getJob(store, "notes").progress).toBe(30);
    const subscriberFiresAfterFirstTick = observed.length;

    // Subsequent ticks return identical progress/message — nothing new
    // for the UI to render. The middleware must NOT re-dispatch
    // appleJobProgressed and so the store subscriber must not fire
    // (state identity is unchanged).
    await ADVANCE(2000);
    await ADVANCE(2000);
    await ADVANCE(2000);

    expect(getJob(store, "notes").progress).toBe(30);
    expect(observed.length).toBe(subscriberFiresAfterFirstTick);

    // Backend finally produces a real change → dispatch must resume.
    mockedGetJobProgress.mockResolvedValue(
      ok({
        progress_percentage: 60,
        status_message: "more done",
        is_complete: false,
      }),
    );
    await ADVANCE(2000);
    expect(getJob(store, "notes").progress).toBe(60);
    expect(observed.length).toBeGreaterThan(subscriberFiresAfterFirstTick);
  });

  // Post-completion contract for the dogfood-2026-05-05 follow-up:
  // once a job has settled into a terminal state (`done` / `error`),
  // nothing the listener middleware does on its own — including a
  // re-arming `appleJobStarted` for a *different* key — may flip the
  // settled job back to `idle`. The only legal route to `idle` is the
  // explicit `appleJobReset` dispatch from the Reset All button.
  //
  // Concretely, the existing `appleJobCompleted` / `appleJobFailed`
  // tests prove the slot reaches the right terminal state. This test
  // proves the middleware never tries to clear that state behind the
  // user's back, regardless of polling activity on neighbouring keys.
  it("never auto-dispatches appleJobReset; settled jobs stay settled while other keys poll", async () => {
    const recorded: Action[] = [];
    const recorder: Middleware = () => (next) => (action) => {
      recorded.push(action as Action);
      return next(action);
    };

    const store = configureStore({
      reducer: combineReducers({ ingestion: ingestionReducer }),
      middleware: (getDefault) =>
        getDefault({ serializableCheck: false })
          .concat(appleJobsListener.middleware)
          .concat(recorder),
    });

    // Photos completes terminally on the first tick.
    mockedGetJobProgress.mockImplementation(async (id: string) => {
      if (id === "photos-1") {
        return ok({
          progress_percentage: 100,
          status_message: "✓ Imported 50 photos",
          is_complete: true,
          results: { total: 50, ingested: 47 },
        });
      }
      // Notes keeps polling indefinitely so the listener stays active.
      return ok({
        progress_percentage: 25,
        status_message: "working",
        is_complete: false,
      });
    });

    store.dispatch(appleJobStarted({ key: "photos", progressId: "photos-1" }));
    store.dispatch(appleJobStarted({ key: "notes", progressId: "notes-1" }));

    await ADVANCE(2000); // photos completes; notes ticks.
    await ADVANCE(2000); // notes ticks again.
    await ADVANCE(2000); // and again.

    const photos = getJob(store, "photos");
    expect(photos.status).toBe("done");
    expect(photos.result).toEqual({ total: 50, ingested: 47 });

    const notes = getJob(store, "notes");
    expect(notes.status).toBe("running");

    // The listener may dispatch Started / Progressed / Completed /
    // Failed. It MUST NOT dispatch Reset on its own — that's reserved
    // for explicit user intent (Reset All button).
    const resetActions = recorded.filter(
      (a) => a.type === appleJobReset.type,
    );
    expect(resetActions).toHaveLength(0);
  });

  it("treats success:false as transient and keeps polling without dispatching failure", async () => {
    mockedGetJobProgress
      .mockResolvedValueOnce(transientFailure())
      .mockResolvedValueOnce(
        ok({
          progress_percentage: 60,
          status_message: "recovered",
          is_complete: false,
        }),
      )
      .mockResolvedValue(
        ok({
          progress_percentage: 60,
          status_message: "recovered",
          is_complete: false,
        }),
      );

    const store = buildStore();
    store.dispatch(appleJobStarted({ key: "notes", progressId: "job-trans" }));

    await ADVANCE(2000);
    let job = getJob(store, "notes");
    expect(job.status).toBe("running");
    // After a transient response we should NOT have updated progress.
    expect(job.progress).toBe(5);
    expect(job.message).toBe("Starting...");

    await ADVANCE(2000);
    job = getJob(store, "notes");
    expect(job.status).toBe("running");
    expect(job.progress).toBe(60);
    expect(job.message).toBe("recovered");
    expect(mockedGetJobProgress).toHaveBeenCalledTimes(2);
  });
});
