import {
  createSlice,
  createAsyncThunk,
  type Action,
  type PayloadAction,
  type ThunkAction,
} from "@reduxjs/toolkit";
import { ingestionClient } from "../api/clients";
import type {
  AppleSyncConfig,
  EnabledSources,
  IngestionConfig,
  IngestionStatus,
} from "../api/clients/ingestionClient";
import type { AppDispatch, RootState } from "./store";

export type AppleSourceKey =
  | "notes"
  | "photos"
  | "calendar"
  | "reminders"
  | "contacts";

export interface ImportResult {
  total?: number;
  ingested?: number;
}

export type AppleJobStatus = "idle" | "running" | "done" | "error";

export interface AppleJob {
  progressId: string | null;
  status: AppleJobStatus;
  progress: number;
  message: string;
  result: ImportResult | null;
}

const APPLE_SOURCE_KEYS: readonly AppleSourceKey[] = [
  "notes",
  "photos",
  "calendar",
  "reminders",
  "contacts",
];

export const makeIdleAppleJob = (): AppleJob => ({
  progressId: null,
  status: "idle",
  progress: 0,
  message: "",
  result: null,
});

const makeInitialAppleJobs = (): Record<AppleSourceKey, AppleJob> =>
  APPLE_SOURCE_KEYS.reduce(
    (acc, key) => {
      acc[key] = makeIdleAppleJob();
      return acc;
    },
    {} as Record<AppleSourceKey, AppleJob>,
  );

interface IngestionState {
  config: IngestionConfig | null;
  // Authoritative readiness from `/api/ingestion/status`. The setup banner
  // and the AI pill key off `status.configured` so the UI agrees with the
  // backend's `IngestionConfig::is_ready()` instead of guessing from the
  // redacted GET /config payload (where `api_key` is masked to
  // "***configured***").
  status: IngestionStatus | null;
  loading: boolean;
  error: string | null;
  saving: boolean;
  saveError: string | null;
  appleJobs: Record<AppleSourceKey, AppleJob>;
  // Backend-authoritative AppleSyncConfig. Both the Auto-Sync settings and
  // the per-source Import-All cards read `sources` and `photos_limit` from
  // here so the two surfaces stay in lockstep — toggling Notes off in one
  // place reflects in the other. `null` until the first fetch resolves.
  appleSyncConfig: AppleSyncConfig | null;
  appleSyncConfigLoaded: boolean;
}

const initialState: IngestionState = {
  config: null,
  status: null,
  loading: false,
  error: null,
  saving: false,
  saveError: null,
  appleJobs: makeInitialAppleJobs(),
  appleSyncConfig: null,
  appleSyncConfigLoaded: false,
};

export const fetchIngestionConfig = createAsyncThunk(
  "ingestion/fetchConfig",
  async (_, { rejectWithValue }) => {
    try {
      const response = await ingestionClient.getConfig();
      if (response.success && response.data) {
        return response.data;
      }
      return rejectWithValue("Failed to fetch ingestion config");
    } catch (error) {
      return rejectWithValue(
        error instanceof Error ? error.message : String(error),
      );
    }
  },
);

export const fetchIngestionStatus = createAsyncThunk(
  "ingestion/fetchStatus",
  async (_, { rejectWithValue }) => {
    try {
      const response = await ingestionClient.getStatus();
      if (response.success && response.data) {
        return response.data;
      }
      return rejectWithValue("Failed to fetch ingestion status");
    } catch (error) {
      return rejectWithValue(
        error instanceof Error ? error.message : String(error),
      );
    }
  },
);

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

// The /api/ingestion/progress LIST response carries `job_type` so the UI
// can route apple-* jobs back to their source card. The IngestionProgress
// type in the client doesn't model that field today; this local extension
// keeps the cast narrow while we read it.
interface ProgressListItem extends JobProgressShape {
  id: string;
  job_type?: string;
}

const APPLE_JOB_TYPE_TO_KEY: Record<string, AppleSourceKey> = {
  "apple-notes": "notes",
  "apple-photos": "photos",
  "apple-calendar": "calendar",
  "apple-reminders": "reminders",
  "apple-contacts": "contacts",
};

