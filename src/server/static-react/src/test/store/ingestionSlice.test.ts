import { describe, it, expect, vi, beforeEach } from "vitest";
import { configureStore } from "@reduxjs/toolkit";
import ingestionReducer, {
  fetchIngestionConfig,
  fetchIngestionStatus,
  saveIngestionConfig,
  selectIngestionConfig,
  selectAiConfiguredFromStatus,
  selectAiProvider,
  selectActiveModel,
  selectIsAiConfigured,
} from "../../store/ingestionSlice";
import type { RootState } from "../../store/store";
import type {
  IngestionConfig,
  IngestionStatus,
} from "../../api/clients/ingestionClient";
import { ingestionClient } from "../../api/clients";

// Spy on the singleton instance methods so the same object the slice uses is mocked
const mockGetConfig = vi.spyOn(ingestionClient, "getConfig");
const mockSaveConfig = vi.spyOn(ingestionClient, "saveConfig");
const mockGetStatus = vi.spyOn(ingestionClient, "getStatus");

function createStore(preloadedIngestion = {}) {
  return configureStore({
    reducer: { ingestion: ingestionReducer },
    preloadedState: { ingestion: { config: null, status: null, loading: false, error: null, saving: false, saveError: null, ...preloadedIngestion } },
  });
}

// Helper to build a RootState-shaped object for selector tests
function stateWith(config: IngestionConfig | null): Pick<RootState, "ingestion"> {
  return { ingestion: { config, status: null, loading: false, error: null, saving: false, saveError: null } } as Pick<RootState, "ingestion">;
}

const anthropicConfig: IngestionConfig = {
  provider: "Anthropic",
  anthropic: {
    api_key: "sk-ant-test",
    base_url: "https://api.anthropic.com",
    model: "claude-sonnet-4-20250514",
  },
  ollama: { model: "", base_url: "http://localhost:11434" },
};

const ollamaConfig: IngestionConfig = {
  provider: "Ollama",
  anthropic: { api_key: "", base_url: "https://api.anthropic.com", model: "" },
  ollama: { model: "llama3.3", base_url: "http://localhost:11434" },
};

const envKeyConfig: IngestionConfig = {
  provider: "Anthropic",
  anthropic: {
    api_key: "***configured***",
    base_url: "https://api.anthropic.com",
    model: "claude-sonnet-4-20250514",
  },
  ollama: { model: "", base_url: "http://localhost:11434" },
};

