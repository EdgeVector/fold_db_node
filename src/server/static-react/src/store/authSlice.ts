import { createSlice, createAsyncThunk, PayloadAction } from "@reduxjs/toolkit";
import {
  getAutoIdentity,
} from "../api/clients/systemClient";
import { getNodePublicKey } from "../api/clients/systemClient";
import { BROWSER_CONFIG } from "../constants/config";

export interface KeyAuthenticationState {
  isAuthenticated: boolean;
  user?: {
    id: string;
    hash: string;
  };
  systemPublicKey: string | null;
  systemKeyId: string | null;
  publicKeyId: string | null;
  isLoading: boolean;
  error: string | null;
  // Magic link auth state
  email: string | null;
  magicLinkPending: boolean;
  sessionToken: string | null;
  apiKey: string | null;
}

const initialState: KeyAuthenticationState = {
  isAuthenticated: false,
  systemPublicKey: null,
  systemKeyId: null,
  publicKeyId: null,
  isLoading: false,
  error: null,
  email: null,
  magicLinkPending: false,
  sessionToken: null,
  apiKey: null,
};

// Async thunk for auto-login using the node's public key identity (all modes)
export const autoLogin = createAsyncThunk(
  "auth/autoLogin",
  async (_, { rejectWithValue }) => {
    try {
      const response = await getAutoIdentity();
      if (
        response.success &&
        response.data?.user_id &&
        response.data?.user_hash
      ) {
        const { user_id, user_hash } = response.data;
        localStorage.setItem(BROWSER_CONFIG.STORAGE_KEYS.USER_ID, user_id);
        localStorage.setItem(BROWSER_CONFIG.STORAGE_KEYS.USER_HASH, user_hash);
        return { id: user_id, hash: user_hash };
      }
      return rejectWithValue("Auto-identity endpoint returned no data");
    } catch (err) {
      return rejectWithValue(
        err instanceof Error ? err.message : "Failed to auto-login",
      );
    }
  },
);

// Async thunk for loading the system public key for display
export const loadSystemPublicKey = createAsyncThunk(
  "auth/loadSystemPublicKey",
  async (_, { rejectWithValue }) => {
    try {
      const response = await getNodePublicKey();
      if (response.success && response.data?.key) {
        return {
          systemPublicKey: response.data.key,
          systemKeyId: null,
        };
      }
      return rejectWithValue("No public key returned");
    } catch (err) {
      return rejectWithValue(
        err instanceof Error ? err.message : "Failed to load system public key",
      );
    }
  },
);

// Async thunk for starting magic link verification
export const startMagicLink = createAsyncThunk(
  "auth/startMagicLink",
  async (email: string, { rejectWithValue }) => {
    try {
      const resp = await fetch("/api/auth/magic-link/start", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ email }),
      });
      const data = await resp.json();
      if (data.ok) {
        return { email };
      }
      return rejectWithValue(data.error || "Failed to send verification code");
    } catch (err) {
      return rejectWithValue(
        err instanceof Error ? err.message : "Network error",
      );
    }
  },
);

// Async thunk for verifying magic link code
export const verifyMagicLink = createAsyncThunk(
  "auth/verifyMagicLink",
  async (
    { email, code }: { email: string; code: string },
    { rejectWithValue },
  ) => {
    try {
      const resp = await fetch("/api/auth/magic-link/verify", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ email, code }),
      });
      const data = await resp.json();
      if (data.ok) {
        return {
          userHash: data.user_hash,
          sessionToken: data.session_token,
          apiKey: data.api_key,
          isNewUser: data.is_new_user,
        };
      }
      return rejectWithValue(data.error || "Verification failed");
    } catch (err) {
      return rejectWithValue(
        err instanceof Error ? err.message : "Network error",
      );
    }
  },
);

const authSlice = createSlice({
  name: "auth",
  initialState,
  reducers: {
    clearAuthentication: (state) => {
      state.isAuthenticated = false;
      state.publicKeyId = null;
      state.error = null;
    },
    setError: (state, action: PayloadAction<string>) => {
      state.error = action.payload;
    },
    clearError: (state) => {
      state.error = null;
    },
    updateSystemKey: (
      state,
      action: PayloadAction<{ systemPublicKey: string; systemKeyId: string }>,
    ) => {
      state.systemPublicKey = action.payload.systemPublicKey;
      state.systemKeyId = action.payload.systemKeyId;
      state.error = null;
    },
    logoutUser: (state) => {
      state.isAuthenticated = false;
      state.user = undefined;
      state.error = null;
    },
    // Restore session from local storage
    restoreSession: (
      state,
      action: PayloadAction<{ id: string; hash: string }>,
    ) => {
      state.isAuthenticated = true;
      state.user = action.payload;
      state.error = null;
    },
  },
  extraReducers: (builder) => {
    builder
      // autoLogin cases
      .addCase(autoLogin.pending, (state) => {
        state.isLoading = true;
        state.error = null;
      })
      .addCase(autoLogin.fulfilled, (state, action) => {
        state.isLoading = false;
        state.isAuthenticated = true;
        state.user = action.payload;
        state.error = null;
      })
      .addCase(autoLogin.rejected, (state, action) => {
        state.isLoading = false;
        state.error = (action.payload as string) || "Auto-login failed";
      })
      // startMagicLink cases
      .addCase(startMagicLink.pending, (state) => {
        state.magicLinkPending = true;
        state.error = null;
      })
      .addCase(startMagicLink.fulfilled, (state, action) => {
        state.magicLinkPending = true;
        state.email = action.payload.email;
        state.error = null;
      })
      .addCase(startMagicLink.rejected, (state, action) => {
        state.magicLinkPending = false;
        state.error = action.payload as string;
      })
      // verifyMagicLink cases
      .addCase(verifyMagicLink.pending, (state) => {
        state.isLoading = true;
        state.error = null;
      })
      .addCase(verifyMagicLink.fulfilled, (state, action) => {
        state.isLoading = false;
        state.magicLinkPending = false;
        state.isAuthenticated = true;
        state.sessionToken = action.payload.sessionToken;
        state.apiKey = action.payload.apiKey;
        state.user = {
          id: state.email || "",
          hash: action.payload.userHash,
        };
        state.error = null;
      })
      .addCase(verifyMagicLink.rejected, (state, action) => {
        state.isLoading = false;
        state.error = action.payload as string;
      })
      // loadSystemPublicKey cases
      .addCase(loadSystemPublicKey.fulfilled, (state, action) => {
        state.systemPublicKey = action.payload.systemPublicKey;
        state.systemKeyId = action.payload.systemKeyId;
      });
  },
});

export const {
  clearAuthentication,
  setError,
  clearError,
  updateSystemKey,
  logoutUser,
  restoreSession,
} = authSlice.actions;

export default authSlice.reducer;