// The backend wraps the list in `{ progress: [...] }` post-2026-04 but
// older deployments return a bare array; tolerate both. Anything else
// produces an empty list — reconcile is best-effort.
const extractProgressList = (raw: unknown): ProgressListItem[] => {
  if (Array.isArray(raw)) return raw as ProgressListItem[];
  if (raw && typeof raw === "object") {
    const inner = (raw as { progress?: unknown }).progress;
    if (Array.isArray(inner)) return inner as ProgressListItem[];
  }
  return [];
};

// Plain Dispatch<Action> is intentionally narrower than AppDispatch — the
// thunk's `dispatch` arg is typed `unknown`-bound (createAsyncThunk doesn't
// thread RootState through), and we only emit plain action creators here,
// so the loose signature avoids the cross-store ThunkDispatch mismatch.
type AnyDispatch = (action: Action) => void;

const settleAppleJobFromProgress = (
  dispatch: AnyDispatch,
  key: AppleSourceKey,
  progressId: string,
  data: JobProgressShape,
) => {
  const progress = data.progress_percentage ?? 0;
  const message = data.status_message ?? data.message ?? "";

  // Check is_failed BEFORE is_complete: a terminally-failed job has both
  // flags set on the wire (see fold_db::progress::Job). Reading
  // is_complete first would reconcile failures into the `done` state.
  if (data.is_failed) {
    dispatch(
      appleJobFailed({
        key,
        message: data.error_message ?? data.message ?? "Import failed",
      }),
    );
    return;
  }
  if (data.is_complete) {
    dispatch(
      appleJobCompleted({
        key,
        result: data.results ?? data.result ?? null,
        message,
      }),
    );
    return;
  }
  // Re-arm the listener's poll loop. appleJobStarted first (it clobbers
  // progress/message back to "Starting..."), then appleJobProgressed so
  // the UI doesn't briefly snap to 5% before the next poll tick.
  dispatch(appleJobStarted({ key, progressId }));
  dispatch(appleJobProgressed({ key, progress, message }));
};

/**
 * Walk every Apple job state on app mount and bring it into agreement with
 * the backend.
 *
 * Two paths run in parallel:
 *
 * 1. Per-id probe — for any slice entry whose status is `running` and which
 *    already carries a `progressId`, fetch /ingestion/progress/{id} and
 *    settle to done / error / re-armed running.
 *
 * 2. List adoption — fetch /ingestion/progress (the user's full job list)
 *    and adopt apple-* job_types into source cards that are otherwise idle
 *    or stuck "running but progressId=null". This is what recovers the
 *    page-reload case: the slice resets to all-idle on reload, but the
 *    backend's job is still alive (or already failed). Without this,
 *    reload after a stuck-at-Starting import would never settle.
 *
 * Path (2) also closes the orphan-progressId case from
 * dogfood-2026-05-10: if `appleJobStarting` fired but `appleJobStarted`
 * never did (e.g. tab was closed mid-await), the next mount still walks
 * the user back into the right state by matching on job_type.
 */
