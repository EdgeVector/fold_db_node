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
    getAppleSyncConfig: vi.fn(),
    updateAppleSyncConfig: vi.fn(),
  },
}));

import { ingestionClient } from "../../api/clients";
import ingestionReducer, {
  appleJobStarting,
  appleJobStarted,
  appleJobProgressed,
  appleJobCompleted,
  appleJobFailed,
  appleJobReset,
  appleSyncConfigOptimisticPatch,
  debouncedPatchAppleSyncConfig,
  fetchAppleSyncConfig,
  flushAppleSyncDebouncedPatch,
  makeIdleAppleJob,
  reconcileAppleJobs,
  selectAppleEnabledSources,
  selectApplePhotosLimit,
  selectAppleJob,
  selectAppleSyncConfig,
  selectAppleSyncConfigLoaded,
  type AppleSourceKey,
} from "../ingestionSlice";
import type { AppleSyncConfig } from "../../api/clients/ingestionClient";
import type { RootState } from "../store";

const mockedGetJobProgress = vi.mocked(ingestionClient.getJobProgress);
const mockedGetAppleSyncConfig = vi.mocked(ingestionClient.getAppleSyncConfig);
const mockedUpdateAppleSyncConfig = vi.mocked(
  ingestionClient.updateAppleSyncConfig,
);

type SyncConfigResponse = Awaited<
  ReturnType<typeof ingestionClient.getAppleSyncConfig>
>;
const okSyncConfig = (data: AppleSyncConfig): SyncConfigResponse =>
  ({ status: 200, success: true, data }) as unknown as SyncConfigResponse;

const baseSyncConfig: AppleSyncConfig = {
  enabled: false,
  schedule: "daily",
  sources: { notes: true, photos: true, calendar: true, reminders: true, contacts: true },
  photos_limit: 50,
  last_sync: null,
  next_sync: null,
  last_error: null,
  last_error_at: null,
};

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

const APPLE_KEYS: AppleSourceKey[] = [
  "notes",
  "photos",
  "calendar",
  "reminders",
  "contacts",
];

function buildStore() {
  return configureStore({
    reducer: combineReducers({ ingestion: ingestionReducer }),
  });
}

// Custom-typed dispatch for tests that send our ThunkAction-typed thunks
// (debouncedPatchAppleSyncConfig, flushAppleSyncDebouncedPatch). The
// production AppDispatch carries thunk overloads; the test store's dispatch
// type loses them via `combineReducers`, so we cast through AppDispatch.
const thunkDispatch = (store: ReturnType<typeof buildStore>) =>
  store.dispatch as unknown as import("../store").AppDispatch;

