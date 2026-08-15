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
- Status, latency, error category, and provider account

Request logs do not store:

- Prompt, message, or input content
- Authorization headers or API keys
- Query parameters
- Raw request or upstream response bodies
- Provider-controlled failure messages

### Token and cache accounting

When a provider reports usage, the relay stores input, cached-input, and output token counts. It does not estimate or locally tokenize requests.

Streaming logs are finalized after the upstream stream ends so terminal usage events are captured.

```text
cache hit rate = cached input tokens / input tokens
```

The dashboard shows today's aggregate cache hit rate and the rate for each request. Older records and providers without cached-token reporting display `0%`.

Failed streams retain provider-reported usage when present. If the provider sends no usage, the token fields remain zero and the request status and error identify the failed attempt.

## Configuration and TLS

Every server option is available as a command-line flag and, where listed, an environment variable. Run the server with `--help` for the complete mapping.

### Core settings

| Environment variable | Default | Purpose |
| --- | --- | --- |
| `TT_BIND_ADDR` | `0.0.0.0:3000` | Main HTTP or HTTPS listener |
| `TT_DATABASE_PATH` | `data/token-toxication.sqlite3` | SQLite database path |
| `TT_STATIC_DIR` | `apps/admin/dist` | Optional external admin build; embedded assets are the fallback |
| `TT_RELAY_STREAM_IDLE_TIMEOUT_SECS` | `60` | Maximum gap between upstream streaming response chunks |
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
