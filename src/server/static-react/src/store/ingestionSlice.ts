import { createSlice, createAsyncThunk } from "@reduxjs/toolkit";
import { ingestionClient } from "../api/clients";
import type {
  IngestionConfig,
  IngestionStatus,
} from "../api/clients/ingestionClient";
import type { RootState } from "./store";

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
}

const initialState: IngestionState = {
  config: null,
  status: null,
  loading: false,
  error: null,
  saving: false,
  saveError: null,
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
  reducers: {},
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

export default ingestionSlice.reducer;