export const reconcileAppleJobs = createAsyncThunk(
  "ingestion/reconcileAppleJobs",
  async (_, { dispatch, getState }) => {
    // getState() is typed via RootState here — accessing only the
    // ingestion slice keeps the cast narrow and safe across test stores
    // that use a slice-only reducer.
    const { appleJobs } = (getState() as RootState).ingestion;
    const knownProgressIds = new Set<string>();
    const orphans: AppleSourceKey[] = []; // running, but no progressId yet
    const idleKeys: AppleSourceKey[] = [];

    const perIdProbes: Array<Promise<void>> = [];
    for (const [key, job] of Object.entries(appleJobs) as [
      AppleSourceKey,
      AppleJob,
    ][]) {
      if (job.status === "running" && job.progressId !== null) {
        const progressId = job.progressId;
        knownProgressIds.add(progressId);
        perIdProbes.push(
          (async () => {
            try {
              const resp = await ingestionClient.getJobProgress(progressId);
              if (!resp.success || !resp.data) return;
              settleAppleJobFromProgress(
                dispatch,
                key,
                progressId,
                resp.data as JobProgressShape,
              );
            } catch {
              // One failed probe must not kill the others. We only mark a
              // job failed when the backend explicitly says so — never on
              // a transient network error.
            }
          })(),
        );
      } else if (job.status === "running" && job.progressId === null) {
        orphans.push(key);
      } else if (job.status === "idle") {
        idleKeys.push(key);
      }
    }

    const listProbe = (async () => {
      try {
        const resp = await ingestionClient.getAllProgress();
        if (!resp.success || !resp.data) return;
        const items = extractProgressList(resp.data);

        // Pick the most recent unsettled apple-* job per source key. The
        // server returns newest-first, so we keep the first match per key
        // and skip everything we've already probed via the per-id path.
        const byKey = new Map<AppleSourceKey, ProgressListItem>();
        for (const item of items) {
          if (!item.id || !item.job_type) continue;
          if (knownProgressIds.has(item.id)) continue;
          const key = APPLE_JOB_TYPE_TO_KEY[item.job_type];
          if (!key) continue;
          if (byKey.has(key)) continue;
          byKey.set(key, item);
        }

        for (const [key, item] of byKey) {
          // Only adopt into idle slots and orphan running-but-null slots.
          // A slot already carrying a progressId is owned by the per-id
          // probe; touching it here would race the listener.
          if (!idleKeys.includes(key) && !orphans.includes(key)) continue;
          settleAppleJobFromProgress(dispatch, key, item.id, item);
        }
      } catch {
        // List probe failure is non-fatal — leave any existing slice
        // entries untouched. Worst case the user sees the pre-reload
        // state until they retry.
      }
    })();

    await Promise.all([...perIdProbes, listProbe]);
  },
);

export const fetchAppleSyncConfig = createAsyncThunk(
  "ingestion/fetchAppleSyncConfig",
  async (_, { rejectWithValue }) => {
    try {
      const response = await ingestionClient.getAppleSyncConfig();
      if (response.success && response.data) {
        return response.data;
      }
      return rejectWithValue("Failed to fetch Apple sync config");
    } catch (error) {
      return rejectWithValue(
        error instanceof Error ? error.message : String(error),
      );
    }
  },
);

export const patchAppleSyncConfig = createAsyncThunk(
  "ingestion/patchAppleSyncConfig",
  async (update: Partial<AppleSyncConfig>, { rejectWithValue }) => {
    try {
      const response = await ingestionClient.updateAppleSyncConfig(update);
      if (response.success && response.data) {
        return response.data;
      }
      return rejectWithValue("Failed to update Apple sync config");
    } catch (error) {
      return rejectWithValue(
        error instanceof Error ? error.message : String(error),
      );
    }
  },
);

// AppleSyncConfig patch type — like Partial<AppleSyncConfig> except
// `sources` is itself partial, so a single-source toggle is expressible
// as `{ sources: { notes: false } }` instead of forcing the caller to
// re-spread the whole EnabledSources object every time.
export type AppleSyncConfigPatch =
  Omit<Partial<AppleSyncConfig>, "sources"> & {
    sources?: Partial<EnabledSources>;
  };

// Debounce wrapper around `patchAppleSyncConfig`. The reducer applies the
// optimistic patch synchronously so the toggle UI flips on click; the API
// call is deferred and coalesced so a flurry of toggles or photos-limit
// keystrokes turns into a single PATCH carrying the merged delta. Without
// this the photos-limit `<input type=number>` would PATCH on every digit.
//
// Why getState() instead of buffering the raw patches: two consecutive
// toggles produce two partial-sources patches (`{notes:false}` then
// `{photos:false}`). We need to send the BACKEND a complete EnabledSources
// object (the route at apple_import.rs replaces the whole `sources` field
// — it doesn't merge). Reading getState() at flush time gives us the
// canonical post-optimistic state, so we emit a faithful snapshot of
// every dirty top-level field instead of trying to merge partial patches.
const APPLE_SYNC_DEBOUNCE_MS = 500;
let appleSyncDebounceTimer: ReturnType<typeof setTimeout> | null = null;
const appleSyncDirtyFields = new Set<keyof AppleSyncConfig>();

