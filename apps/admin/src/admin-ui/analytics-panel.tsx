import { useEffect, useRef, useState, type ReactNode } from "react";
import { Trans, useLingui } from "@lingui/react/macro";
import { RefreshCwIcon } from "lucide-react";

import { api } from "../api";
import type {
  AnalyticsDimension,
  AnalyticsGroup,
  AnalyticsOptions,
  AnalyticsStats,
} from "../types";
import { AnalyticsChart, type ChartSeries } from "./analytics-chart";
import {
  createAnalyticsLoader,
  ratio,
  selectionKey,
  type AnalyticsSelection,
  type AnalyticsSnapshot,
} from "./analytics-loader";
import { Field } from "./shared";
import { formatNumber } from "./helpers";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Skeleton } from "@/components/ui/skeleton";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";

const number = (value: number | null | undefined, unit = "") =>
  value == null ? "—" : `${value.toLocaleString(undefined, { maximumFractionDigits: 1 })}${unit}`;
const percent = (value: number | null) =>
  value == null
    ? "—"
    : new Intl.NumberFormat(undefined, { style: "percent", maximumFractionDigits: 1 }).format(
        value,
      );

function useAnalytics(query: AnalyticsSelection) {
  const [snapshot, setSnapshot] = useState<AnalyticsSnapshot | null>(null);
  const refresh = useRef<() => void>(() => {});
  const key = selectionKey(query);
  const { range, model, accountId, rankBy } = query;
  useEffect(() => {
    const loader = createAnalyticsLoader(
      { range, model, accountId, rankBy },
      (selection, signal) => api.analytics(selection, signal),
      setSnapshot,
    );
    const load = () => {
      if (document.visibilityState !== "hidden") void loader.refresh();
    };
    refresh.current = load;
    load();
    const timer = window.setInterval(load, 15_000);
    document.addEventListener("visibilitychange", load);
    return () => {
      loader.dispose();
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", load);
    };
  }, [range, model, accountId, rankBy]);
  return { snapshot: snapshot?.key === key ? snapshot : null, refresh: () => refresh.current() };
}

function useGroupLabel() {
  const { t } = useLingui();
  return (group: AnalyticsGroup) =>
    group.other
      ? t`Other`
      : group.deleted
        ? t`Deleted account (${group.id?.slice(0, 8)})`
        : group.name || group.id || t`Unassigned`;
}

function HistoricalFilter({
  dimension,
  range,
  selected,
  onSelect,
}: {
  dimension: AnalyticsDimension;
  range: AnalyticsSelection["range"];
  selected: AnalyticsGroup | null;
  onSelect: (group: AnalyticsGroup | null) => void;
}) {
  const { t } = useLingui();
  const label = useGroupLabel();
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState("");
  const [page, setPage] = useState(0);
  const [result, setResult] = useState<{
    key: string;
    data?: AnalyticsOptions;
    failed?: boolean;
  } | null>(null);
  const key = JSON.stringify([range, dimension, search, page]);
  useEffect(() => {
    if (!open) return;
    const controller = new AbortController();
    const timer = window.setTimeout(() => {
      api
        .analyticsOptions({ range, dimension, search, page }, controller.signal)
        .then((data) => {
          if (!controller.signal.aborted) setResult({ key, data });
        })
        .catch(() => {
          if (!controller.signal.aborted) setResult({ key, failed: true });
        });
    }, 250);
    return () => {
      window.clearTimeout(timer);
      controller.abort();
    };
  }, [open, range, dimension, search, page, key]);
  const data = result?.key === key ? result.data : null;
  const title = dimension === "model" ? t`Model` : t`Account`;
  return (
    <details
      className="min-w-0 rounded-md border p-3"
      onToggle={(event) => setOpen(event.currentTarget.open)}
    >
      <summary className="cursor-pointer truncate text-sm focus-visible:outline-2 focus-visible:outline-ring">
        {title}: {selected ? label(selected) : t`All`}
      </summary>
      {open ? (
        <div className="mt-3 flex flex-col gap-3">
          <Field
            label={t`Search historical names or IDs`}
            htmlFor={`analytics-${dimension}-search`}
          >
            <Input
              id={`analytics-${dimension}-search`}
              maxLength={100}
              value={search}
              onChange={(e) => {
                setSearch(e.target.value);
                setPage(0);
              }}
            />
          </Field>
          <p className="text-xs text-muted-foreground">
            <Trans>Includes deleted accounts observed in this time range.</Trans>
          </p>
          <Button variant="outline" size="sm" onClick={() => onSelect(null)}>
            <Trans>Clear filter</Trans>
          </Button>
          {!data && result?.key === key && result.failed ? (
            <p role="alert">
              <Trans>Could not load filter options. Change the search to retry.</Trans>
            </p>
          ) : !data ? (
            <Skeleton className="h-24 w-full" />
          ) : (
            <>
              <div className="flex max-h-52 flex-col gap-1 overflow-y-auto">
                {data.data
                  .filter((group) => dimension === "account" || group.id != null)
                  .map((group) => (
                    <Button
                      key={group.id ?? "unassigned"}
                      variant="ghost"
                      size="sm"
                      className="justify-start"
                      onClick={() => onSelect(group)}
                      title={group.id ?? undefined}
                    >
                      <span className="truncate">{label(group)}</span>
                    </Button>
                  ))}
                {data.data.length === 0 ? (
                  <p className="text-sm text-muted-foreground">
                    <Trans>No matching history</Trans>
                  </p>
                ) : null}
              </div>
              <div className="flex items-center justify-between gap-2">
                <Button
                  variant="outline"
                  size="sm"
                  disabled={page === 0}
                  onClick={() => setPage(page - 1)}
                >
                  <Trans>Previous</Trans>
                </Button>
                <span className="text-xs tabular-nums">{page + 1}</span>
                <Button
                  variant="outline"
                  size="sm"
                  disabled={!data.hasMore || page >= 1000}
                  onClick={() => setPage(page + 1)}
                >
                  <Trans>Next</Trans>
                </Button>
              </div>
            </>
          )}
        </div>
      ) : null}
    </details>
  );
}

