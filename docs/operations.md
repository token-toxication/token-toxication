# Operations

[Back to the project README](../README.md)

## Reliability

Provider account health and route health are tracked separately.

| Upstream outcome | Recorded behavior |
| --- | --- |
| HTTP 401 or 403 | Block the provider account |
| HTTP 429 | Cool the selected route for 60 seconds |
| MiniMax model quota reaches zero | Cool the selected route for one hour |
| HTTP 5xx or transport failure | Cool the selected route for 30 seconds |
| Responses `server_error` event | Record 502 and cool the route for 30 seconds |
| Responses `rate_limit_exceeded` event | Record 429 and cool the route for 60 seconds |
| Unknown Responses failure code | Record 502 without changing route health |

Codex subscription retries that have not produced text, reasoning, or tool output use the
OpenCode-compatible bounded policy: up to ten attempts, exponential gaps starting at two
seconds and capped at ten seconds per gap, jitter, and a maximum accepted `Retry-After` of
15 minutes. Requests with `previous_response_id` or already-started output are not replayed.

OpenAI Responses streams can fail after the upstream has already returned HTTP 200. The relay recognizes terminal `error` and `response.failed` events and records the attempt as failed.

The original response stream is forwarded unchanged. Failure classification affects route health and request logs, not the bytes returned to the client.

## Usage analytics and privacy

### Request logs

Request logs store the operational metadata required to diagnose routing and provider behavior:

- Upstream origin and path without query parameters
- Top-level request keys and body size
- Streaming mode
- Public and upstream model IDs
- Stripped parameter names
- Status, latency, average output token throughput (OTPS), error category, and provider account
- Logical request id, physical attempt index, retry-after advice, output boundary, and TTFT

Request logs do not store:

- Prompt, message, or input content
- Authorization headers or API keys
- Query parameters
- Raw request or upstream response bodies
- Provider-controlled failure messages

### Token and cache accounting

When a provider reports usage, the relay stores input, cached-input, and output token counts. It does not estimate or locally tokenize requests.

Streaming logs are finalized after the upstream stream ends so terminal usage events are captured.

`OTPS` is reported as output tokens divided by the time from the first output delta to stream
completion, excluding TTFT. Requests without an observable output delta display no OTPS value.

`TTFT` is the elapsed time from the start of one physical upstream attempt to its first
observable text, reasoning, or tool output. It is stored per attempt and is not folded into
the retry count or total latency. A retried logical request therefore has one TTFT for each
attempt, while the attempt index makes the retry boundary explicit.

```text
cache hit rate = cached input tokens / input tokens
```

The Metrics page shows a weighted cache hit rate for the selected hour, 24 hours, or
seven days. Only complete usage records with a valid cache split enter its
denominator; other recorded input remains unclassified. With no valid input
denominator it displays `—`, not `0%`. Per-request logs retain their recorded
usage values. See [Relay analytics](metrics.md) for coverage and metric definitions.

Failed streams retain provider-reported usage when present. If the provider sends no usage, the token fields remain zero and the request status and error identify the failed attempt.

## Configuration and TLS

Every server option is available as a command-line flag and, where listed, an environment variable. Run the server with `--help` for the complete mapping.

### Core settings

| Environment variable | Default | Purpose |
| --- | --- | --- |
| `TT_BIND_ADDR` | `0.0.0.0:3000` | Main HTTP or HTTPS listener |
| `TT_DATABASE_PATH` | `data/token-toxication.sqlite3` | SQLite database path |
| `TT_STATIC_DIR` | `apps/admin/dist` | Optional external admin build; embedded assets are the fallback |
| `TT_RELAY_STREAM_IDLE_TIMEOUT_SECS` | `1800` | Maximum gap between upstream streaming response chunks |
| `TT_RELAY_STREAM_MAX_DURATION_SECS` | `900` | Maximum lifetime of one relay stream |
| `TT_ADMIN_USERNAME` | `admin` | Admin login username |
| `TT_ADMIN_PASSWORD` | `change-this-password` | Admin login password; change before deployment |
| `TT_API_KEY_PREFIX` | `tokentoxication-` | Prefix for generated relay keys |
| `RUST_LOG` | Server and HTTP info logs | Tracing filter |

### TLS modes

