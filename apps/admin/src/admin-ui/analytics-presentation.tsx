import type { ReactNode } from "react";
import { Trans } from "@lingui/react/macro";

import { AnalyticsChart, type ChartSeries, type ChartTimeAxis } from "./analytics-chart";
import { formatNumber } from "./helpers";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";

export const formatMeasurement = (value: number | null | undefined, unit = "") =>
  value == null || !Number.isFinite(value)
    ? "—"
    : `${value.toLocaleString(undefined, { maximumFractionDigits: 1 })}${unit}`;

export function AnalyticsPlot({
  title,
  description,
  labels,
  series,
  horizontal = false,
  timeAxis,
}: {
  title: string;
  description: ReactNode;
  labels: string[];
  series: ChartSeries[];
  horizontal?: boolean;
  timeAxis?: ChartTimeAxis;
}) {
  return (
    <Card className="min-w-0">
      <CardHeader>
        <CardTitle>{title}</CardTitle>
        <CardDescription>{description}</CardDescription>
      </CardHeader>
      <CardContent>
        <AnalyticsChart
          title={title}
          labels={labels}
          series={series}
          horizontal={horizontal}
          timeAxis={timeAxis}
        />
        <details className="mt-2 text-xs text-muted-foreground">
          <summary className="cursor-pointer focus-visible:outline-2 focus-visible:outline-ring">
            <Trans>View data table</Trans>
          </summary>
          <div className="mt-3 max-h-72 overflow-auto">
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>
                    <Trans>Interval / group</Trans>
                  </TableHead>
                  {series.map((s) => (
                    <TableHead key={s.name}>{s.name}</TableHead>
                  ))}
                </TableRow>
              </TableHeader>
              <TableBody>
                {labels.length === 0 ? (
                  <TableRow>
                    <TableCell colSpan={series.length + 1}>
                      <Trans>No data</Trans>
                    </TableCell>
                  </TableRow>
                ) : null}
                {labels.map((label, index) => (
                  <TableRow key={`${index}-${label}`}>
                    <TableCell>{label}</TableCell>
                    {series.map((s) => (
                      <TableCell key={s.name}>{formatMeasurement(s.values[index])}</TableCell>
                    ))}
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </div>
        </details>
      </CardContent>
    </Card>
  );
}

export function AnalyticsCounts({ rows }: { rows: readonly { name: string; count: number }[] }) {
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>
            <Trans>Classification</Trans>
          </TableHead>
          <TableHead>
            <Trans>Count</Trans>
          </TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {rows.length === 0 ? (
          <TableRow>
            <TableCell colSpan={2}>
              <Trans>No data</Trans>
            </TableCell>
          </TableRow>
        ) : null}
        {rows.map((row) => (
          <TableRow key={row.name}>
            <TableCell className="whitespace-normal break-words">{row.name}</TableCell>
            <TableCell>{formatNumber(row.count)}</TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}
