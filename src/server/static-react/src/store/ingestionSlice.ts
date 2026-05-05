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