describe("ingestionSlice — appleJobs", () => {
  describe("initial state", () => {
    it("initializes all five Apple sources to an idle job", () => {
      const store = buildStore();
      const { appleJobs } = store.getState().ingestion;

      expect(Object.keys(appleJobs).sort()).toEqual([...APPLE_KEYS].sort());
      for (const key of APPLE_KEYS) {
        expect(appleJobs[key]).toEqual(makeIdleAppleJob());
      }
    });
  });

  describe("appleJobStarting", () => {
    it("flips an idle job to running with no progressId yet", () => {
      const store = buildStore();
      store.dispatch(appleJobStarting({ key: "notes" }));

      const job = store.getState().ingestion.appleJobs.notes;
      expect(job).toEqual({
        progressId: null,
        status: "running",
        progress: 5,
        message: "Starting...",
        result: null,
      });
    });

    it("clobbers a prior `done` run so a re-import never shows stale message/result", () => {
      const store = buildStore();
      store.dispatch(appleJobStarted({ key: "photos", progressId: "p1" }));
      store.dispatch(
        appleJobCompleted({
          key: "photos",
          result: { total: 50, ingested: 50 },
          message: "Imported 50 photos",
        }),
      );
      store.dispatch(appleJobStarting({ key: "photos" }));

      const job = store.getState().ingestion.appleJobs.photos;
      expect(job.status).toBe("running");
      expect(job.progressId).toBeNull();
      expect(job.message).toBe("Starting...");
      expect(job.progress).toBe(5);
      expect(job.result).toBeNull();
    });

    it("clobbers a prior `error` run", () => {
      const store = buildStore();
      store.dispatch(appleJobStarted({ key: "contacts", progressId: "c1" }));
      store.dispatch(appleJobFailed({ key: "contacts", message: "boom" }));
      store.dispatch(appleJobStarting({ key: "contacts" }));

      const job = store.getState().ingestion.appleJobs.contacts;
      expect(job.status).toBe("running");
      expect(job.progressId).toBeNull();
      expect(job.message).toBe("Starting...");
    });

    it("only mutates the targeted source", () => {
      const store = buildStore();
      store.dispatch(appleJobStarting({ key: "calendar" }));

      const { appleJobs } = store.getState().ingestion;
      expect(appleJobs.calendar.status).toBe("running");
      for (const other of APPLE_KEYS.filter((k) => k !== "calendar")) {
        expect(appleJobs[other]).toEqual(makeIdleAppleJob());
      }
    });
  });

  describe("appleJobStarted", () => {
    it("transitions an idle job to running with progress 5 and the given progressId", () => {
      const store = buildStore();
      store.dispatch(
        appleJobStarted({ key: "notes", progressId: "job-abc" }),
      );

      const job = store.getState().ingestion.appleJobs.notes;
      expect(job).toEqual({
        progressId: "job-abc",
        status: "running",
        progress: 5,
        message: "Starting...",
        result: null,
      });
    });

    it("clears any prior result/message when restarting a finished job", () => {
      const store = buildStore();
      store.dispatch(appleJobStarted({ key: "photos", progressId: "p1" }));
      store.dispatch(
        appleJobCompleted({
          key: "photos",
          result: { total: 10, ingested: 9 },
          message: "Done",
        }),
      );
      store.dispatch(appleJobStarted({ key: "photos", progressId: "p2" }));

      const job = store.getState().ingestion.appleJobs.photos;
      expect(job.status).toBe("running");
      expect(job.result).toBeNull();
      expect(job.message).toBe("Starting...");
      expect(job.progressId).toBe("p2");
    });

    it("only mutates the targeted source", () => {
      const store = buildStore();
      store.dispatch(
        appleJobStarted({ key: "calendar", progressId: "cal-1" }),
      );

      const { appleJobs } = store.getState().ingestion;
      expect(appleJobs.calendar.status).toBe("running");
      for (const other of APPLE_KEYS.filter((k) => k !== "calendar")) {
        expect(appleJobs[other]).toEqual(makeIdleAppleJob());
      }
    });
  });

  describe("appleJobProgressed", () => {
    it("updates progress and message while running", () => {
      const store = buildStore();
      store.dispatch(appleJobStarted({ key: "notes", progressId: "n1" }));
      store.dispatch(
        appleJobProgressed({
          key: "notes",
          progress: 42,
          message: "Halfway",
        }),
      );

      const job = store.getState().ingestion.appleJobs.notes;
      expect(job.progress).toBe(42);
      expect(job.message).toBe("Halfway");
      expect(job.status).toBe("running");
    });

    it("is ignored when the job is not running (idle)", () => {
      const store = buildStore();
      store.dispatch(
        appleJobProgressed({
          key: "notes",
          progress: 50,
          message: "Should be ignored",
        }),
      );

      expect(store.getState().ingestion.appleJobs.notes).toEqual(
        makeIdleAppleJob(),
      );
    });

    it("is ignored when the job is done", () => {
      const store = buildStore();
      store.dispatch(appleJobStarted({ key: "notes", progressId: "n1" }));
      store.dispatch(
        appleJobProgressed({
          key: "notes",
          progress: 80,
          message: "Almost",
        }),
      );
      store.dispatch(
        appleJobCompleted({
          key: "notes",
          result: { total: 1, ingested: 1 },
          message: "Done",
        }),
      );
      store.dispatch(
        appleJobProgressed({
          key: "notes",
          progress: 99,
          message: "ignored",
        }),
      );

      const job = store.getState().ingestion.appleJobs.notes;
      expect(job.status).toBe("done");
      // Progress is the last value seen *before* completion — appleJobProgressed
      // is a no-op once the job is done, so the late-arriving 99% poll can't
      // overwrite the visible-to-user terminal state.
      expect(job.progress).toBe(80);
      expect(job.message).toBe("Done");
    });

    it("is ignored when the job is in error state", () => {
      const store = buildStore();
      store.dispatch(appleJobStarted({ key: "notes", progressId: "n1" }));
      store.dispatch(
        appleJobFailed({ key: "notes", message: "boom" }),
      );
      store.dispatch(
        appleJobProgressed({ key: "notes", progress: 70, message: "ignored" }),
      );

      const job = store.getState().ingestion.appleJobs.notes;
      expect(job.status).toBe("error");
      expect(job.message).toBe("boom");
    });
  });

  describe("appleJobCompleted", () => {
    it("transitions to done and stores result + message", () => {
      const store = buildStore();
      store.dispatch(appleJobStarted({ key: "photos", progressId: "p1" }));
      store.dispatch(
        appleJobCompleted({
          key: "photos",
          result: { total: 100, ingested: 87 },
          message: "Imported",
        }),
      );

      const job = store.getState().ingestion.appleJobs.photos;
      expect(job.status).toBe("done");
      expect(job.result).toEqual({ total: 100, ingested: 87 });
      expect(job.message).toBe("Imported");
    });

    it("accepts a null result", () => {
      const store = buildStore();
      store.dispatch(appleJobStarted({ key: "reminders", progressId: "r1" }));
      store.dispatch(
        appleJobCompleted({ key: "reminders", result: null, message: "Done" }),
      );

      const job = store.getState().ingestion.appleJobs.reminders;
      expect(job.status).toBe("done");
      expect(job.result).toBeNull();
    });
  });

  describe("appleJobFailed", () => {
    it("transitions to error and records the message", () => {
      const store = buildStore();
      store.dispatch(appleJobStarted({ key: "contacts", progressId: "c1" }));
      store.dispatch(
        appleJobFailed({ key: "contacts", message: "permission denied" }),
      );

      const job = store.getState().ingestion.appleJobs.contacts;
      expect(job.status).toBe("error");
      expect(job.message).toBe("permission denied");
    });
  });

  describe("appleJobReset", () => {
    it("resets a finished job back to idle", () => {
      const store = buildStore();
      store.dispatch(appleJobStarted({ key: "notes", progressId: "n1" }));
      store.dispatch(
        appleJobCompleted({
          key: "notes",
          result: { total: 5, ingested: 5 },
          message: "Done",
        }),
      );
      store.dispatch(appleJobReset({ key: "notes" }));

      expect(store.getState().ingestion.appleJobs.notes).toEqual(
        makeIdleAppleJob(),
      );
    });

    it("resets a failed job back to idle", () => {
      const store = buildStore();
      store.dispatch(appleJobStarted({ key: "notes", progressId: "n1" }));
      store.dispatch(appleJobFailed({ key: "notes", message: "fail" }));
      store.dispatch(appleJobReset({ key: "notes" }));

      expect(store.getState().ingestion.appleJobs.notes).toEqual(
        makeIdleAppleJob(),
      );
    });

    it("does not affect other sources", () => {
      const store = buildStore();
      store.dispatch(appleJobStarted({ key: "notes", progressId: "n1" }));
      store.dispatch(appleJobStarted({ key: "photos", progressId: "p1" }));
      store.dispatch(appleJobReset({ key: "notes" }));

      const { appleJobs } = store.getState().ingestion;
      expect(appleJobs.notes).toEqual(makeIdleAppleJob());
      expect(appleJobs.photos.status).toBe("running");
      expect(appleJobs.photos.progressId).toBe("p1");
    });
  });

  describe("selectAppleJob", () => {
    it("returns the per-source job slice", () => {
      const store = buildStore();
      store.dispatch(
        appleJobStarted({ key: "calendar", progressId: "cal-7" }),
      );

      const job = selectAppleJob("calendar")(
        store.getState() as never,
      );
      expect(job.progressId).toBe("cal-7");
      expect(job.status).toBe("running");
    });

    it("returns the idle default for an untouched source", () => {
      const store = buildStore();
      const job = selectAppleJob("contacts")(store.getState() as never);
      expect(job).toEqual(makeIdleAppleJob());
    });

    it("returns a fresh reference after reducer transitions (selector factory has no stale closure)", () => {
      const store = buildStore();
      const select = selectAppleJob("notes");

      const before = select(store.getState() as never);
      store.dispatch(appleJobStarted({ key: "notes", progressId: "n1" }));
      const after = select(store.getState() as never);

      expect(before.status).toBe("idle");
      expect(after.status).toBe("running");
      expect(after.progressId).toBe("n1");
    });
  });
});