// Thunks below are typed with ThunkAction (rather than annotating dispatch as
// `AppDispatch` directly) so they cross store boundaries cleanly. Test stores
// built with bare `combineReducers` have a Dispatch that doesn't carry the
// thunk overloads of the production AppDispatch — the explicit ThunkAction
// return type makes the type compatible with both.
type AppleSyncThunk = ThunkAction<void, RootState, unknown, Action>;

function flushDirtyFields(dispatch: AppDispatch, getState: () => RootState) {
  if (appleSyncDirtyFields.size === 0) return;
  const cfg = getState().ingestion.appleSyncConfig;
  if (!cfg) {
    appleSyncDirtyFields.clear();
    return;
  }
  const payload: Partial<AppleSyncConfig> = {};
  for (const field of appleSyncDirtyFields) {
    // Field-by-field copy keeps the type discriminations sound; a generic
    // `payload[field] = cfg[field]` trips TS variance.
    if (field === "enabled") payload.enabled = cfg.enabled;
    else if (field === "schedule") payload.schedule = cfg.schedule;
    else if (field === "sources") payload.sources = cfg.sources;
    else if (field === "photos_limit") payload.photos_limit = cfg.photos_limit;
    // last_sync / next_sync / last_error / last_error_at are sidecar
    // fields the backend owns — we never PATCH them outbound, so skip.
  }
  appleSyncDirtyFields.clear();
  if (Object.keys(payload).length > 0) {
    dispatch(patchAppleSyncConfig(payload));
  }
}

export const flushAppleSyncDebouncedPatch =
  (): AppleSyncThunk => (dispatch, getState) => {
    if (appleSyncDebounceTimer === null) return;
    clearTimeout(appleSyncDebounceTimer);
    appleSyncDebounceTimer = null;
    flushDirtyFields(dispatch as AppDispatch, getState);
  };

const PATCHABLE_FIELDS: ReadonlyArray<keyof AppleSyncConfig> = [
  "enabled",
  "schedule",
  "sources",
  "photos_limit",
];

export const debouncedPatchAppleSyncConfig =
  (patch: AppleSyncConfigPatch): AppleSyncThunk =>
  (dispatch, getState) => {
    dispatch(appleSyncConfigOptimisticPatch(patch));
    for (const field of PATCHABLE_FIELDS) {
      if (patch[field] !== undefined) {
        appleSyncDirtyFields.add(field);
      }
    }
    if (appleSyncDebounceTimer !== null) {
      clearTimeout(appleSyncDebounceTimer);
    }
    appleSyncDebounceTimer = setTimeout(() => {
      appleSyncDebounceTimer = null;
      flushDirtyFields(dispatch as AppDispatch, getState);
    }, APPLE_SYNC_DEBOUNCE_MS);
  };

export const saveIngestionConfig = createAsyncThunk(
  "ingestion/saveConfig",
  async (config: IngestionConfig, { dispatch, rejectWithValue }) => {
    try {
      const response = await ingestionClient.saveConfig(config);
      if (response.success) {
        // Re-fetch both the config (for fields the UI renders) and the
        // status (for `configured` — drives the setup banner and pill).
        await Promise.all([
          dispatch(fetchIngestionConfig()).unwrap(),
          dispatch(fetchIngestionStatus()).unwrap(),
        ]);
        return true;
      }
      return rejectWithValue("Failed to save ingestion config");
    } catch (error) {
      return rejectWithValue(
        error instanceof Error ? error.message : String(error),
      );
    }
  },
);

