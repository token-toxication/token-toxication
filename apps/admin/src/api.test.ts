import { describe, expect, it } from "vite-plus/test";

import { AdminApi } from "./generated/token-toxication";
import { api } from "./api";

describe("admin API response handling", () => {
  it("accepts successful Codex status responses with a null error", async () => {
    const response = {
      account: null,
      error: null,
      expiresAt: "2026-10-10T00:00:00Z",
      flowId: "flow-1",
      nextPollAt: "2026-10-10T00:00:05Z",
      status: "pending",
    };
    const original = Object.getOwnPropertyDescriptor(
      AdminApi.prototype,
      "getCodexDeviceOauthStatus",
    );
    if (!original) {
      throw new Error("expected AdminApi prototype method");
    }
    Object.defineProperty(AdminApi.prototype, "getCodexDeviceOauthStatus", {
      ...original,
      value: async () => response,
    });

    try {
      await expect(api.codexDeviceOAuthStatus("flow-1")).resolves.toEqual(response);
    } finally {
      Object.defineProperty(AdminApi.prototype, "getCodexDeviceOauthStatus", original);
    }
  });
});