describe("reconcileAppleJobs", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  // Action recorder middleware so we can assert on the exact action stream
  // dispatched during reconciliation (notably appleJobStarted re-dispatch
  // for still-running jobs — the listener middleware keys off that).
  function buildStoreWithRecorder() {
    const recorded: Action[] = [];
    const recorder: Middleware = () => (next) => (action) => {
      recorded.push(action as Action);
      return next(action);
    };
    const store = configureStore({
      reducer: combineReducers({ ingestion: ingestionReducer }),
      middleware: (getDefault) => getDefault().concat(recorder),
    });
    return { store, recorded };
  }

  it("transitions completed and failed jobs and leaves idle jobs untouched", async () => {
    const { store } = buildStoreWithRecorder();

    store.dispatch(appleJobStarted({ key: "notes", progressId: "notes-1" }));
    store.dispatch(appleJobStarted({ key: "photos", progressId: "photos-1" }));
    // calendar stays idle.

    mockedGetJobProgress.mockImplementation(async (id: string) => {
      if (id === "notes-1") {
        return ok({
          progress_percentage: 100,
          is_complete: true,
          status_message: "Imported",
          results: { total: 12, ingested: 12 },
        });
      }
      if (id === "photos-1") {
        return ok({
          is_failed: true,
          error_message: "permission denied",
        });
      }
      throw new Error(`unexpected progress id: ${id}`);
    });

    await store.dispatch(reconcileAppleJobs());

    const { appleJobs } = store.getState().ingestion;
    expect(appleJobs.notes.status).toBe("done");
    expect(appleJobs.notes.result).toEqual({ total: 12, ingested: 12 });
    expect(appleJobs.notes.message).toBe("Imported");

    expect(appleJobs.photos.status).toBe("error");
    expect(appleJobs.photos.message).toBe("permission denied");

    expect(appleJobs.calendar).toEqual(makeIdleAppleJob());

    expect(mockedGetJobProgress).toHaveBeenCalledTimes(2);
    const calledIds = mockedGetJobProgress.mock.calls.map((c) => c[0]);
    expect(calledIds.sort()).toEqual(["notes-1", "photos-1"]);
  });

  it("treats a dual-flag job (is_complete + is_failed) as failed", async () => {
    // Backend stamps both flags on a terminal failure. reconcile must
    // route through appleJobFailed — otherwise a job that errored while
    // the user was on another tab gets reconciled into `done` on app
    // mount and shows a green ✓.
    const { store } = buildStoreWithRecorder();

    store.dispatch(
      appleJobStarted({ key: "contacts", progressId: "ct-1" }),
    );

    mockedGetJobProgress.mockResolvedValueOnce(
      ok({
        is_complete: true,
        is_failed: true,
        status_message:
          "Failed to extract contacts: osascript timed out after 300 seconds",
        error_message: "osascript timeout",
      }),
    );

    await store.dispatch(reconcileAppleJobs());

    const job = store.getState().ingestion.appleJobs.contacts;
    expect(job.status).toBe("error");
    expect(job.message).toBe("osascript timeout");
  });

  it("re-dispatches appleJobStarted for a still-running job to re-arm the listener", async () => {
    const { store, recorded } = buildStoreWithRecorder();

    store.dispatch(
      appleJobStarted({ key: "reminders", progressId: "rem-1" }),
    );
    // Drop the seed action from the recorded log so we only assert on
    // actions emitted by the reconcile thunk itself.
    recorded.length = 0;

    mockedGetJobProgress.mockResolvedValueOnce(
      ok({
        progress_percentage: 42,
        status_message: "Halfway",
        is_complete: false,
        is_failed: false,
      }),
    );

    await store.dispatch(reconcileAppleJobs());

    const types = recorded.map((a) => a.type);
    expect(types).toContain(appleJobStarted.type);
    expect(types).toContain(appleJobProgressed.type);

    const startedActions = recorded.filter(
      (a): a is ReturnType<typeof appleJobStarted> =>
        a.type === appleJobStarted.type,
    );
    expect(startedActions).toHaveLength(1);
    expect(startedActions[0].payload).toEqual({
      key: "reminders",
      progressId: "rem-1",
    });

    const job = store.getState().ingestion.appleJobs.reminders;
    expect(job.status).toBe("running");
    expect(job.progress).toBe(42);
    expect(job.message).toBe("Halfway");
    expect(job.progressId).toBe("rem-1");
  });

  it("does nothing and makes no API call when no jobs are running", async () => {
    const { store } = buildStoreWithRecorder();

    await store.dispatch(reconcileAppleJobs());

    expect(mockedGetJobProgress).not.toHaveBeenCalled();
  });

  it("swallows network errors without marking the job failed", async () => {
    const { store } = buildStoreWithRecorder();

    store.dispatch(appleJobStarted({ key: "contacts", progressId: "c1" }));
    store.dispatch(appleJobStarted({ key: "notes", progressId: "n1" }));

    mockedGetJobProgress.mockImplementation(async (id: string) => {
      if (id === "c1") throw new Error("network blew up");
      if (id === "n1") {
        return ok({ is_complete: true, status_message: "Done" });
      }
      throw new Error(`unexpected: ${id}`);
    });

    await store.dispatch(reconcileAppleJobs());

    // Network error MUST NOT be conflated with backend-reported failure —
    // contacts stays running, only the explicit is_complete completes.
    const { appleJobs } = store.getState().ingestion;
    expect(appleJobs.contacts.status).toBe("running");
    expect(appleJobs.notes.status).toBe("done");
  });
});

