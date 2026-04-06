/**
 * UI Constants for FoldDB React Application
 * Extracted from hardcoded values per Section 2.1.12
 * Part of TASK-005: Constants Extraction and Configuration Centralization
 *
 * Note: This file contains UI constants that are actively used.
 * These should be gradually migrated to more specific constant files.
 */

// Tab Configuration and Navigation
export const TAB_TRANSITION_DURATION_MS = 200;
export const FORM_FIELD_DEBOUNCE_MS = 300;

// Tab Definitions
// Main tabs: core user-facing features (5 tabs max visible)
// Advanced tabs: developer/power-user tools in "More" dropdown
export const DEFAULT_TABS = [
  // Core experience
  { id: "agent", label: "Agent", icon: "💬", group: "main" },
  { id: "smart-folder", label: "Import", icon: "📂", group: "main" },
  { id: "feed", label: "Feed", icon: "📷", group: "main" },
  { id: "my-profile", label: "My Profile", icon: "🧬", group: "main" },
  // Import methods (under More > Import)
  { id: "file-upload", label: "File Upload", icon: "📄", group: "advanced" },
  { id: "apple-import", label: "Apple Import", icon: "🍎", group: "advanced" },
  { id: "ingestion", label: "JSON Ingestion", icon: "📥", group: "advanced" },
  // Social
  { id: "shared-moments", label: "Shared Moments", icon: "📸", group: "advanced" },
  { id: "sharing", label: "Sharing", icon: "🔗", group: "advanced" },
  // Data tools
  { id: "llm-query", label: "AI Query", icon: "🤖", group: "advanced" },
  { id: "schemas", label: "Schema", icon: "📋", group: "advanced" },
  { id: "query", label: "Query", icon: "🔍", group: "advanced" },
  { id: "mutation", label: "Mutation", icon: "✏️", group: "advanced" },
  { id: "data-browser", label: "Data Browser", icon: "🗄️", group: "advanced" },
  { id: "native-index", label: "Native Index", icon: "🧭", group: "advanced" },
  { id: "word-graph", label: "Word Graph", icon: "🕸️", group: "advanced" },
  { id: "views", label: "Views", icon: "👁️", group: "advanced" },
  // Network
  { id: "discovery", label: "Discovery", icon: "🌐", group: "advanced" },
  { id: "discovery-browse", label: "Browse Network", icon: "🔎", group: "advanced" },
  // Sync
  { id: "conflicts", label: "Sync Conflicts", icon: "⚡", group: "advanced" },
];

// Button Text Constants
export const BUTTON_TEXT = {
  approve: "Approve",
  block: "Block",
  unload: "Unload",
  executeQuery: "Execute Query",
  executeMutation: "Execute Mutation",
  confirm: "Confirm",
  cancel: "Cancel",
};

// Form Label Constants
export const FORM_LABELS = {
  schema: "Schema",
  schemaEmpty: "No schemas available",
  schemaHelp: "Select a schema to work with",
  rangeKeyFilter: "Range Key Filter",
  rangeKeyRequired: "Range key is required",
  rangeKeyOptional: "Range key is optional",
  operationType: "Operation Type",
  operationHelp: "Select the type of operation to perform",
};

// UI State Constants
export const UI_STATES = {
  loading: "Loading...",
  error: "Error",
  success: "Success",
  idle: "Ready",
};

// Mutation Type Constants
export const MUTATION_TYPES = [
  { value: "Insert", label: "Insert" },
  { value: "Update", label: "Update" },
];

// Backend mutation type normalization map
export const MUTATION_TYPE_API_MAP = {
  Insert: "create",
  Create: "create",
  Update: "update",
};

// Schema Badge Colors
export const SCHEMA_BADGE_COLORS = {
  approved: "badge badge-success",
  available: "badge badge-info",
  blocked: "badge badge-error",
  pending: "badge badge-warning",
};

// Authentication Indicators
export const AUTH_INDICATORS = {
  authenticated: "🔐",
  unauthenticated: "🔓",
  loading: "⏳",
};

// Help Text Constants
export const HELP_TEXT = {
  rangeSchema: "Range schemas support filtering by a range key",
  mutation: "Select an operation to perform on the schema",
  query: "Query approved schemas for data",
  schemaStates: {
    approved: "Schema is approved for use in queries and mutations",
    available: "Schema is available but requires approval before use",
    blocked: "Schema is blocked and cannot be used",
    pending: "Schema approval is pending review",
    unknown: "Schema state is unknown or invalid",
  },
};

// Range Schema Configuration
export const RANGE_SCHEMA_CONFIG = {
  FIELD_TYPE: "Range",
  MUTATION_WRAPPER_KEY: "value",
  label: "Range Key",
  backgroundColor: "bg-gruvbox-elevated border border-border p-4",
  badgeColor: "badge badge-info",
  indicator: {
    text: "Range",
    className: "badge badge-info",
  },
  tooltip: "This schema supports range-based queries",
};
