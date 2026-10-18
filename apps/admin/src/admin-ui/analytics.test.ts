import { describe, expect, it } from "vite-plus/test";
import { init } from "echarts/core";
import type { AnalyticsResponse, AnalyticsStats } from "../types";
import {
  createAnalyticsLoader,
  ratio,
  selectionKey,
  type AnalyticsSnapshot,
} from "./analytics-loader";
import { chartOption } from "./analytics-chart";

function stats(): AnalyticsStats {
  const empty = { samples: 0, p50: null, p95: null };
  return {
    attempts: 0,
    success: 0,
    failures: 0,
    clientDisconnect: 0,
    serverShutdown: 0,
    unknown: 0,
    inputTokens: 0,
    outputTokens: 0,
    classifiedInputTokens: 0,
    cachedInputTokens: 0,
    cacheSamples: 0,
    completeUsageSamples: 0,
    retryAttempts: 0,
    overloadRetries: 0,
    ttft: empty,
    latency: empty,
    queue: empty,
    otps: empty,
  };
}

function fixture(): AnalyticsResponse {
  return {
    history: {
      range: "24h",
      rankBy: "attempts",
      model: null,
      accountId: null,
      windowStartedAt: "2026-10-18T00:00:00Z",
      windowEndedAt: "2026-10-19T00:00:00Z",
      generatedAt: "2026-10-19T00:00:00Z",
      bucketDurationSeconds: 300,
      summary: stats(),
      buckets: [],
      accounts: [],
      models: [],
      outcomes: [],
      transports: [],
      statuses: [],
    },
    runtime: {
      startedAt: "2026-10-19T00:00:00Z",
      snapshotAt: "2026-10-19T00:00:00Z",
      requestOutcomes: [],
      routeSelections: [],
      sessionAffinity: [],
      selectionFailures: [],
      overloadRetries: 0,
    },
  };
}

function deferred<T>() {
  let resolve: (value: T) => void = () => {};
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

describe("analytics query isolation", () => {
  it("retains only the same query's snapshot after a refresh failure", async () => {
    const updates: AnalyticsSnapshot[] = [];
    let fail = false;
    const data = fixture();
    const loader = createAnalyticsLoader(
      { range: "24h", rankBy: "attempts" },
      async () => {
        if (fail) throw new Error("offline");
        return data;
      },
      (s) => updates.push(s),
    );
    await loader.refresh();
    fail = true;
    await loader.refresh();
    expect(updates.at(-1)).toMatchObject({ data, error: true, pending: false });
    loader.dispose();
  });

  it("aborts old contexts and ignores late responses even if fetch ignores abort", async () => {
    const old = deferred<AnalyticsResponse>();
    const updates: AnalyticsSnapshot[] = [];
    let signal: AbortSignal | undefined;
    const loader = createAnalyticsLoader(
      { range: "24h", model: "a", rankBy: "attempts" },
      (_, s) => {
        signal = s;
        return old.promise;
      },
      (s) => updates.push(s),
    );
    const pending = loader.refresh();
    loader.dispose();
    expect(signal?.aborted).toBe(true);
    const next = createAnalyticsLoader(
      { range: "24h", model: "b", rankBy: "attempts" },
      async () => {
        throw new Error("offline");
      },
      (s) => updates.push(s),
    );
    await next.refresh();
    old.resolve(fixture());
    await pending;
    expect(updates.at(-1)).toMatchObject({
      key: selectionKey({ range: "24h", model: "b", rankBy: "attempts" }),
      data: null,
      error: true,
    });
    next.dispose();
  });

  it("coalesces concurrent refresh calls", async () => {
    const result = deferred<AnalyticsResponse>();
    let calls = 0;
    const loader = createAnalyticsLoader(
      { range: "1h", rankBy: "attempts" },
      () => {
        calls++;
        return result.promise;
      },
      () => {},
    );
    const first = loader.refresh();
    await loader.refresh();
    expect(calls).toBe(1);
    result.resolve(fixture());
    await first;
    loader.dispose();
  });

  it("does not invent percentages for zero denominators", () => {
    expect(ratio(0, 0)).toBeNull();
    expect(ratio(80, 100)).toBe(0.8);
    expect(selectionKey({ range: "1h", rankBy: "attempts" })).not.toBe(
      selectionKey({ range: "24h", rankBy: "attempts" }),
    );
  });
});

it("uses non-HTML tooltips and does not connect missing measurements", () => {
  const option = chartOption(
    ["<img onerror=alert(1)>"],
    [{ name: "latency", values: [null, 10] }],
    ["#555"],
    "#666",
    "#ddd",
  );
  expect(option).toMatchObject({
    animation: false,
    tooltip: { renderMode: "richText", confine: true },
    series: [{ connectNulls: false, smooth: false, data: [null, 10] }],
  });
});

it("renders isolated measurements even when the category label is thinned", () => {
  const chart = init(null, undefined, { renderer: "svg", ssr: true, width: 840, height: 300 });
  try {
    const labels = Array.from({ length: 288 }, (_, i) => `interval-${i}`);
    const values = labels.map((_, i) => (i === 137 ? 10 : null));
    chart.setOption(chartOption(labels, [{ name: "latency", values }], ["#555"], "#666", "#ddd"));
    const svg = chart.renderToSVGString();
    expect(svg).toContain('ecmeta_data_index="137"');
    expect(svg).not.toContain('ecmeta_data_index="136"');
    expect(svg).not.toContain('ecmeta_data_index="138"');
  } finally {
    chart.dispose();
  }
});