describe("ingestionSlice — appleSyncConfig", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
    // Drain any debounce timer the previous test left armed so module-level
    // state can't leak across tests.
    const buster = buildStore();
    thunkDispatch(buster)(flushAppleSyncDebouncedPatch());
  });

  describe("initial state and selectors", () => {
    it("starts with no sync config and reports not-loaded", () => {
      const store = buildStore();
      const state = store.getState() as RootState;
      expect(selectAppleSyncConfig(state)).toBeNull();
      expect(selectAppleSyncConfigLoaded(state)).toBe(false);
    });

    it("falls back to all-enabled sources and 50-photo limit when no config loaded", () => {
      const store = buildStore();
      const state = store.getState() as RootState;
      expect(selectAppleEnabledSources(state)).toEqual({
        notes: true,
        photos: true,
        calendar: true,
        reminders: true,
        contacts: true,
      });
      expect(selectApplePhotosLimit(state)).toBe(50);
    });

    it("reads sources/photos_limit from a loaded config", () => {
      const store = buildStore();
      store.dispatch(
        appleSyncConfigOptimisticPatch({}),
      );
      // Seed via fulfilled action so the slice transitions to loaded.
      store.dispatch({
        type: fetchAppleSyncConfig.fulfilled.type,
        payload: {
          ...baseSyncConfig,
          sources: { ...baseSyncConfig.sources, contacts: false },
          photos_limit: 17,
        },
      });
      const state = store.getState() as RootState;
      expect(selectAppleEnabledSources(state).contacts).toBe(false);
      expect(selectApplePhotosLimit(state)).toBe(17);
      expect(selectAppleSyncConfigLoaded(state)).toBe(true);
    });
  });

  describe("fetchAppleSyncConfig", () => {
    it("populates appleSyncConfig and flips loaded on success", async () => {
      mockedGetAppleSyncConfig.mockResolvedValueOnce(
        okSyncConfig(baseSyncConfig),
      );

      const store = buildStore();
      await store.dispatch(fetchAppleSyncConfig());

      const state = store.getState() as RootState;
      expect(state.ingestion.appleSyncConfig).toEqual(baseSyncConfig);
      expect(state.ingestion.appleSyncConfigLoaded).toBe(true);
    });

    it("flips loaded even on failure so the UI un-gates without a working config", async () => {
      mockedGetAppleSyncConfig.mockResolvedValueOnce({
        status: 500,
        success: false,
      } as unknown as Awaited<ReturnType<typeof ingestionClient.getAppleSyncConfig>>);

      const store = buildStore();
      await store.dispatch(fetchAppleSyncConfig());

      const state = store.getState() as RootState;
      // Without this guard the SourceCard toggles would stay disabled
      // forever after a failed fetch — verify the rejected reducer
      // un-gates by setting loaded=true while leaving config null.
      expect(state.ingestion.appleSyncConfig).toBeNull();
      expect(state.ingestion.appleSyncConfigLoaded).toBe(true);
    });
  });

  describe("appleSyncConfigOptimisticPatch", () => {
    it("merges partial sources without clobbering other source keys", () => {
      const store = buildStore();
      store.dispatch({
        type: fetchAppleSyncConfig.fulfilled.type,
        payload: baseSyncConfig,
      });

      store.dispatch(
        appleSyncConfigOptimisticPatch({ sources: { contacts: false } }),
      );

      const cfg = store.getState().ingestion.appleSyncConfig;
      expect(cfg?.sources).toEqual({
        notes: true,
        photos: true,
        calendar: true,
        reminders: true,
        contacts: false,
      });
    });

    it("updates photos_limit independently of sources", () => {
      const store = buildStore();
      store.dispatch({
        type: fetchAppleSyncConfig.fulfilled.type,
        payload: baseSyncConfig,
      });

      store.dispatch(
        appleSyncConfigOptimisticPatch({ photos_limit: 250 }),
      );

      const cfg = store.getState().ingestion.appleSyncConfig;
      expect(cfg?.photos_limit).toBe(250);
      expect(cfg?.sources).toEqual(baseSyncConfig.sources);
    });

    it("no-ops when no config has been loaded yet", () => {
      const store = buildStore();
      store.dispatch(
        appleSyncConfigOptimisticPatch({ sources: { notes: false } }),
      );
      // Without a base config there is nothing to merge into; the
      // reducer leaves state untouched. Callers gate this in the UI by
      // disabling toggles until appleSyncConfigLoaded.
      expect(store.getState().ingestion.appleSyncConfig).toBeNull();
    });
  });

  describe("debouncedPatchAppleSyncConfig", () => {
    it("applies the optimistic patch immediately and the API call after the debounce", async () => {
      mockedUpdateAppleSyncConfig.mockResolvedValue(
        okSyncConfig({ ...baseSyncConfig, sources: { ...baseSyncConfig.sources, contacts: false } }),
      );

      const store = buildStore();
      store.dispatch({
        type: fetchAppleSyncConfig.fulfilled.type,
        payload: baseSyncConfig,
      });

      thunkDispatch(store)(
        debouncedPatchAppleSyncConfig({ sources: { contacts: false } }),
      );

      // Optimistic: state already shows contacts off.
      expect(
        store.getState().ingestion.appleSyncConfig?.sources.contacts,
      ).toBe(false);
      // API not called yet — debounce window is still open.
      expect(mockedUpdateAppleSyncConfig).not.toHaveBeenCalled();

      await vi.advanceTimersByTimeAsync(500);

      expect(mockedUpdateAppleSyncConfig).toHaveBeenCalledTimes(1);
      // The PATCH carries the FULL EnabledSources, not the partial — the
      // route at apple_import.rs replaces the whole `sources` field, so
      // sending only `{ contacts: false }` would erase the other four.
      expect(mockedUpdateAppleSyncConfig).toHaveBeenCalledWith({
        sources: {
          notes: true,
          photos: true,
          calendar: true,
          reminders: true,
          contacts: false,
        },
      });
    });

    it("coalesces a flurry of toggles into a single PATCH carrying every change", async () => {
      mockedUpdateAppleSyncConfig.mockResolvedValue(okSyncConfig(baseSyncConfig));

      const store = buildStore();
      store.dispatch({
        type: fetchAppleSyncConfig.fulfilled.type,
        payload: baseSyncConfig,
      });

      // Three quick toggles — the debounce timer resets on each one, so
      // only ONE PATCH should fire and it should carry all three changes.
      const dispatchT = thunkDispatch(store);
      dispatchT(debouncedPatchAppleSyncConfig({ sources: { notes: false } }));
      vi.advanceTimersByTime(100);
      dispatchT(debouncedPatchAppleSyncConfig({ sources: { photos: false } }));
      vi.advanceTimersByTime(100);
      dispatchT(debouncedPatchAppleSyncConfig({ photos_limit: 200 }));

      expect(mockedUpdateAppleSyncConfig).not.toHaveBeenCalled();

      await vi.advanceTimersByTimeAsync(500);

      expect(mockedUpdateAppleSyncConfig).toHaveBeenCalledTimes(1);
      const callArg = mockedUpdateAppleSyncConfig.mock.calls[0][0];
      expect(callArg).toEqual({
        sources: {
          notes: false,
          photos: false,
          calendar: true,
          reminders: true,
          contacts: true,
        },
        photos_limit: 200,
      });
    });

    it("flushAppleSyncDebouncedPatch sends the queued PATCH immediately", async () => {
      mockedUpdateAppleSyncConfig.mockResolvedValue(okSyncConfig(baseSyncConfig));

      const store = buildStore();
      store.dispatch({
        type: fetchAppleSyncConfig.fulfilled.type,
        payload: baseSyncConfig,
      });

      const dispatchT = thunkDispatch(store);
      dispatchT(debouncedPatchAppleSyncConfig({ sources: { contacts: false } }));
      // Imagine the user clicks "Import All" right after toggling — the
      // pending patch must land BEFORE the imports start so the run uses
      // the latest sources, not the pre-toggle ones.
      dispatchT(flushAppleSyncDebouncedPatch());

      // No timer advance needed — flush ran synchronously.
      expect(mockedUpdateAppleSyncConfig).toHaveBeenCalledTimes(1);
    });

    it("flushAppleSyncDebouncedPatch is a no-op when nothing is queued", () => {
      const store = buildStore();
      thunkDispatch(store)(flushAppleSyncDebouncedPatch());
      expect(mockedUpdateAppleSyncConfig).not.toHaveBeenCalled();
    });
  });
});
