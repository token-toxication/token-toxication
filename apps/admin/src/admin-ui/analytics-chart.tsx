import { useEffect, useRef, useState } from "react";
import { Trans } from "@lingui/react/macro";
import { useTheme } from "next-themes";
import { init, use as registerCharts, type EChartsCoreOption } from "echarts/core";
import { BarChart, LineChart } from "echarts/charts";
import {
  AriaComponent,
  GridComponent,
  LegendComponent,
  TooltipComponent,
} from "echarts/components";
import { SVGRenderer } from "echarts/renderers";

registerCharts([
  BarChart,
  LineChart,
  AriaComponent,
  GridComponent,
  LegendComponent,
  TooltipComponent,
  SVGRenderer,
]);

export type ChartSeries = {
  name: string;
  values: (number | null)[];
  type?: "line" | "bar";
  stack?: string;
  tone?: "failure";
};

export type ChartTimeAxis = {
  timestamps: number[];
  from: number;
  to: number;
  locale?: string;
};

// Prevent field names from being interpreted as ECharts rich-text markup.
const tooltipText = (text: string) => text.replaceAll("{", "{\u200b");

function tooltipContent(params: unknown, labels: string[], timeAxis?: ChartTimeAxis) {
  const items: unknown[] = Array.isArray(params) ? params : [params];
  let heading = "";
  const rows = items.flatMap((item) => {
    if (!item || typeof item !== "object" || !("value" in item)) return [];
    const value = Array.isArray(item.value) ? item.value[1] : item.value;
    if (typeof value !== "number" || !Number.isFinite(value)) return [];
    if (!heading) {
      const index =
        timeAxis && Array.isArray(item.value)
          ? timeAxis.timestamps.indexOf(item.value[0])
          : "dataIndex" in item && typeof item.dataIndex === "number"
            ? item.dataIndex
            : -1;
      const label =
        labels[index] ??
        ("axisValueLabel" in item && typeof item.axisValueLabel === "string"
          ? item.axisValueLabel
          : "");
      heading = tooltipText(label);
    }
    const name = "seriesName" in item ? tooltipText(String(item.seriesName)) : "";
    return [
      `${name}: ${value.toLocaleString(undefined, {
        maximumSignificantDigits: 21,
        notation: value !== 0 && Math.abs(value) < 0.000001 ? "scientific" : "standard",
      })}`,
    ];
  });
  return rows.length ? [heading, ...rows].filter(Boolean).join("\n") : "";
}

export function chartOption(
  labels: string[],
  series: ChartSeries[],
  colors: string[],
  foreground: string,
  border: string,
  options: {
    horizontal?: boolean;
    danger?: string;
    timeAxis?: ChartTimeAxis;
    width?: number;
    description?: string;
  } = {},
): EChartsCoreOption {
  const { horizontal = false, danger = foreground, timeAxis } = options;
  const normalized = series.map((s) => ({
    ...s,
    rows: labels
      .map((_, index) => {
        const value = s.values[index];
        return {
          time: timeAxis?.timestamps[index],
          value: typeof value === "number" && Number.isFinite(value) ? value : null,
        };
      })
      .filter((row) => !timeAxis || (typeof row.time === "number" && Number.isFinite(row.time))),
  }));
  const measurements = normalized.flatMap((s) => s.rows.flatMap((row) => row.value ?? []));
  const hasMeasurements = measurements.length > 0;
  const allZero = hasMeasurements && measurements.every((value) => value === 0);
  const hasBars = series.some((s) => s.type === "bar");
  const category = {
    type: "category",
    data: labels,
    axisTick: { show: false },
    axisLine: { show: false },
    axisLabel: {
      color: foreground,
      hideOverlap: true,
      width: horizontal ? 120 : undefined,
      overflow: "truncate",
    },
  };
  const value = {
    type: "value",
    // Grafana disables an unfixed numeric axis with no observations.
    show: hasMeasurements,
    scale: !hasBars,
    boundaryGap: hasBars ? [0, "10%"] : ["10%", "10%"],
    // Domain fallback only: these bounds never add samples or center zero.
    ...(allZero ? { min: 0, max: 1 } : {}),
    axisLabel: {
      color: foreground,
      formatter: (n: number) => Intl.NumberFormat(undefined, { notation: "compact" }).format(n),
    },
    splitLine: { lineStyle: { color: border, type: "dashed" } },
  };
  const time = timeAxis
    ? {
        type: "time",
        min: timeAxis.from,
        max: timeAxis.to,
        boundaryGap: false,
        containShape: false,
        axisTick: { show: false },
        axisLine: { show: false },
        axisLabel: {
          color: foreground,
          hideOverlap: true,
          formatter: (timestamp: number) =>
            new Date(timestamp).toLocaleString(timeAxis.locale, {
              month: "short",
              day: "numeric",
              hour: "2-digit",
              minute: "2-digit",
            }),
        },
      }
    : category;
  return {
    animation: false,
    aria: {
      enabled: true,
      decal: { show: true },
      // The adjacent data table exposes values without ECharts reading nulls as NaN.
      label: { description: options.description },
    },
    color: colors,
    textStyle: { fontFamily: "Geist Variable, sans-serif" },
    grid: { top: 14, left: horizontal ? 136 : 55, right: 18, bottom: 76 },
    legend: { bottom: 0, textStyle: { color: foreground }, type: "scroll" },
    // Rich text is drawn by ECharts, never inserted as HTML. Names are untrusted.
    tooltip: {
      trigger: "axis",
      renderMode: "richText",
      confine: true,
      axisPointer: { snap: true },
      formatter: (params: unknown) => tooltipContent(params, labels, timeAxis),
    },
    xAxis: horizontal ? value : time,
    yAxis: horizontal ? { ...category, inverse: true } : value,
    series: normalized.map((s, index) => {
      const samples = s.rows.filter((row) => row.value != null).length;
      const sparse = samples <= Math.max(1, Math.floor((options.width ?? 600) / 12));
      return {
        name: s.name,
        type: s.type ?? "line",
        stack: s.stack,
        data: s.rows.map((row) => (timeAxis ? [row.time, row.value] : row.value)),
        connectNulls: false,
        showSymbol: true,
        showAllSymbol: true,
        // Auto points: sparse series get markers; dense lines retain gap-isolated points.
        symbolSize: (_: unknown, point: { dataIndex: number }) => {
          const i = point.dataIndex;
          const isolated = s.rows[i - 1]?.value == null && s.rows[i + 1]?.value == null;
          return sparse || isolated ? 5 : 0;
        },
        smooth: false,
        lineStyle: {
          width: 2,
          type: index % 2 ? "dashed" : "solid",
          ...(s.tone === "failure" ? { color: danger } : {}),
        },
        ...(s.tone === "failure" ? { itemStyle: { color: danger } } : {}),
        emphasis: { focus: "series" },
        barMaxWidth: 30,
      };
    }),
  };
}

