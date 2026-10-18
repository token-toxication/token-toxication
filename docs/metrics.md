# Relay analytics

The Overview uses ECharts and authenticated SQLite-backed analytics. Choose the
last hour, 24 hours, or seven days, then optionally filter by model or provider
account. Historical account search includes deleted accounts; those rows retain
their stable IDs rather than disappearing from totals.

## Reading the charts

- Counts are **completed upstream attempts**, grouped by completion time, not
  user requests, arrival rate, or live concurrency. A retry can add an attempt.
- Success rate uses classified attempts as its denominator, including client
  cancellations and service shutdowns. Unknown outcomes are shown separately.
  This is not an end-to-end availability or logical-request success metric.
- TTFT, attempt duration, queue wait, and output speed use successful measured
  attempts only. Percentiles use exact nearest rank; absent samples stay empty.
  Attempt duration excludes queueing and previous attempts. Output speed excludes
  the wait for the first token.
- Total recorded tokens are input plus output. Cached tokens are part of input.
  The cache split and weighted hit rate only use complete, valid usage records;
  other recorded input is shown as unclassified. Missing usage is not free usage.
- The rightmost bucket is still in progress. Times display in the browser's local
  timezone; the API returns UTC boundaries. Historical series survive restarts.
- Runtime diagnostics are **unfiltered counters since service startup**. They
  reset on restart and include routing rejections that have no upstream attempt.
  Internal overload retries and cross-account retry attempts are distinct counts.
  Internal retry totals count completed attempts explicitly marked as internal
  retries, never the cumulative retry index attached to each log row.

Older logs do not have persisted transport/outcome metadata. Their attempts,
status codes, and recorded tokens remain visible, but classifications and
successful-performance sample coverage may initially be incomplete. The relay
does not guess WebSocket transport from the request path.
Old records without the per-attempt internal-retry marker are excluded from that
counter; cumulative retry indexes are preserved in request logs, not summed.

## Refresh and resource limits

The Overview refreshes analytics every 15 seconds while visible. A failed refresh
keeps the snapshot for the **same filters** and marks it stale. Changing filters
never displays another filter's snapshot; late responses are ignored. Each chart
also exposes an accessible data table.

Analytics use an independent query-only SQLite connection in a blocking worker,
not the relay's shared connection mutex. One worker runs at a time, a queued
request waits at most one second, SQLite work is interrupted after five seconds,
and up to 16 query contexts are cached for ten seconds. Responses have at most
288 buckets and each ranking has ten entries plus Other. Exact percentiles accept
at most 500,000 successful attempts per query to bound memory; larger or timed-out
queries fail rather than sampling silently. Select a shorter range or narrower
filter if necessary. These limits bound analysis work, not relay request traffic.

Admin-only endpoints:

- `GET /admin/api/analytics?range=24h&rankBy=attempts`
- `GET /admin/api/analytics/options?range=24h&dimension=account&search=&page=0`

Optional history filters are `model` and `accountId` (`unassigned` selects no
account). Ranking choices are `attempts`, `tokens`, and `failures`. Options are
searchable by name or ID, 50 per page. Responses contain aggregate metadata only,
not prompts, credentials, request bodies, or upstream error messages. No separate
monitoring service or production configuration change is required.
