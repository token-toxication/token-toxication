import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { I18nProvider } from "@lingui/react";
import { describe, expect, it } from "vite-plus/test";

import { i18n } from "../i18n";
import { CodexDeviceLoginDialogBody } from "./accounts";

describe("Codex device login dialog", () => {
  it("keeps the device code and verification link visible while pending", () => {
    const markup = renderToStaticMarkup(
      createElement(
        I18nProvider,
        { i18n },
        createElement(CodexDeviceLoginDialogBody, {
          login: {
            flowId: "flow-1",
            verificationUrl: "https://auth.openai.com/codex/device",
            userCode: "ABCD-EFGH",
            expiresAt: "2026-10-10T00:00:00Z",
            status: "pending",
          },
          onCancel: () => undefined,
        }),
      ),
    );

    expect(markup).toContain("ABCD-EFGH");
    expect(markup).toContain('href="https://auth.openai.com/codex/device"');
    expect(markup).toContain("Waiting for Codex sign-in to complete");
  });
});