function tokenColor(element: HTMLElement, token: string) {
  const value = getComputedStyle(element).getPropertyValue(token).trim();
  const context = document.createElement("canvas").getContext("2d");
  if (!context) return value;
  context.fillStyle = value;
  context.fillRect(0, 0, 1, 1);
  const [r, g, b] = context.getImageData(0, 0, 1, 1).data;
  return `rgb(${r}, ${g}, ${b})`;
}

type AnalyticsChartProps = {
  labels: string[];
  series: ChartSeries[];
  title: string;
  horizontal?: boolean;
  timeAxis?: ChartTimeAxis;
};

export function AnalyticsChart(props: AnalyticsChartProps) {
  // All-null rows still describe a time range. Only a result without rows is empty.
  if (
    props.labels.length === 0 ||
    props.series.length === 0 ||
    props.series.every((series) => series.values.length === 0)
  ) {
    return (
      <div
        className="flex h-72 items-center justify-center text-sm text-muted-foreground"
        role="status"
        aria-label={props.title}
      >
        <Trans>No data</Trans>
      </div>
    );
  }
  return <ChartCanvas {...props} />;
}

function ChartCanvas({ labels, series, title, horizontal = false, timeAxis }: AnalyticsChartProps) {
  const node = useRef<HTMLDivElement>(null);
  const instance = useRef<ReturnType<typeof init> | null>(null);
  const [width, setWidth] = useState(0);
  const { resolvedTheme } = useTheme();
  useEffect(() => {
    if (!node.current) return;
    const chart = init(node.current, undefined, { renderer: "svg" });
    instance.current = chart;
    const resize = () => {
      chart.resize();
      setWidth(chart.getWidth());
    };
    resize();
    const observer = new ResizeObserver(resize);
    observer.observe(node.current);
    return () => {
      observer.disconnect();
      chart.dispose();
      instance.current = null;
    };
  }, []);
  useEffect(() => {
    if (!node.current || !instance.current) return;
    const colors = ["--chart-3", "--chart-5", "--chart-2", "--chart-4", "--chart-1"].map((token) =>
      tokenColor(node.current!, token),
    );
    instance.current.setOption(
      chartOption(
        labels,
        series,
        colors,
        tokenColor(node.current, "--muted-foreground"),
        tokenColor(node.current, "--border"),
        {
          horizontal,
          danger: tokenColor(node.current, "--destructive"),
          timeAxis,
          width,
          description: title,
        },
      ),
      // Reset old zero-range bounds without resetting the user's legend selection.
      { replaceMerge: ["series", "xAxis", "yAxis"] },
    );
  }, [labels, series, horizontal, timeAxis, width, title, resolvedTheme]);
  return <div ref={node} className="h-72 w-full min-w-0" role="img" aria-label={title} />;
}
