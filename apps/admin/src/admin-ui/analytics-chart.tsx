import { useEffect, useRef } from "react";
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

export function chartOption(
  labels: string[],
  series: ChartSeries[],
  colors: string[],
  foreground: string,
  border: string,
  horizontal = false,
  danger = foreground,
): EChartsCoreOption {
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
    axisLabel: {
      color: foreground,
      formatter: (n: number) => Intl.NumberFormat(undefined, { notation: "compact" }).format(n),
    },
    splitLine: { lineStyle: { color: border, type: "dashed" } },
  };
  return {
    animation: false,
    aria: { enabled: true, decal: { show: true } },
    color: colors,
    textStyle: { fontFamily: "Geist Variable, sans-serif" },
    grid: { top: 14, left: horizontal ? 136 : 55, right: 18, bottom: 76 },
    legend: { bottom: 0, textStyle: { color: foreground }, type: "scroll" },
    // Rich text is drawn by ECharts, never inserted as HTML. Names are untrusted.
    tooltip: { trigger: "axis", renderMode: "richText", confine: true },
    xAxis: horizontal ? value : category,
    yAxis: horizontal ? { ...category, inverse: true } : value,
    series: series.map((s, index) => ({
      name: s.name,
      type: s.type ?? "line",
      stack: s.stack,
      data: s.values,
      connectNulls: false,
      showSymbol: true,
      showAllSymbol: true,
      symbolSize: 4,
      smooth: false,
      lineStyle: {
        width: 2,
        type: index % 2 ? "dashed" : "solid",
        ...(s.tone === "failure" ? { color: danger } : {}),
      },
      ...(s.tone === "failure" ? { itemStyle: { color: danger } } : {}),
      emphasis: { focus: "series" },
      barMaxWidth: 30,
    })),
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

export function AnalyticsChart({
  labels,
  series,
  title,
  horizontal = false,
}: {
  labels: string[];
  series: ChartSeries[];
  title: string;
  horizontal?: boolean;
}) {
  const node = useRef<HTMLDivElement>(null);
  const instance = useRef<ReturnType<typeof init> | null>(null);
  const { resolvedTheme } = useTheme();
  useEffect(() => {
    if (!node.current) return;
    const chart = init(node.current, undefined, { renderer: "svg" });
    instance.current = chart;
    const observer = new ResizeObserver(() => chart.resize());
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
        horizontal,
        tokenColor(node.current, "--destructive"),
      ),
      { replaceMerge: ["series"] },
    );
  }, [labels, series, horizontal, resolvedTheme]);
  return <div ref={node} className="h-72 w-full min-w-0" role="img" aria-label={title} />;
}
