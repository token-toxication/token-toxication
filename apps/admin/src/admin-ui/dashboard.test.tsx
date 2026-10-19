import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { MemoryRouter } from "react-router";
import { I18nProvider } from "@lingui/react";
import { describe, expect, it, vi } from "vite-plus/test";

import { i18n } from "../i18n";
import type { Dashboard, ProviderAccount, RequestLog } from "../types";
import { Overview } from "./dashboard";

const { analyticsModuleLoaded } = vi.hoisted(() => ({ analyticsModuleLoaded: vi.fn() }));

vi.mock("./analytics-panel", async (importOriginal) => {
  analyticsModuleLoaded();
  return importOriginal<typeof import("./analytics-panel")>();
});

const account: ProviderAccount = {
  id: "account/west asia",
  name: "West Asia provider",
  provider: "openai",
  authMode: "api-key",
  baseUrl: "https://provider.example.test",
  createdAt: "2026-10-19T00:00:00Z",
  isActive: true,
  maxRunningRequests: 0,
  status: "healthy",
  wireApi: "responses",
};

const request: RequestLog = {
  id: "request/recent",
  apiKeyId: "key-1",
  providerAccountId: account.id,
  cachedInputTokens: 0,
  costUsd: 0,
  createdAt: "2026-10-19T00:30:00Z",
  inputTokens: 10,
  outputTokens: 5,
  latencyMs: 200,
  method: "POST",
  model: "operations-model",
  path: "/v1/responses",
  statusCode: 200,
};

const dashboard: Dashboard = {
  accounts: [account],
  activeApiKeys: 2,
  healthyAccounts: 1,
  recentRequests: [request],
  totalAccounts: 1,
  totalApiKeys: 3,
  requestTrend: {
    bucketDurationSeconds: 60,
    buckets: [],
    windowStartedAt: "2026-10-19T00:00:00Z",
    windowEndedAt: "2026-10-19T01:00:00Z",
  },
  usage: {
    cachedInputTokensToday: 0,
    estimatedCostToday: 0,
    inputTokensToday: 10,
    requestsToday: 1,
    tokensToday: 15,
    totalRequests: 1,
    totalTokens: 15,
  },
};

function renderOverview(data: Dashboard) {
  return renderToStaticMarkup(
    createElement(
      I18nProvider,
      { i18n },
      createElement(
        MemoryRouter,
        null,
        createElement(Overview, {
          dashboard: data,
          apiKeys: [],
          accounts: data.accounts,
          onCreateAccount: () => undefined,
          onCreateKey: () => undefined,
        }),
      ),
    ),
  );
}

describe("Overview", () => {
  // Value: protects=Overview keeps provider health and recent request links without loading analytics;
  // fails_when=operational data is dropped or the analytics module and its loading fallback return;
  // why_new=no existing test renders Overview with provider accounts and recent requests;
  // seam=none;
  // no_claim=SSR with typed dashboard fixtures; analytics import is observed but not replaced; no click, API, or browser proof
  it("retains operational data and detail links without loading the analytics module", async () => {
    const markup = renderOverview(dashboard);
    await vi.dynamicImportSettled();

    expect(markup).toContain("Provider pool");
    expect(markup).toContain("1/1 healthy accounts");
    expect(markup).toContain("2/3 active API keys");
    expect(markup).toContain("West Asia provider");
    expect(markup).toContain('href="/accounts/account%2Fwest%20asia"');
    expect(markup).toContain("Recent requests");
    expect(markup).toContain("operations-model");
    expect(markup).toContain('href="/logs/request%2Frecent"');
    expect(markup).not.toContain("No provider accounts");
    expect(markup).not.toContain("No relay traffic");
    expect(markup).not.toContain('aria-label="Relay analytics"');
    expect(markup).not.toContain('data-slot="skeleton"');
    expect(analyticsModuleLoaded).not.toHaveBeenCalled();
  });

  // Value: protects=Empty Overview gives provider and traffic guidance without an analytics loading state;
  // fails_when=empty operations disappear behind analytics, a skeleton, or populated-only rendering;
  // why_new=the populated Overview test cannot cover the two empty collection branches;
  // seam=none;
  // no_claim=SSR with empty typed fixtures; analytics imports are observed; no create-button, network, or browser proof
  it("keeps empty provider and traffic guidance usable without loading analytics", async () => {
    const markup = renderOverview({
      ...dashboard,
      accounts: [],
      recentRequests: [],
      healthyAccounts: 0,
      totalAccounts: 0,
      activeApiKeys: 0,
      totalApiKeys: 0,
    });
    await vi.dynamicImportSettled();

    expect(markup).toContain("0/0 healthy accounts");
    expect(markup).toContain("0/0 active API keys");
    expect(markup).toContain("No provider accounts");
    expect(markup).toContain("Add a provider account to relay traffic.");
    expect(markup).toContain("Recent requests");
    expect(markup).toContain("No relay traffic");
    expect(markup).toContain("Requests appear here after clients call the relay.");
    expect(markup).not.toContain("West Asia provider");
    expect(markup).not.toContain("operations-model");
    expect(markup).not.toContain('aria-label="Relay analytics"');
    expect(markup).not.toContain('data-slot="skeleton"');
    expect(analyticsModuleLoaded).not.toHaveBeenCalled();
  });
});
