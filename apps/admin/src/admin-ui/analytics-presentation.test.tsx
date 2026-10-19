import { createElement, type ReactElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { I18nProvider } from "@lingui/react";
import { describe, expect, it } from "vite-plus/test";

import { i18n } from "../i18n";
import { AnalyticsPlot, AnalyticsCounts, formatMeasurement } from "./analytics-presentation";

const render = (element: ReactElement) =>
  renderToStaticMarkup(createElement(I18nProvider, { i18n }, element));

describe("analytics presentation", () => {
  // Value: protects=measurement displays distinguish zero from missing/non-finite values;
  // fails_when=missing measurements are coerced to zero or NaN/Infinity are displayed;
  // why_new=chart option tests do not exercise the stat/table display function;
  // seam=none; no_claim=display values only, not backend measurement correctness
  it("keeps zero measurements numeric and missing measurements unavailable", () => {
    expect(formatMeasurement(0, " ms")).toBe("0 ms");
    expect(formatMeasurement(25, " ms")).toBe("25 ms");
    for (const value of [null, undefined, Number.NaN, Number.POSITIVE_INFINITY]) {
      expect(formatMeasurement(value, " ms")).toBe("—");
    }
  });

  // Value: protects=empty plot tables say No data while zero/null rows retain their distinct cells;
  // fails_when=empty tables lose guidance or populated measurements receive an empty-state row;
  // why_new=AnalyticsChart SSR tests do not render the surrounding data table;
  // seam=none; no_claim=real component SSR, not mounted chart effects or requests
  it("renders empty plot guidance without hiding zero and null table values", () => {
    const props: Parameters<typeof AnalyticsPlot>[0] = {
      title: "Queue wait",
      description: "Successful measurements",
      labels: [],
      series: [{ name: "P50", values: [] }],
    };
    const empty = render(createElement(AnalyticsPlot, props));
    expect(empty).toContain("No data");
    expect(empty).toMatch(/colSpan="2"/i);
    expect(empty).not.toContain('role="img"');
    const populated = render(
      createElement(AnalyticsPlot, {
        ...props,
        labels: ["zero", "missing"],
        series: [{ name: "P50", values: [0, null] }],
      }),
    );
    expect(populated).not.toContain("No data");
    expect(populated).toContain(">0</td>");
    expect(populated).toContain(">—</td>");
    expect(populated).toContain('role="img"');
  });

  // Value: protects=empty classification/runtime tables differ from explicit zero counters;
  // fails_when=zero-count rows are suppressed or empty tables silently lack a result row;
  // why_new=no existing tests render the shared counter table;
  // seam=none; no_claim=counter presentation only, not runtime counter collection
  it("shows no-data guidance only when counter rows are absent", () => {
    const empty = render(createElement(AnalyticsCounts, { rows: [] }));
    expect(empty).toContain("No data");
    expect(empty).toMatch(/colSpan="2"/i);
    const populated = render(
      createElement(AnalyticsCounts, { rows: [{ name: "success", count: 0 }] }),
    );
    expect(populated).toContain("success");
    expect(populated).toContain(">0</td>");
    expect(populated).not.toContain("No data");
  });
});
