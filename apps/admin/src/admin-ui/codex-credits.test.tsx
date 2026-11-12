import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { I18nProvider } from "@lingui/react";
import { describe, expect, it } from "vite-plus/test";

import { i18n } from "../i18n";
import type { ProviderAccount } from "../types";
import { CodexCreditsControl } from "./codex-credits";

const account: ProviderAccount = {
  id: "synthetic-account",
  name: "Synthetic Codex",
  provider: "codex-subscription",
  baseUrl: "https://example.test/backend-api",
  authMode: "codex-manual-refresh",
  wireApi: "openai-responses",
  isActive: true,
  allowCredits: false,
  maxRunningRequests: 0,
  status: "quota_exhausted",
  createdAt: "2026-11-12T00:00:00Z",
};

function render(values: Partial<ProviderAccount> = {}, creditsUsable = true) {
  return renderToStaticMarkup(
    createElement(
      I18nProvider,
      { i18n },
      createElement(CodexCreditsControl, {
        account: { ...account, ...values },
        creditsUsable,
        onChange: async () => undefined,
      }),
    ),
  );
}

describe("Codex credits continuation", () => {
  it("defaults off and explains persistent permission and billing boundaries", () => {
    const markup = render();
    expect(markup).toContain("Credits continuation");
    expect(markup).toContain("Off");
    expect(markup).toContain("Permission stays on until you disable it");
    expect(markup).toContain("not a spending cap");
    expect(markup).toContain("Allow credits continuation");
    expect(markup).not.toContain('disabled=""');
  });

  it.each([{ isActive: false }, { status: "blocked" }])(
    "does not offer activation for an unavailable account %j",
    (values) => {
      expect(render(values)).toContain('disabled=""');
    },
  );

  it("keeps enable disabled without a current usable-credit observation", () => {
    expect(render({}, false)).toContain('disabled=""');
  });

  it("always allows revocation, including when quota or authentication is unavailable", () => {
    const markup = render({ allowCredits: true, isActive: false, status: "blocked" }, false);
    expect(markup).toContain("Allowed");
    expect(markup).toContain("Disable credits continuation");
    expect(markup).not.toContain('disabled=""');
  });
});