describe("ingestionSlice", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  describe("initial state", () => {
    it("starts with null config and no loading/error", () => {
      const store = createStore();
      const state = store.getState().ingestion;
      expect(state.config).toBeNull();
      expect(state.loading).toBe(false);
      expect(state.error).toBeNull();
      expect(state.saving).toBe(false);
      expect(state.saveError).toBeNull();
    });
  });

  describe("fetchIngestionConfig", () => {
    it("sets loading while pending then stores config on success", async () => {
      mockGetConfig.mockResolvedValue({ success: true, data: anthropicConfig } as any);
      const store = createStore();

      const promise = store.dispatch(fetchIngestionConfig());
      expect(store.getState().ingestion.loading).toBe(true);

      await promise;
      const state = store.getState().ingestion;
      expect(state.loading).toBe(false);
      expect(state.config).toEqual(anthropicConfig);
      expect(state.error).toBeNull();
    });

    it("sets error on API failure", async () => {
      mockGetConfig.mockResolvedValue({ success: false, data: null } as any);
      const store = createStore();

      await store.dispatch(fetchIngestionConfig());
      const state = store.getState().ingestion;
      expect(state.loading).toBe(false);
      expect(state.config).toBeNull();
      expect(state.error).toBe("Failed to fetch ingestion config");
    });

    it("sets error on network exception", async () => {
      mockGetConfig.mockRejectedValue(new Error("Network error"));
      const store = createStore();

      await store.dispatch(fetchIngestionConfig());
      const state = store.getState().ingestion;
      expect(state.loading).toBe(false);
      expect(state.error).toBe("Network error");
    });
  });

  describe("fetchIngestionStatus", () => {
    it("stores status on success", async () => {
      const status: IngestionStatus = {
        configured: true,
        enabled: true,
        provider: "Anthropic",
        model: "claude-sonnet-4-20250514",
        auto_execute_mutations: false,
      };
      mockGetStatus.mockResolvedValue({ success: true, data: status } as any);
      const store = createStore();

      await store.dispatch(fetchIngestionStatus());
      expect(store.getState().ingestion.status).toEqual(status);
    });

    it("leaves status null when API returns failure", async () => {
      mockGetStatus.mockResolvedValue({ success: false, data: null } as any);
      const store = createStore();

      await store.dispatch(fetchIngestionStatus());
      expect(store.getState().ingestion.status).toBeNull();
    });
  });

  describe("saveIngestionConfig", () => {
    it("saves then re-fetches config + status on success", async () => {
      const ollamaStatus: IngestionStatus = {
        configured: true,
        enabled: true,
        provider: "Ollama",
        model: "llama3.3",
        auto_execute_mutations: false,
      };
      mockSaveConfig.mockResolvedValue({ success: true, data: { success: true, message: "ok" } } as any);
      mockGetConfig.mockResolvedValue({ success: true, data: ollamaConfig } as any);
      mockGetStatus.mockResolvedValue({ success: true, data: ollamaStatus } as any);
      const store = createStore();

      const promise = store.dispatch(saveIngestionConfig(ollamaConfig));
      expect(store.getState().ingestion.saving).toBe(true);

      await promise;
      const state = store.getState().ingestion;
      expect(state.saving).toBe(false);
      expect(state.saveError).toBeNull();
      // Config + status should both be populated from the re-fetch
      expect(state.config).toEqual(ollamaConfig);
      expect(state.status).toEqual(ollamaStatus);
      expect(mockSaveConfig).toHaveBeenCalledWith(ollamaConfig);
      expect(mockGetConfig).toHaveBeenCalledTimes(1);
      expect(mockGetStatus).toHaveBeenCalledTimes(1);
    });

    it("sets saveError when API returns failure", async () => {
      mockSaveConfig.mockResolvedValue({ success: false } as any);
      const store = createStore();

      await store.dispatch(saveIngestionConfig(anthropicConfig));
      const state = store.getState().ingestion;
      expect(state.saving).toBe(false);
      expect(state.saveError).toBe("Failed to save ingestion config");
    });

    it("sets saveError on network exception", async () => {
      mockSaveConfig.mockRejectedValue(new Error("Save failed"));
      const store = createStore();

      await store.dispatch(saveIngestionConfig(anthropicConfig));
      const state = store.getState().ingestion;
      expect(state.saving).toBe(false);
      expect(state.saveError).toBe("Save failed");
    });
  });

  describe("selectors", () => {
    describe("selectIngestionConfig", () => {
      it("returns null when no config", () => {
        expect(selectIngestionConfig(stateWith(null) as RootState)).toBeNull();
      });

      it("returns the config object", () => {
        expect(selectIngestionConfig(stateWith(anthropicConfig) as RootState)).toEqual(anthropicConfig);
      });
    });

    describe("selectAiProvider", () => {
      it("returns null when no config", () => {
        expect(selectAiProvider(stateWith(null) as RootState)).toBeNull();
      });

      it("returns Anthropic", () => {
        expect(selectAiProvider(stateWith(anthropicConfig) as RootState)).toBe("Anthropic");
      });

      it("returns Ollama", () => {
        expect(selectAiProvider(stateWith(ollamaConfig) as RootState)).toBe("Ollama");
      });
    });

    describe("selectActiveModel", () => {
      it("returns null when no config", () => {
        expect(selectActiveModel(stateWith(null) as RootState)).toBeNull();
      });

      it("returns Anthropic model when provider is Anthropic", () => {
        expect(selectActiveModel(stateWith(anthropicConfig) as RootState)).toBe("claude-sonnet-4-20250514");
      });

      it("returns Ollama model when provider is Ollama", () => {
        expect(selectActiveModel(stateWith(ollamaConfig) as RootState)).toBe("llama3.3");
      });
    });

    describe("selectIsAiConfigured", () => {
      it("returns false when no config", () => {
        expect(selectIsAiConfigured(stateWith(null) as RootState)).toBe(false);
      });

      it("returns true for Anthropic with api_key", () => {
        expect(selectIsAiConfigured(stateWith(anthropicConfig) as RootState)).toBe(true);
      });

      it("returns true for Anthropic with redacted env key", () => {
        expect(selectIsAiConfigured(stateWith(envKeyConfig) as RootState)).toBe(true);
      });

      it("returns false for Anthropic without api_key", () => {
        const noKey: IngestionConfig = {
          ...anthropicConfig,
          anthropic: { ...anthropicConfig.anthropic, api_key: "" },
        };
        expect(selectIsAiConfigured(stateWith(noKey) as RootState)).toBe(false);
      });

      it("returns true for Ollama with model", () => {
        expect(selectIsAiConfigured(stateWith(ollamaConfig) as RootState)).toBe(true);
      });

      it("returns false for Ollama without model", () => {
        const noModel: IngestionConfig = {
          ...ollamaConfig,
          ollama: { ...ollamaConfig.ollama, model: "" },
        };
        expect(selectIsAiConfigured(stateWith(noModel) as RootState)).toBe(false);
      });
    });

    describe("selectAiConfiguredFromStatus", () => {
      const stateWithStatus = (status: IngestionStatus | null): Pick<RootState, "ingestion"> =>
        ({
          ingestion: {
            config: null,
            status,
            loading: false,
            error: null,
            saving: false,
            saveError: null,
          },
        }) as Pick<RootState, "ingestion">;

      it("returns null before the first status fetch lands", () => {
        expect(selectAiConfiguredFromStatus(stateWithStatus(null) as RootState)).toBeNull();
      });

      it("returns true when backend reports configured", () => {
        const status: IngestionStatus = {
          configured: true,
          enabled: true,
          provider: "Anthropic",
          model: "claude-sonnet-4-20250514",
          auto_execute_mutations: false,
        };
        expect(selectAiConfiguredFromStatus(stateWithStatus(status) as RootState)).toBe(true);
      });

      it("returns false when backend reports not configured", () => {
        const status: IngestionStatus = {
          configured: false,
          enabled: true,
          provider: "Anthropic",
          model: "",
          auto_execute_mutations: false,
        };
        expect(selectAiConfiguredFromStatus(stateWithStatus(status) as RootState)).toBe(false);
      });
    });

  });
});