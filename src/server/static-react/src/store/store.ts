// @ts-nocheck — pre-existing strict-mode debt; remove this directive after fixing.
import { configureStore } from "@reduxjs/toolkit";
import authReducer from "./authSlice";
import schemaReducer from "./schemaSlice";
import aiQueryReducer from "./aiQuerySlice";
import ingestionReducer from "./ingestionSlice";

export const store = configureStore({
  reducer: {
    auth: authReducer,
    schemas: schemaReducer,
    aiQuery: aiQueryReducer,
    ingestion: ingestionReducer,
  },
  middleware: (getDefaultMiddleware) =>
    getDefaultMiddleware({
      serializableCheck: {
        // Ignore these action types in serializability checks
        ignoredActions: [
          // Schema async thunk actions that may contain non-serializable data
          "schemas/fetchSchemas/fulfilled",
          "schemas/approveSchema/fulfilled",
          "schemas/blockSchema/fulfilled",
          "schemas/unloadSchema/fulfilled",
          "schemas/loadSchema/fulfilled",
        ],
        // Ignore these field paths in all actions
        ignoredActionsPaths: [
          "payload.schemas.definition",
        ],
        // Ignore these paths in the state
        ignoredPaths: ["schemas.schemas.*.definition"],
      },
    }),
  devTools: import.meta.env.DEV,
});

export type RootState = ReturnType<typeof store.getState>;
export type AppDispatch = typeof store.dispatch;