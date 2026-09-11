import { Trans, useLingui } from "@lingui/react/macro";
import { Link, useParams } from "react-router";

import { adminPaths } from "../admin-routes";
import {
  formatCacheHitRate,
  formatDate,
  formatLogModel,
  formatNumber,
  formatRequestSummary,
  statusCodeBadge,
} from "./helpers";
import { EmptyNotice, MissingRecordView } from "./shared";
import type { RequestLog } from "../types";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";

export function RequestLogDetailView({ logs }: { logs: readonly RequestLog[] }) {
  const { t } = useLingui();
  const { logId } = useParams();
  const log = logs.find((item) => item.id === logId);

  if (!log) {
    return (
      <MissingRecordView
        title={t`Request Log not found`}
        body={t`This Request Log may have aged out of the current log collection or the link is incomplete.`}
        to={adminPaths.logs()}
        label={t`Back to request log`}
      />
    );
  }

  return (
    <div className="flex flex-col gap-5">
      <div className="flex flex-col justify-between gap-4 sm:flex-row sm:items-start">
        <div className="flex min-w-0 flex-col gap-2">
          <Button asChild type="button" variant="ghost" size="sm" className="w-fit">
            <Link to={adminPaths.logs()}>
              <Trans>Request Log</Trans>
            </Link>
          </Button>
          <div>
            <div className="text-sm text-muted-foreground">
              <Trans>Request Log</Trans>
            </div>
            <h1 className="truncate text-2xl font-semibold">{formatLogModel(log)}</h1>
            <p className="mt-1 text-sm text-muted-foreground">{formatDate(log.createdAt)}</p>
          </div>
        </div>
        {statusCodeBadge(log.statusCode)}
      </div>

      <div className="grid gap-5 xl:grid-cols-[0.75fr_1.25fr]">
        <Card>
          <CardHeader>
            <CardTitle>
              <Trans>Request summary</Trans>
            </CardTitle>
            <CardDescription>
              <Trans>Relay result and accounting for this completed attempt.</Trans>
            </CardDescription>
          </CardHeader>
          <CardContent>
            <dl className="grid gap-4 text-sm">
              <div>
                <dt className="text-xs text-muted-foreground">
                  <Trans>Latency</Trans>
                </dt>
                <dd className="mt-1">
                  <Trans>{log.latencyMs}ms</Trans>
                </dd>
              </div>
              <div>
                <dt className="text-xs text-muted-foreground">
                  <Trans>Tokens</Trans>
                </dt>
                <dd className="mt-1">
                  <Trans>
                    {formatNumber(log.inputTokens + log.outputTokens)} total ·{" "}
                    {formatCacheHitRate(log.cachedInputTokens, log.inputTokens)} cache hit
                  </Trans>
                </dd>
              </div>
              <div>
                <dt className="text-xs text-muted-foreground">
                  <Trans>Client key</Trans>
                </dt>
                <dd className="mt-1 font-mono text-xs">{log.apiKeyId}</dd>
              </div>
              {log.providerAccountId ? (
                <div>
                  <dt className="text-xs text-muted-foreground">
                    <Trans>Provider Account</Trans>
                  </dt>
                  <dd className="mt-1">
                    <Link
                      to={adminPaths.account(log.providerAccountId)}
                      className="font-mono text-xs underline-offset-4 hover:underline"
                    >
                      {log.providerAccountId}
                    </Link>
                  </dd>
                </div>
              ) : null}
            </dl>
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle>
              <Trans>Routing details</Trans>
            </CardTitle>
            <CardDescription>{formatRequestSummary(log)}</CardDescription>
          </CardHeader>
          <CardContent>
            <RequestLogFacts log={log} />
          </CardContent>
        </Card>
      </div>
    </div>
  );
}