| Mode | Behavior |
| --- | --- |
| `off` | Serve plain HTTP on `TT_BIND_ADDR` |
| `cert-files` | Serve HTTPS from an existing certificate and private key |
| `acme-http-01` | Obtain and renew certificates through ACME HTTP-01 |

Use existing certificate files:

```bash
token-toxication-server \
  --bind-addr 0.0.0.0:443 \
  --https-mode cert-files \
  --tls-cert-path /etc/token-toxication/fullchain.pem \
  --tls-key-path /etc/token-toxication/privkey.pem
```

Manage a certificate through ACME:

```bash
token-toxication-server \
  --bind-addr 0.0.0.0:443 \
  --https-mode acme-http-01 \
  --acme-identifier relay.example.com \
  --acme-email ops@example.com \
  --acme-http-bind-addr 0.0.0.0:80 \
  --acme-cert-dir /var/lib/token-toxication/acme
```

HTTP-01 validation requires the challenge listener to be reachable on public port 80. Use `--acme-allow-nonstandard-http-port` only when an explicit forwarding layer maps public port 80 to another local port.

ACME accepts a domain or public IP address. IP certificates use the short-lived profile and renew aggressively. Account data, certificates, keys, and metadata remain under the configured certificate directory.

When systemd runs the service as a non-root user on ports 80 or 443, grant only the low-port bind capability:

```ini
AmbientCapabilities=CAP_NET_BIND_SERVICE
CapabilityBoundingSet=CAP_NET_BIND_SERVICE
```

## Observed Codex token capacity

The Codex account quota dialog automatically shows observation segments for each
quota window, recorded token composition, and an estimated full-quota capacity.
There is no calibration button or exclusive-use setting. Existing request history
cannot reconstruct old quota changes: observation starts after this version runs.

For active Codex subscription accounts, the server samples quota once per minute
while requests are running and for ten minutes after recent activity. New accounts
receive an initial baseline. Idle accounts continue sampling approximately every
three minutes to preserve a settled baseline for the next session. Sampling uses at most four concurrent queries with a
20-second deadline each and does not change routing health. Large account lists
or slow upstreams can reduce sampling frequency. Opening the quota dialog also
records a sample. Reopen the dialog to retrieve the latest observation.

The generic `codex` pool is evaluated independently for its primary and secondary
windows; these overlap and their token counts must not be added together.
Additional quota pools are displayed without token attribution. Tokens are split
into uncached input (`input - cached input`), cached input, and output. Reasoning
is already part of output. Each upstream attempt, including retries and failures
with reported usage, counts separately. The workload detail lists models and
requested service tiers, not a guarantee of the tier actually delivered.

A full-quota estimate uses `observed tokens × 100 / consumed percentage points`.
It requires at least 10 percentage points of consumption (a conservative product
threshold, not an upstream guarantee), complete usage metadata, and idle endpoints
with no completed proxy requests in the preceding 30 seconds. Missing usage is
unknown, not zero. Requests crossing the baseline disable the estimate. Incomplete intervals are
archived at the next settled sample and collection resumes from a fresh baseline
automatically; unknown tokens are never filled in as zero. During
continuous activity the panel can remain in a settling state. A first busy interval
without a settled baseline may be excluded; collection resumes automatically once
a settled baseline is available.

A new segment starts after a server restart, a sampling gap over five minutes,
a plan/window change, or a reset boundary change. A remaining-quota increase of
at least one percentage point is confirmed by a later sample at least 30 seconds
apart; smaller corrections conservatively start a fresh segment. Confirmed
recoveries also work without reset-credit records, including observable extra
resets granted by Tibo. The system labels the observed recovery, not its source.
Consumption that hides a reset between samples cannot be detected reliably.
Ambiguous recovery intervals are excluded from extrapolation.

External account usage, upstream lag, rounding, changes in model/token mix, and
unobserved resets can bias results. Neither an estimate nor a full observed segment
is an official token allowance. The UI always explains this limitation and never
requires users to confirm exclusive proxy use.

Sanitized quota snapshots (no credentials or request bodies) are kept for 90 days
per sampled account. Up to 40 historical window segments are retained per account;
the UI returns the latest six. Historical totals are derived from retained request
logs, so removing logs also removes their metering evidence. Cleanup runs when a
new sample is saved; inactive accounts are not a strict wall-clock retention job.
