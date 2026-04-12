/**
 * Constants Integration Tests
 */

import {
  BROWSER_CONFIG,
  SCHEMA_STATES,
  RANGE_SCHEMA_CONFIG,
  DEFAULT_TAB,
} from "../index";

describe("Constants Integration Tests", () => {
  describe("Configuration Constants", () => {
    test("BROWSER_CONFIG should contain storage keys", () => {
      expect(BROWSER_CONFIG).toBeDefined();
      expect(BROWSER_CONFIG.STORAGE_KEYS.USER_ID).toBe("fold_user_id");
      expect(BROWSER_CONFIG.STORAGE_KEYS.USER_HASH).toBe("fold_user_hash");
    });

    test("DEFAULT_TAB should be exported directly", () => {
      expect(DEFAULT_TAB).toBe("agent");
    });
  });

  describe("Schema Constants", () => {
    test("SCHEMA_STATES should contain all valid states", () => {
      expect(SCHEMA_STATES).toBeDefined();
      expect(SCHEMA_STATES.APPROVED).toBe("approved");
      expect(SCHEMA_STATES.AVAILABLE).toBe("available");
      expect(SCHEMA_STATES.BLOCKED).toBe("blocked");
    });

    test("RANGE_SCHEMA_CONFIG should have expected values", () => {
      expect(RANGE_SCHEMA_CONFIG).toBeDefined();
      expect(RANGE_SCHEMA_CONFIG.FIELD_TYPE).toBe("Range");
      expect(RANGE_SCHEMA_CONFIG.MUTATION_WRAPPER_KEY).toBe("value");
    });
  });

  describe("Type Safety", () => {
    test("Constants should have consistent types", () => {
      expect(typeof DEFAULT_TAB).toBe("string");
      expect(typeof SCHEMA_STATES.APPROVED).toBe("string");
    });
  });
});
