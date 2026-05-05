import { createSlice, createAsyncThunk, type PayloadAction } from "@reduxjs/toolkit";
import { ingestionClient } from "../api/clients";
import type {
  IngestionConfig,
  IngestionStatus,
} from "../api/clients/ingestionClient";
import type { RootState } from "./store";

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
}

const initialState: IngestionState = {
  config: null,
  status: null,
  loading: false,
  error: null,
  saving: false,
  saveError: null,
  appleJobs: makeInitialAppleJobs(),
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

/**
 * On app mount, walk every Apple job currently `running` in the store and
 * ask the backend for its current state. Without this, a job that finished
 * while the user was on another tab can stay stuck at "running 87%" forever
 * because the listener middleware only re-arms on a fresh `appleJobStarted`.
 *
 * For still-running jobs we re-dispatch `appleJobStarted` so the listener
 * forks a fresh poll loop (the middleware cancels any prior fork for the
 * same key — see appleJobsMiddleware.ts).
 */
export const reconcileAppleJobs = createAsyncThunk(
  "ingestion/reconcileAppleJobs",
  async (_, { dispatch, getState }) => {
    // getState() is typed `unknown` here on purpose — the thunk doesn't
    // bind a RootState generic so it stays dispatchable from test stores
    // that use a slice-only reducer. Accessing only the ingestion slice
    // is the entire surface, so the local cast is narrow and safe.
    const { appleJobs } = (getState() as RootState).ingestion;
    const running = (
      Object.entries(appleJobs) as [AppleSourceKey, AppleJob][]
    ).filter(
      ([, job]) => job.status === "running" && job.progressId !== null,
    );

    await Promise.all(
      running.map(async ([key, job]) => {
        const progressId = job.progressId as string;
        try {
          const resp = await ingestionClient.getJobProgress(progressId);
          if (!resp.success || !resp.data) return;
          const data = resp.data as JobProgressShape;
          const progress = data.progress_percentage ?? 0;
          const message = data.status_message ?? data.message ?? "";

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
          if (data.is_failed) {
            dispatch(
              appleJobFailed({
                key,
                message:
                  data.error_message ?? data.message ?? "Import failed",
              }),
            );
            return;
          }
          // Re-arm the listener's poll loop. appleJobStarted first (it
          // clobbers progress/message back to "Starting..."), then
          // appleJobProgressed to immediately reflect the latest backend
          // state — avoids a visible snap to 5% before the next poll tick.
          dispatch(appleJobStarted({ key, progressId }));
          dispatch(appleJobProgressed({ key, progress, message }));
        } catch {
          // Swallow individual fetch errors — one failed reconcile
          // shouldn't kill the others. Don't surface as appleJobFailed;
          // we only mark a job failed when the backend explicitly says so.
        }
      }),
    );
  },
);

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

export const {
  appleJobStarted,
  appleJobProgressed,
  appleJobCompleted,
  appleJobFailed,
  appleJobReset,
} = ingestionSlice.actions;

export default ingestionSlice.reducer;
