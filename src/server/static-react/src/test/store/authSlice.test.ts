import { describe, it, expect } from "vitest";
import {
  isNodeNotProvisionedError,
  NODE_NOT_PROVISIONED_ERROR,
} from "../../store/authSlice";
import { ApiError } from "../../api/core/errors";

// The autoLogin thunk routes the user into the bootstrap wizard iff its
// catch handler classifies the failure as `node_not_provisioned`. The
// detection is a pure function so it can be tested without the redux
// machinery — keep it that way.
describe("authSlice / isNodeNotProvisionedError", () => {
  it("matches via the response body the backend actually sends (the regression case)", () => {
    // Wire shape from `/api/system/auto-identity` 503:
    //   { "error": "node_not_provisioned",
    //     "next": "POST /api/setup/bootstrap" }
    const err = new ApiError("node_not_provisioned", 503, {
      response: {
        error: "node_not_provisioned",
        next: "POST /api/setup/bootstrap",
      },
    });
    expect(isNodeNotProvisionedError(err)).toBe(true);
  });

  it("matches via response.code as well as response.error", () => {
    const err = new ApiError("HTTP 503", 503, {
      response: { code: "node_not_provisioned" },
    });
    expect(isNodeNotProvisionedError(err)).toBe(true);
  });

  it("matches via the ApiError.code field", () => {
    const err = new ApiError("HTTP 503", 503, {
      code: "node_not_provisioned",
    });
    expect(isNodeNotProvisionedError(err)).toBe(true);
  });

  it("matches via nested details.error", () => {
    const err = new ApiError("HTTP 503", 503, {
      details: { error: "node_not_provisioned" },
    });
    expect(isNodeNotProvisionedError(err)).toBe(true);
  });

  it("does NOT match a generic 503 (e.g. service unavailable)", () => {
    const err = new ApiError("Service unavailable", 503, {
      response: { error: "service_unavailable" },
    });
    expect(isNodeNotProvisionedError(err)).toBe(false);
  });

  it("does NOT match the sentinel string on a non-503 (defense in depth — only the 503 contract carries this signal)", () => {
    const err = new ApiError("node_not_provisioned", 500, {
      response: { error: "node_not_provisioned" },
    });
    expect(isNodeNotProvisionedError(err)).toBe(false);
  });

  it("does NOT match plain Error instances (no ApiError shape)", () => {
    expect(isNodeNotProvisionedError(new Error("node_not_provisioned"))).toBe(
      false,
    );
  });

  it("exports the canonical sentinel string used by App.tsx", () => {
    expect(NODE_NOT_PROVISIONED_ERROR).toBe("node_not_provisioned");
  });
});