function Plot({
  title,
  description,
  labels,
  series,
  horizontal = false,
}: {
  title: string;
  description: ReactNode;
  labels: string[];
  series: ChartSeries[];
  horizontal?: boolean;
}) {
  return (
    <Card className="min-w-0">
      <CardHeader>
        <CardTitle>{title}</CardTitle>
        <CardDescription>{description}</CardDescription>
      </CardHeader>
      <CardContent>
        <AnalyticsChart title={title} labels={labels} series={series} horizontal={horizontal} />
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
                {labels.map((label, index) => (
                  <TableRow key={`${index}-${label}`}>
                    <TableCell>{label}</TableCell>
                    {series.map((s) => (
                      <TableCell key={s.name}>{number(s.values[index])}</TableCell>
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

function Counts({ rows }: { rows: readonly { name: string; count: number }[] }) {
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

export default function AnalyticsPanel() {
  const { t, i18n } = useLingui();
  const [query, setQuery] = useState<AnalyticsSelection>({ range: "24h", rankBy: "attempts" });
  const [selectedModel, setSelectedModel] = useState<AnalyticsGroup | null>(null);
  const [selectedAccount, setSelectedAccount] = useState<AnalyticsGroup | null>(null);
  const { snapshot, refresh } = useAnalytics(query);
  const history = snapshot?.data?.history;
  const runtime = snapshot?.data?.runtime;
  const label = useGroupLabel();
  const stats = history?.summary;
  const labels =
    history?.buckets.map(
      (bucket) =>
        `${new Date(bucket.startedAt).toLocaleString(i18n.locale, { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" })}${bucket.partial ? ` (${t`in progress`})` : ""}`,
    ) ?? [];
  const values = (value: (stats: AnalyticsStats) => number | null) =>
    history?.buckets.map((bucket) => value(bucket.stats)) ?? [];
  const known = stats ? stats.attempts - stats.unknown : 0;
  const rankValue = (group: AnalyticsGroup) =>
    query.rankBy === "tokens"
      ? group.tokens
      : query.rankBy === "failures"
        ? group.failures
        : group.attempts;
  const rankTitle =
    query.rankBy === "tokens"
      ? t`Recorded tokens`
      : query.rankBy === "failures"
        ? t`Failed attempts`
        : t`Completed attempts`;
  return (
    <section className="flex min-w-0 flex-col gap-5" aria-label={t`Relay analytics`}>
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h2 className="text-lg font-semibold">
            <Trans>Relay analytics</Trans>
          </h2>
          <p className="text-sm text-muted-foreground">
            <Trans>Completed upstream attempts, not user requests or live concurrency.</Trans>
          </p>
        </div>
        <div className="flex items-center gap-2">
          <Select
            value={query.range}
            onValueChange={(range) => {
              if (range === "1h" || range === "24h" || range === "7d")
                setQuery({ ...query, range });
            }}
          >
            <SelectTrigger aria-label={t`Time range`}>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectGroup>
                <SelectItem value="1h">
                  <Trans>Last hour</Trans>
                </SelectItem>
                <SelectItem value="24h">
                  <Trans>Last 24 hours</Trans>
                </SelectItem>
                <SelectItem value="7d">
                  <Trans>Last 7 days</Trans>
                </SelectItem>
              </SelectGroup>
            </SelectContent>
          </Select>
          <Button
            variant="outline"
            onClick={refresh}
            disabled={snapshot?.pending}
            aria-label={t`Refresh analytics`}
          >
            <RefreshCwIcon data-icon="inline-start" />
            <Trans>Refresh</Trans>
          </Button>
        </div>
      </div>
      <div className="grid items-start gap-3 md:grid-cols-2">
        <HistoricalFilter
          dimension="model"
          range={query.range}
          selected={selectedModel}
          onSelect={(group) => {
            setSelectedModel(group);
            setQuery({ ...query, model: group?.id ?? undefined });
          }}
        />
        <HistoricalFilter
          dimension="account"
          range={query.range}
          selected={selectedAccount}
          onSelect={(group) => {
            setSelectedAccount(group);
            setQuery({ ...query, accountId: group ? (group.id ?? "unassigned") : undefined });
          }}
        />
      </div>
      {snapshot?.error ? (
        <Alert variant="destructive">
          <AlertTitle>
            {history ? t`Showing an older snapshot` : t`Analytics unavailable`}
          </AlertTitle>
          <AlertDescription>
            <Trans>
              Refresh to retry, or select a shorter range. No data from a different filter is shown.
            </Trans>
          </AlertDescription>
        </Alert>
      ) : null}
      {!history ? (
        snapshot?.error ? null : (
          <div className="grid gap-4 md:grid-cols-2">
            <Skeleton className="h-32 w-full md:col-span-2" />
            <Skeleton className="h-80 w-full" />
            <Skeleton className="h-80 w-full" />
          </div>
        )
      ) : (
        <>
          <div
            className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground"
            aria-live="polite"
          >
            <span>
              {new Date(history.windowStartedAt).toLocaleString()} –{" "}
              {new Date(history.windowEndedAt).toLocaleString()}
            </span>
            <Badge variant="outline">{Intl.DateTimeFormat().resolvedOptions().timeZone}</Badge>
            <span>
              <Trans>Updated {new Date(history.generatedAt).toLocaleTimeString()}</Trans>
            </span>
            <span>
              <Trans>Refreshes every 15 seconds while visible</Trans>
            </span>
          </div>
          {stats ? (
            <dl className="grid grid-cols-2 gap-4 border-y py-4 lg:grid-cols-5">
              {[
                [
                  t`Completed attempts`,
                  formatNumber(stats.attempts),
                  t`${stats.retryAttempts} retry attempts; ${stats.overloadRetries} internal overload retries`,
                ],
                [
                  t`Successful attempts`,
                  percent(ratio(stats.success, known)),
                  t`${known} of ${stats.attempts} results classified; cancellations included`,
                ],
                [
                  t`TTFT P95`,
                  number(stats.ttft.p95, " ms"),
                  t`${stats.ttft.samples} successful samples`,
                ],
                [
                  t`Recorded tokens`,
                  formatNumber(stats.inputTokens + stats.outputTokens),
                  t`${stats.completeUsageSamples} of ${stats.attempts} usage records complete`,
                ],
                [
                  t`Cache hit rate`,
                  percent(ratio(stats.cachedInputTokens, stats.classifiedInputTokens)),
                  t`${percent(ratio(stats.classifiedInputTokens, stats.inputTokens))} input coverage; ${stats.cacheSamples} samples`,
                ],
              ].map(([title, value, detail]) => (
                <div key={title} className="flex min-w-0 flex-col gap-1">
                  <dt className="text-sm text-muted-foreground">{title}</dt>
                  <dd className="text-2xl font-semibold tabular-nums">{value}</dd>
                  <dd className="text-xs text-muted-foreground">{detail}</dd>
                </div>
              ))}
            </dl>
          ) : null}
          {stats?.attempts === 0 ? (
            <Alert>
              <AlertTitle>
                <Trans>No completed attempts in this window</Trans>
              </AlertTitle>
              <AlertDescription>
                <Trans>
                  Try a longer range or clear the model and account filters. Missing measurements
                  are not plotted as zero.
                </Trans>
              </AlertDescription>
            </Alert>
          ) : null}
          <div className="grid gap-5 xl:grid-cols-2">
            <Plot
              title={t`Attempt outcomes`}
              description={t`By completion time. The last interval is still in progress.`}
              labels={labels}
              series={[
                {
                  name: t`Success`,
                  values: values((s) => s.success),
                  type: "bar",
                  stack: "outcomes",
                },
                {
                  name: t`Failed attempts`,
                  tone: "failure",
                  values: values((s) => s.failures),
                  type: "bar",
                  stack: "outcomes",
                },
                {
                  name: t`Client cancellation`,
                  values: values((s) => s.clientDisconnect),
                  type: "bar",
                  stack: "outcomes",
                },
                {
                  name: t`Server shutdown`,
                  values: values((s) => s.serverShutdown),
                  type: "bar",
                  stack: "outcomes",
                },
                {
                  name: t`Unknown`,
                  values: values((s) => s.unknown),
                  type: "bar",
                  stack: "outcomes",
                },
              ]}
            />
            <Plot
              title={t`Time to first token · ms`}
              description={t`Nearest-rank percentiles of successful, measured attempts. Missing samples leave gaps.`}
              labels={labels}
              series={[
                { name: t`P50`, values: values((s) => s.ttft.p50 ?? null) },
                { name: t`P95`, values: values((s) => s.ttft.p95 ?? null) },
              ]}
            />
            <Plot
              title={t`Attempt duration · ms`}
              description={t`Excludes queue wait and earlier attempts; not end-to-end user latency.`}
              labels={labels}
              series={[
                { name: t`P50`, values: values((s) => s.latency.p50 ?? null) },
                { name: t`P95`, values: values((s) => s.latency.p95 ?? null) },
              ]}
            />
            <Plot
              title={t`Queue wait · ms`}
              description={t`Successful attempts with queue measurements.`}
              labels={labels}
              series={[
                { name: t`P50`, values: values((s) => s.queue.p50 ?? null) },
                { name: t`P95`, values: values((s) => s.queue.p95 ?? null) },
              ]}
            />
            <Plot
              title={t`Output speed · token/s`}
              description={t`Median after the first token; excludes TTFT. ${stats?.otps.samples ?? 0} measured successful attempts.`}
              labels={labels}
              series={[{ name: t`Median`, values: values((s) => s.otps.p50 ?? null) }]}
            />
            <Plot
              title={t`Recorded token usage`}
              description={t`Cached input is part of input, not extra usage. Unclassified input has incomplete cache metadata.`}
              labels={labels}
              series={[
                {
                  name: t`Cached input`,
                  values: values((s) => s.cachedInputTokens),
                  type: "bar",
                  stack: "tokens",
                },
                {
                  name: t`Uncached input`,
                  values: values((s) => s.classifiedInputTokens - s.cachedInputTokens),
                  type: "bar",
                  stack: "tokens",
                },
                {
                  name: t`Unclassified input`,
                  values: values((s) => s.inputTokens - s.classifiedInputTokens),
                  type: "bar",
                  stack: "tokens",
                },
                {
                  name: t`Output`,
                  values: values((s) => s.outputTokens),
                  type: "bar",
                  stack: "tokens",
                },
              ]}
            />
            <Plot
              title={t`Cache hit rate · %`}
              description={t`Weighted by valid input tokens, not an average of request percentages. Missing cache metadata is excluded.`}
              labels={labels}
              series={[
                {
                  name: t`Cache hit rate`,
                  values: values((s) => {
                    const r = ratio(s.cachedInputTokens, s.classifiedInputTokens);
                    return r == null ? null : r * 100;
                  }),
                },
              ]}
            />
            <Card className="min-w-0">
              <CardHeader>
                <CardTitle>
                  <Trans>Historical classifications</Trans>
                </CardTitle>
                <CardDescription>
                  <Trans>
                    Old records without persisted metadata remain unknown. Routing rejections before
                    an attempt are excluded.
                  </Trans>
                </CardDescription>
              </CardHeader>
              <CardContent>
                <Counts rows={history.outcomes} />
                <details className="mt-3">
                  <summary className="cursor-pointer text-sm">
                    <Trans>Transport and status codes</Trans>
                  </summary>
                  <Counts
                    rows={[
                      ...history.transports.map((row) => ({
                        ...row,
                        name: `transport: ${row.name}`,
                      })),
                      ...history.statuses.map((row) => ({ ...row, name: `HTTP: ${row.name}` })),
                    ]}
                  />
                </details>
              </CardContent>
            </Card>
          </div>
          <div className="flex items-center justify-between gap-3">
            <h3 className="font-semibold">
              <Trans>Model and account comparison</Trans>
            </h3>
            <Select
              value={query.rankBy}
              onValueChange={(rankBy) => {
                if (rankBy === "attempts" || rankBy === "tokens" || rankBy === "failures")
                  setQuery({ ...query, rankBy });
              }}
            >
              <SelectTrigger aria-label={t`Rank by`}>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectGroup>
                  <SelectItem value="attempts">
                    <Trans>Attempts</Trans>
                  </SelectItem>
                  <SelectItem value="tokens">
                    <Trans>Tokens</Trans>
                  </SelectItem>
                  <SelectItem value="failures">
                    <Trans>Failures</Trans>
                  </SelectItem>
                </SelectGroup>
              </SelectContent>
            </Select>
          </div>
          <div className="grid gap-5 xl:grid-cols-2">
            <Plot
              title={t`Models`}
              description={t`Top 10 + other · ${rankTitle}`}
              labels={history.models.map(label)}
              series={[{ name: rankTitle, values: history.models.map(rankValue), type: "bar" }]}
              horizontal
            />
            <Plot
              title={t`Accounts`}
              description={t`Top 10 + other · ${rankTitle}`}
              labels={history.accounts.map(label)}
              series={[{ name: rankTitle, values: history.accounts.map(rankValue), type: "bar" }]}
              horizontal
            />
          </div>
        </>
      )}
      {runtime ? (
        <Card>
          <CardHeader>
            <CardTitle>
              <Trans>Since service startup</Trans>
            </CardTitle>
            <CardDescription>
              <Trans>
                Unfiltered process counters. Not affected by the history range, model or account
                filters; resets on restart.
              </Trans>
            </CardDescription>
          </CardHeader>
          <CardContent>
            <p className="text-xs text-muted-foreground">
              {new Date(runtime.startedAt).toLocaleString()} –{" "}
              {new Date(runtime.snapshotAt).toLocaleString()}
            </p>
            <p className="mt-3 text-sm">
              <Trans>Internal overload retries: {runtime.overloadRetries}</Trans>
            </p>
            <details className="mt-3">
              <summary className="cursor-pointer text-sm">
                <Trans>Transport, outcomes and retries</Trans>
              </summary>
              <Counts
                rows={runtime.requestOutcomes.map((row) => ({
                  name: `${row.transport} · ${row.statusCode} · ${row.outcome} · ${t`retry index`} ${row.retryCount}`,
                  count: row.count,
                }))}
              />
            </details>
            <details className="mt-3">
              <summary className="cursor-pointer text-sm">
                <Trans>Session affinity and route selections</Trans>
              </summary>
              <Counts
                rows={[
                  ...runtime.sessionAffinity.map((row) => ({
                    name: `${row.client} · ${row.wireApi} · ${row.status}`,
                    count: row.count,
                  })),
                  ...runtime.routeSelections.map((row) => ({
                    name: `${row.wireApi} · ${row.role} · ${row.affinity}`,
                    count: row.count,
                  })),
                ]}
              />
            </details>
            <details className="mt-3">
              <summary className="cursor-pointer text-sm">
                <Trans>Routing rejections</Trans>
              </summary>
              <Counts
                rows={runtime.selectionFailures.map((row) => ({
                  name: `${row.model} · ${row.reason} · ${row.statusCode}`,
                  count: row.count,
                }))}
              />
            </details>
          </CardContent>
        </Card>
      ) : null}
    </section>
  );
}
