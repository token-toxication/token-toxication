import { describe, expect, it } from "vite-plus/test";
import { init } from "echarts/core";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { I18nProvider } from "@lingui/react";
import { i18n } from "../i18n";
import type { AnalyticsResponse, AnalyticsStats } from "../types";
import {
  createAnalyticsLoader,
  ratio,
  selectionKey,
  type AnalyticsSnapshot,
} from "./analytics-loader";
import { AnalyticsChart, chartOption } from "./analytics-chart";

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
    expect(ratio(0, 100)).toBe(0);
    expect(ratio(80, 100)).toBe(0.8);
    expect(selectionKey({ range: "1h", rankBy: "attempts" })).not.toBe(
      selectionKey({ range: "24h", rankBy: "attempts" }),
    );
  });
});

it("uses non-HTML tooltips and does not connect missing measurements", () => {
  const option = chartOption(
    ["<img onerror=alert(1)>", "next"],
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

it("preserves numeric zero while treating absent and non-finite samples as gaps", () => {
  const input = [0, null, Number.NaN, Number.POSITIVE_INFINITY, 5];
  const option = chartOption(
    ["zero", "missing", "invalid", "infinite", "value", "absent"],
    [{ name: "queue wait", values: input }],
    ["#555"],
    "#666",
    "#ddd",
  );
  expect(option).toMatchObject({
    series: [{ data: [0, null, null, null, 5, null], connectNulls: false }],
  });
  expect(input).toEqual([0, null, Number.NaN, Number.POSITIVE_INFINITY, 5]);
});

it("shows No data only for absent rows, not zero or all-null time series", () => {
  const render = (labels: string[], values: (number | null)[]) =>
    renderToStaticMarkup(
      createElement(
        I18nProvider,
        { i18n },
        createElement(AnalyticsChart, {
          title: "Queue wait",
          labels,
          series: [{ name: "P50", values }],
        }),
      ),
    );
  expect(render([], [])).toContain("No data");
  expect(render([], [])).not.toContain('role="img"');
  for (const values of [
    [0, 0],
    [null, null],
  ]) {
    const markup = render(["first", "second"], values);
    expect(markup).not.toContain("No data");
    expect(markup).toContain('role="img"');
    expect(markup).toContain('aria-label="Queue wait"');
  }
});

it("uses the selected time window for sparse and all-null series without adding observations", () => {
  const timeAxis = { from: 1_000, to: 5_000, timestamps: [2_000, 3_000], locale: "en" };
  const options = (values: (number | null)[]) =>
    chartOption(["first", "second"], [{ name: "queue", values }], ["#555"], "#666", "#ddd", {
      timeAxis,
      description: "Queue wait",
    });
  expect(options([null, 0])).toMatchObject({
    xAxis: { type: "time", min: 1_000, max: 5_000 },
    yAxis: { show: true, min: 0, max: 1 },
    series: [
      {
        data: [
          [2_000, null],
          [3_000, 0],
        ],
        connectNulls: false,
      },
    ],
  });
  expect(options([null, null])).toMatchObject({
    aria: { label: { description: "Queue wait" } },
    xAxis: { type: "time", min: 1_000, max: 5_000 },
    yAxis: { show: false },
    series: [
      {
        data: [
          [2_000, null],
          [3_000, null],
        ],
      },
    ],
  });
});

it("rejects invalid timestamps rather than plotting measurements at epoch zero", () => {
  const option = chartOption(
    ["invalid", "valid"],
    [{ name: "queue", values: [50, 0] }],
    ["#555"],
    "#666",
    "#ddd",
    { timeAxis: { from: 1_000, to: 5_000, timestamps: [Number.NaN, 3_000] } },
  );
  expect(option).toMatchObject({ series: [{ data: [[3_000, 0]] }] });
});

it("omits missing tooltip rows but preserves zeros, names, and non-HTML rendering", () => {
  const option = chartOption([], [], ["#555"], "#666", "#ddd");
  const tooltip = option.tooltip as { formatter: (params: unknown) => string };
  const output = tooltip.formatter([
    { seriesName: "missing", value: null },
    { seriesName: "invalid", value: Number.NaN },
    { seriesName: "queue", value: [2_000, 0], axisValueLabel: "first" },
    { seriesName: "{style|<img onerror=alert(1)>}", value: 5 },
  ]);
  expect(output).toContain("queue: 0");
  expect(output).toContain("first");
  expect(output).toContain("<img onerror=alert(1)>");
  expect(output).not.toContain("{style|");
  expect(output).not.toContain("missing");
  expect(output).not.toContain("invalid");
  expect(tooltip.formatter([{ seriesName: "queue", value: null }])).toBe("");
  const partial = chartOption(
    ["last (in progress)"],
    [{ name: "cache", values: [0.0001] }],
    ["#555"],
    "#666",
    "#ddd",
    { timeAxis: { from: 1_000, to: 5_000, timestamps: [3_000] } },
  ).tooltip as { formatter: (params: unknown) => string };
  const partialText = partial.formatter([
    { seriesName: "cache", value: [3_000, 0.0001], dataIndex: 0, axisValueLabel: "last" },
  ]);
  expect(partialText).toContain("last (in progress)");
  expect(partialText).toContain("0.0001");
  expect(tooltip.formatter([{ seriesName: "tiny", value: 1e-10 }])).toMatch(/1E-10/i);
  expect(tooltip.formatter([{ seriesName: "smallest", value: Number.MIN_VALUE }])).toMatch(
    /5E-324/i,
  );
  expect(tooltip.formatter([{ seriesName: "precise", value: 123.12345678901235 }])).toContain(
    "123.12345678901235",
  );
  expect(option).toMatchObject({
    tooltip: { renderMode: "richText", axisPointer: { snap: true } },
  });
});

// Value: protects=stacked time bars use the exact selected window and zero baseline;
// fails_when=ECharts expands the mapping to contain bar shapes outside the window;
// why_new=option min/max checks do not inspect the engine's pixel-to-data mapping;
// seam=none; no_claim=real ECharts SSR geometry, not a running Grafana comparison
it("keeps stacked time bars inside the exact response window", () => {
  const chart = init(null, undefined, { renderer: "svg", ssr: true, width: 840, height: 300 });
  try {
    chart.setOption(
      chartOption(
        ["first", "last"],
        [
          { name: "success", type: "bar", stack: "outcomes", values: [0, 1] },
          { name: "failure", type: "bar", stack: "outcomes", values: [1, 0] },
        ],
        ["#555"],
        "#666",
        "#ddd",
        {
          timeAxis: { from: 1_000, to: 5_000, timestamps: [2_000, 3_000] },
        },
      ),
    );
    // Fix fixture geometry independently of automatic tick-label space reservation.
    chart.setOption({ grid: { outerBoundsMode: "none" } });
    expect(chart.convertFromPixel({ xAxisIndex: 0 }, 55)).toBeCloseTo(1_000);
    expect(chart.convertFromPixel({ xAxisIndex: 0 }, 840 - 18)).toBeCloseTo(5_000);
    expect(chart.getOption()).toMatchObject({ yAxis: [{ scale: false }] });
  } finally {
    chart.dispose();
  }
});

it("clears zero-domain bounds on refresh without losing legend selections", () => {
  const chart = init(null, undefined, { renderer: "svg", ssr: true, width: 840, height: 300 });
  const options = (values: (number | null)[]) =>
    chartOption(["first", "second"], [{ name: "queue", values }], ["#555"], "#666", "#ddd");
  try {
    chart.setOption(options([0, 0]));
    chart.dispatchAction({ type: "legendUnSelect", name: "queue" });
    chart.setOption(options([5, 10]), { replaceMerge: ["series", "xAxis", "yAxis"] });
    expect(chart.getOption()).toMatchObject({ legend: [{ selected: { queue: false } }] });
    const axes = chart.getOption().yAxis as { min?: number; max?: number }[];
    expect(axes[0].min).toBeUndefined();
    expect(axes[0].max).toBeUndefined();
    chart.setOption(options([null, null]), { replaceMerge: ["series", "xAxis", "yAxis"] });
    expect(chart.getOption()).toMatchObject({ yAxis: [{ show: false }] });
  } finally {
    chart.dispose();
  }
});

it("uses automatic points for dense lines while retaining gap-isolated zero observations", () => {
  const values: (number | null)[] = Array.from({ length: 288 }, () => 10);
  values[136] = null;
  values[137] = 0;
  values[138] = null;
  const option = chartOption(
    values.map((_, i) => `interval-${i}`),
    [{ name: "queue", values }],
    ["#555"],
    "#666",
    "#ddd",
    { width: 600 },
  );
  const series = option.series as {
    symbolSize: (_: unknown, point: { dataIndex: number }) => number;
  }[];
  expect(series[0].symbolSize(10, { dataIndex: 50 })).toBe(0);
  expect(series[0].symbolSize(0, { dataIndex: 137 })).toBeGreaterThan(0);
  expect(option).toMatchObject({ series: [{ showAllSymbol: true, connectNulls: false }] });
});

it.each([0, 10])(
  "renders an isolated %s measurement even when category labels are thinned",
  (value) => {
    const chart = init(null, undefined, { renderer: "svg", ssr: true, width: 840, height: 300 });
    try {
      const labels = Array.from({ length: 288 }, (_, i) => `interval-${i}`);
      const values = labels.map((_, i) => (i === 137 ? value : null));
      chart.setOption(chartOption(labels, [{ name: "latency", values }], ["#555"], "#666", "#ddd"));
      const svg = chart.renderToSVGString();
      expect(svg).toContain('ecmeta_data_index="137"');
      expect(svg).not.toContain('ecmeta_data_index="136"');
      expect(svg).not.toContain('ecmeta_data_index="138"');
    } finally {
      chart.dispose();
    }
  },
);
