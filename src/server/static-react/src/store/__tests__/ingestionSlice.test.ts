import { describe, it, expect } from "vitest";
import { combineReducers, configureStore } from "@reduxjs/toolkit";
import ingestionReducer, {
  appleJobStarted,
  appleJobProgressed,
  appleJobCompleted,
  appleJobFailed,
  appleJobReset,
  makeIdleAppleJob,
  selectAppleJob,
  type AppleSourceKey,
} from "../ingestionSlice";

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
      expect(job.progress).toBe(0);
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