const ingestionSlice = createSlice({
  name: "ingestion",
  initialState,
  reducers: {
    /**
     * Optimistic transition into `running` BEFORE the start POST
     * resolves, so the UI never shows the click-after-done window as
     * `idle`. The listener middleware doesn't fire on this action — no
     * `progressId` to poll yet — that's intentional. `appleJobStarted`
     * follows once the POST resolves with a real `progressId`, and that
     * is what arms the polling fork.
     *
     * Bug it closes: under heavy backend load (HEIC conversion + 100+
     * notes ingest + contacts permission timeout), POSTs to
     * `/apple-import/<source>` can stay in flight for tens of seconds.
     * The previous flow dispatched `appleJobReset` first and then
     * scheduled `start()` on a 0ms timeout, which left every job
     * visibly `idle` for the duration of the in-flight POSTs. Anyone
     * inspecting the Redux store during that window — or any flow that
     * never gets a POST response — saw five idle entries even though
     * the user just kicked off five imports.
     */
    appleJobStarting(
      state,
      action: PayloadAction<{ key: AppleSourceKey }>,
    ) {
      state.appleJobs[action.payload.key] = {
        progressId: null,
        status: "running",
        progress: 5,
        message: "Starting...",
        result: null,
      };
    },
    appleJobStarted(
      state,
      action: PayloadAction<{ key: AppleSourceKey; progressId: string }>,
    ) {
      const { key, progressId } = action.payload;
      state.appleJobs[key] = {
        progressId,
        status: "running",
        progress: 5,
        message: "Starting...",
        result: null,
      };
    },
    appleJobProgressed(
      state,
      action: PayloadAction<{
        key: AppleSourceKey;
        progress: number;
        message: string;
      }>,
    ) {
      const { key, progress, message } = action.payload;
      const job = state.appleJobs[key];
      if (job.status !== "running") return;
      job.progress = progress;
      job.message = message;
    },
    appleJobCompleted(
      state,
      action: PayloadAction<{
        key: AppleSourceKey;
        result: ImportResult | null;
        message: string;
      }>,
    ) {
      const { key, result, message } = action.payload;
      const job = state.appleJobs[key];
      job.status = "done";
      job.result = result;
      job.message = message;
    },
    appleJobFailed(
      state,
      action: PayloadAction<{ key: AppleSourceKey; message: string }>,
    ) {
      const { key, message } = action.payload;
      const job = state.appleJobs[key];
      job.status = "error";
      job.message = message;
    },
    appleJobReset(state, action: PayloadAction<{ key: AppleSourceKey }>) {
      state.appleJobs[action.payload.key] = makeIdleAppleJob();
    },
    /**
     * Apply a partial AppleSyncConfig patch to local state without
     * waiting for the backend. Drives the snappy toggle/limit feedback
     * for the per-source cards and the Auto-Sync panel; the backend
     * PATCH is fired (debounced) by `debouncedPatchAppleSyncConfig` and
     * its `fulfilled` reducer reconciles to the canonical server state.
     *
     * No-op when no config has been loaded yet — without a base shape
     * there is nothing to merge into. Toggles in that window are
     * disabled by the UI, so this branch should not fire in practice.
     */
    appleSyncConfigOptimisticPatch(
      state,
      action: PayloadAction<AppleSyncConfigPatch>,
    ) {
      const cfg = state.appleSyncConfig;
      if (!cfg) return;
      const patch = action.payload;
      if (patch.enabled !== undefined) cfg.enabled = patch.enabled;
      if (patch.schedule !== undefined) cfg.schedule = patch.schedule;
      if (patch.sources) cfg.sources = { ...cfg.sources, ...patch.sources };
      if (patch.photos_limit !== undefined) {
        cfg.photos_limit = patch.photos_limit;
      }
      if (patch.last_sync !== undefined) cfg.last_sync = patch.last_sync;
      if (patch.next_sync !== undefined) cfg.next_sync = patch.next_sync;
      if (patch.last_error !== undefined) cfg.last_error = patch.last_error;
      if (patch.last_error_at !== undefined) {
        cfg.last_error_at = patch.last_error_at;
      }
    },
  },
  extraReducers: (builder) => {
    builder
      .addCase(fetchIngestionConfig.pending, (state) => {
        state.loading = true;
        state.error = null;
      })
      .addCase(fetchIngestionConfig.fulfilled, (state, action) => {
        state.config = action.payload;
        state.loading = false;
        state.error = null;
      })
      .addCase(fetchIngestionConfig.rejected, (state, action) => {
        state.loading = false;
        state.error = (action.payload as string) ?? "Unknown error";
      })
      .addCase(fetchIngestionStatus.fulfilled, (state, action) => {
        state.status = action.payload;
      })
      .addCase(saveIngestionConfig.pending, (state) => {
        state.saving = true;
        state.saveError = null;
      })
      .addCase(saveIngestionConfig.fulfilled, (state) => {
        state.saving = false;
        state.saveError = null;
      })
      .addCase(saveIngestionConfig.rejected, (state, action) => {
        state.saving = false;
        state.saveError = (action.payload as string) ?? "Unknown error";
      })
      .addCase(fetchAppleSyncConfig.fulfilled, (state, action) => {
        state.appleSyncConfig = action.payload;
        state.appleSyncConfigLoaded = true;
      })
      .addCase(fetchAppleSyncConfig.rejected, (state) => {
        state.appleSyncConfigLoaded = true;
      })
      .addCase(patchAppleSyncConfig.fulfilled, (state, action) => {
        state.appleSyncConfig = action.payload;
        state.appleSyncConfigLoaded = true;
      });
  },
});

