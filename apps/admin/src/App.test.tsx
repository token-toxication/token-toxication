import { PassThrough } from "node:stream";
import { createElement } from "react";
import { renderToPipeableStream, renderToStaticMarkup } from "react-dom/server";
import { MemoryRouter } from "react-router";
import { I18nProvider } from "@lingui/react";
import { beforeEach, describe, expect, it, vi } from "vite-plus/test";

import App from "./App";
import { i18n } from "./i18n";
import type { Dashboard } from "./types";
import type { useAdminWorkspace } from "./admin-ui/use-admin-workspace";

const workspace = vi.hoisted(() => {
  const state: Pick<ReturnType<typeof useAdminWorkspace>, "token" | "dashboard" | "isLoading"> = {
    token: "test",
    dashboard: null,
    isLoading: false,
  };
  return state;
});

vi.mock("./admin-ui/use-admin-workspace", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./admin-ui/use-admin-workspace")>();
  return {
    ...actual,
    useAdminWorkspace: () => ({ ...actual.useAdminWorkspace(), ...workspace }),
  };
});

vi.mock("./api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./api")>();
  return { ...actual, getStoredToken: () => null };
});

const emptyDashboard: Dashboard = {
  accounts: [],
  activeApiKeys: 0,
  healthyAccounts: 0,
  recentRequests: [],
  totalAccounts: 0,
  totalApiKeys: 0,
  requestTrend: {
    bucketDurationSeconds: 60,
    buckets: [],
    windowStartedAt: "2026-10-19T00:00:00Z",
    windowEndedAt: "2026-10-19T01:00:00Z",
  },
  usage: {
    cachedInputTokensToday: 0,
    estimatedCostToday: 0,
    inputTokensToday: 0,
    requestsToday: 0,
    tokensToday: 0,
    totalRequests: 0,
    totalTokens: 0,
  },
};

function appAt(pathname: string) {
  return createElement(
    I18nProvider,
    { i18n },
    createElement(MemoryRouter, { initialEntries: [pathname] }, createElement(App)),
  );
}

function renderReadyApp(pathname: string): Promise<string> {
  return new Promise((resolve, reject) => {
    const output = new PassThrough();
    const chunks: Buffer[] = [];
    output.on("data", (chunk: Buffer) => chunks.push(chunk));
    output.on("end", () => resolve(Buffer.concat(chunks).toString("utf8")));
    output.on("error", reject);
    const stream = renderToPipeableStream(appAt(pathname), {
      onAllReady() {
        stream.pipe(output);
      },
      onError(error) {
        reject(error);
        stream.abort();
      },
    });
  });
}

describe("App metrics and overview pages", () => {
  beforeEach(() => {
    workspace.dashboard = null;
    workspace.isLoading = false;
  });

  // Value: protects=Metrics renders real analytics independently of the Overview dashboard;
  // fails_when=the metrics route is missing, shows Overview, or requires dashboard data;
  // why_new=route metadata tests do not render App or its lazy analytics page;
  // seam=none;
  // no_claim=SSR only; token and dashboard state are fixtures, effects do not run; no auth, API, chart, or browser proof
  it("renders analytics at its own active navigation entry without dashboard data", async () => {
    const markup = await renderReadyApp("/analytics");

    expect(markup).toMatch(/<a\b[^>]*aria-current="page"[^>]*href="\/analytics"/);
    expect(markup).toContain('aria-label="Relay analytics"');
    expect(markup).toContain('aria-label="Time range"');
    expect(markup).toContain('aria-label="Refresh analytics"');
    expect(markup).toContain("Completed upstream attempts, not user requests or live concurrency.");
    expect(markup).not.toContain("Provider pool");
    expect(markup).not.toContain("Recent requests");
    expect(markup).not.toContain("Dashboard unavailable");
    expect(markup).not.toContain("Page not found");
  });

  // Value: protects=App routes Overview to operational cards rather than analytics;
  // fails_when=App maps the root route to Metrics or renders analytics alongside Overview;
  // why_new=direct Overview and route metadata tests cannot catch App element wiring errors;
  // seam=none;
  // no_claim=SSR only; workspace token and dashboard are fixtures; no browser navigation, authentication, or network proof
  it("keeps the root page focused on the provider pool and recent requests", () => {
    workspace.dashboard = emptyDashboard;
    const markup = renderToStaticMarkup(appAt("/"));

    expect(markup).toMatch(/<a\b[^>]*aria-current="page"[^>]*href="\/"/);
    expect(markup).toContain('href="/analytics"');
    expect(markup).toContain("Provider pool");
    expect(markup).toContain("Recent requests");
    expect(markup).not.toContain('aria-label="Relay analytics"');
    expect(markup).not.toContain('aria-label="Time range"');
    expect(markup).not.toContain('data-slot="skeleton"');
  });
});