export function RequestLogsView({
  logs,
  compact = false,
}: {
  logs: readonly RequestLog[];
  compact?: boolean;
}) {
  const { t } = useLingui();

  return (
    <Card>
      <CardHeader>
        <CardTitle>{compact ? t`Recent requests` : t`Request Log`}</CardTitle>
        <CardDescription>
          <Trans>
            Relay status, latency, model, sanitized routing metadata, and token accounting.
          </Trans>
        </CardDescription>
      </CardHeader>
      <CardContent>
        <div className="hidden xl:block">
          <Table className="table-fixed">
            <colgroup>
              <col className="w-32" />
              <col className="w-[18%]" />
              <col className="w-[14%]" />
              <col />
              <col className="w-20" />
              <col className="w-24" />
              <col className="w-24" />
              <col className="w-24" />
            </colgroup>
            <TableHeader>
              <TableRow>
                <TableHead>
                  <Trans>Time</Trans>
                </TableHead>
                <TableHead>
                  <Trans>Path</Trans>
                </TableHead>
                <TableHead>
                  <Trans>Model</Trans>
                </TableHead>
                <TableHead>
                  <Trans>Request</Trans>
                </TableHead>
                <TableHead>
                  <Trans>Status</Trans>
                </TableHead>
                <TableHead>
                  <Trans>Latency</Trans>
                </TableHead>
                <TableHead className="text-right">
                  <Trans>Tokens</Trans>
                </TableHead>
                <TableHead className="text-right">
                  <Trans>Cache hit</Trans>
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {logs.map((log) => (
                <TableRow key={log.id}>
                  <TableCell className="text-xs text-muted-foreground">
                    <Link
                      to={adminPaths.log(log.id)}
                      className="underline-offset-4 hover:underline"
                    >
                      {formatDate(log.createdAt)}
                    </Link>
                  </TableCell>
                  <TableCell>
                    <div className="flex min-w-0 flex-col gap-1">
                      <Link
                        to={adminPaths.log(log.id)}
                        className="truncate font-mono text-xs underline-offset-4 hover:underline"
                        title={log.path}
                      >
                        {log.path}
                      </Link>
                      {log.upstreamUrl ? (
                        <span
                          className="truncate font-mono text-xs text-muted-foreground"
                          title={log.upstreamUrl}
                        >
                          {log.upstreamUrl}
                        </span>
                      ) : null}
                    </div>
                  </TableCell>
                  <TableCell>
                    <Link
                      to={adminPaths.log(log.id)}
                      className="block truncate underline-offset-4 hover:underline"
                      title={formatLogModel(log)}
                    >
                      {formatLogModel(log)}
                    </Link>
                  </TableCell>
                  <TableCell>
                    <span
                      className="block truncate text-xs text-muted-foreground"
                      title={formatRequestSummary(log)}
                    >
                      {formatRequestSummary(log)}
                    </span>
                  </TableCell>
                  <TableCell>{statusCodeBadge(log.statusCode)}</TableCell>
                  <TableCell className="text-right tabular-nums">
                    <Trans>{log.latencyMs}ms</Trans>
                  </TableCell>
                  <TableCell className="text-right tabular-nums">
                    {formatNumber(log.inputTokens + log.outputTokens)}
                  </TableCell>
                  <TableCell className="text-right tabular-nums">
                    {formatCacheHitRate(log.cachedInputTokens, log.inputTokens)}
                  </TableCell>
                </TableRow>
              ))}
              {logs.length === 0 ? (
                <TableRow>
                  <TableCell colSpan={8}>
                    <EmptyNotice
                      title={t`No relay traffic`}
                      body={t`Requests appear here after clients call the relay.`}
                    />
                  </TableCell>
                </TableRow>
              ) : null}
            </TableBody>
          </Table>
        </div>
        <div className="hidden md:block xl:hidden">
          <Table className="table-fixed">
            <colgroup>
              <col />
              <col className="w-20" />
              <col className="w-24" />
              <col className="w-24" />
              <col className="w-24" />
            </colgroup>
            <TableHeader>
              <TableRow>
                <TableHead>
                  <Trans>Request</Trans>
                </TableHead>
                <TableHead>
                  <Trans>Status</Trans>
                </TableHead>
                <TableHead className="text-right">
                  <Trans>Latency</Trans>
                </TableHead>
                <TableHead className="text-right">
                  <Trans>Tokens</Trans>
                </TableHead>
                <TableHead className="text-right">
                  <Trans>Cache hit</Trans>
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {logs.map((log) => (
                <TableRow key={log.id}>
                  <TableCell className="whitespace-normal">
                    <div className="min-w-0">
                      <Link
                        to={adminPaths.log(log.id)}
                        className="block truncate font-medium underline-offset-4 hover:underline"
                        title={formatLogModel(log)}
                      >
                        {formatLogModel(log)}
                      </Link>
                      <div
                        className="mt-1 truncate text-xs text-muted-foreground"
                        title={formatRequestSummary(log)}
                      >
                        {formatRequestSummary(log)}
                      </div>
                      <RequestLogDetails log={log} />
                    </div>
                  </TableCell>
                  <TableCell>{statusCodeBadge(log.statusCode)}</TableCell>
                  <TableCell className="text-right tabular-nums">
                    <Trans>{log.latencyMs}ms</Trans>
                  </TableCell>
                  <TableCell className="text-right tabular-nums">
                    {formatNumber(log.inputTokens + log.outputTokens)}
                  </TableCell>
                  <TableCell className="text-right tabular-nums">
                    {formatCacheHitRate(log.cachedInputTokens, log.inputTokens)}
                  </TableCell>
                </TableRow>
              ))}
              {logs.length === 0 ? (
                <TableRow>
                  <TableCell colSpan={5}>
                    <EmptyNotice
                      title={t`No relay traffic`}
                      body={t`Requests appear here after clients call the relay.`}
                    />
                  </TableCell>
                </TableRow>
              ) : null}
            </TableBody>
          </Table>
        </div>
        <div className="grid gap-3 md:hidden">
          {logs.length === 0 ? (
            <EmptyNotice
              title={t`No relay traffic`}
              body={t`Requests appear here after clients call the relay.`}
            />
          ) : (
            logs.map((log) => (
              <div key={log.id} className="flex flex-col gap-3 rounded-md border p-3">
                <div className="flex items-start justify-between gap-3">
                  <div className="min-w-0">
                    <Link
                      to={adminPaths.log(log.id)}
                      className="block truncate font-mono text-xs underline-offset-4 hover:underline"
                      title={log.path}
                    >
                      {log.path}
                    </Link>
                    <div
                      className="truncate text-xs text-muted-foreground"
                      title={formatLogModel(log)}
                    >
                      {formatLogModel(log)}
                    </div>
                    {log.upstreamUrl ? (
                      <div
                        className="truncate font-mono text-xs text-muted-foreground"
                        title={log.upstreamUrl}
                      >
                        {log.upstreamUrl}
                      </div>
                    ) : null}
                  </div>
                  {statusCodeBadge(log.statusCode)}
                </div>
                <div className="grid grid-cols-3 gap-2 text-xs text-muted-foreground">
                  <span>{formatDate(log.createdAt)}</span>
                  <span className="text-right tabular-nums">
                    <Trans>{log.latencyMs}ms</Trans>
                  </span>
                  <span className="text-right tabular-nums">
                    <span className="block">
                      <Trans>{formatNumber(log.inputTokens + log.outputTokens)} tokens</Trans>
                    </span>
                    <span className="block">
                      <Trans>
                        {formatCacheHitRate(log.cachedInputTokens, log.inputTokens)} cache hit
                      </Trans>
                    </span>
                  </span>
                </div>
                <div className="break-words text-xs leading-5 text-muted-foreground">
                  {formatRequestSummary(log)}
                </div>
              </div>
            ))
          )}
        </div>
      </CardContent>
    </Card>
  );
}