// Selectors
export const selectIngestionConfig = (state: RootState) =>
  state.ingestion.config;

export const selectIngestionStatus = (state: RootState) =>
  state.ingestion.status;

/**
 * Backend-authoritative readiness from `/api/ingestion/status`. Returns
 * `null` until the first successful fetch — callers that need to gate
 * banner visibility (avoid flashing "configure AI" before the status
 * lands) should null-check this rather than collapsing to `false`.
 */
export const selectAiConfiguredFromStatus = (state: RootState) =>
  state.ingestion.status?.configured ?? null;

export const selectAiProvider = (state: RootState) =>
  state.ingestion.config?.provider ?? null;

/** Get the active provider's config object based on the selected provider. */
const getActiveProviderConfig = (config: IngestionConfig | null) => {
  if (!config) return null;
  const key = config.provider.toLowerCase() as keyof Pick<IngestionConfig, "ollama" | "anthropic">;
  return config[key] ?? null;
};

export const selectActiveModel = (state: RootState) => {
  const providerConfig = getActiveProviderConfig(state.ingestion.config);
  return providerConfig?.model ?? null;
};

export const selectIsAiConfigured = (state: RootState) => {
  const providerConfig = getActiveProviderConfig(state.ingestion.config);
  if (!providerConfig) return false;
  // Cloud providers need an API key; Ollama just needs a model
  if ("api_key" in providerConfig) return !!providerConfig.api_key;
  return !!providerConfig.model;
};

export const selectAppleJob =
  (key: AppleSourceKey) =>
  (state: RootState): AppleJob =>
    state.ingestion.appleJobs[key];

export const selectAppleSyncConfig = (state: RootState): AppleSyncConfig | null =>
  state.ingestion.appleSyncConfig;

export const selectAppleSyncConfigLoaded = (state: RootState): boolean =>
  state.ingestion.appleSyncConfigLoaded;

const DEFAULT_ENABLED_SOURCES: EnabledSources = {
  notes: true,
  photos: true,
  calendar: true,
  reminders: true,
  contacts: true,
};

export const selectAppleEnabledSources = (state: RootState): EnabledSources =>
  state.ingestion.appleSyncConfig?.sources ?? DEFAULT_ENABLED_SOURCES;

export const selectApplePhotosLimit = (state: RootState): number =>
  state.ingestion.appleSyncConfig?.photos_limit ?? 50;

export const {
  appleJobStarting,
  appleJobStarted,
  appleJobProgressed,
  appleJobCompleted,
  appleJobFailed,
  appleJobReset,
  appleSyncConfigOptimisticPatch,
} = ingestionSlice.actions;

export default ingestionSlice.reducer;