function RequestLogDetails({ log }: { log: RequestLog }) {
  return (
    <details className="group mt-2 text-xs text-muted-foreground">
      <summary className="w-fit cursor-pointer list-none text-foreground underline-offset-4 hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
        <Trans>Routing details</Trans>
      </summary>
      <RequestLogFacts log={log} />
    </details>
  );
}

function RequestLogFacts({ log }: { log: RequestLog }) {
  return (
    <dl className="mt-2 grid gap-1 border-l pl-3 text-xs text-muted-foreground">
      <div className="flex gap-2">
        <dt className="shrink-0">
          <Trans>Time</Trans>
        </dt>
        <dd>{formatDate(log.createdAt)}</dd>
      </div>
      <div className="flex min-w-0 gap-2">
        <dt className="shrink-0">
          <Trans>Path</Trans>
        </dt>
        <dd className="min-w-0 truncate font-mono" title={log.path}>
          {log.path}
        </dd>
      </div>
      {log.upstreamUrl ? (
        <div className="flex min-w-0 gap-2">
          <dt className="shrink-0">
            <Trans>Upstream</Trans>
          </dt>
          <dd className="min-w-0 truncate font-mono" title={log.upstreamUrl}>
            {log.upstreamUrl}
          </dd>
        </div>
      ) : null}
      <div className="flex gap-2">
        <dt className="shrink-0">
          <Trans>Tokens</Trans>
        </dt>
        <dd>
          <Trans>
            {formatNumber(log.inputTokens)} input · {formatNumber(log.cachedInputTokens)} cached ·{" "}
            {formatNumber(log.outputTokens)} output
          </Trans>
        </dd>
      </div>
      {log.error ? (
        <div className="flex min-w-0 gap-2 text-destructive">
          <dt className="shrink-0">
            <Trans>Error</Trans>
          </dt>
          <dd className="min-w-0 truncate" title={log.error}>
            {log.error}
          </dd>
        </div>
      ) : null}
    </dl>
  );
}
