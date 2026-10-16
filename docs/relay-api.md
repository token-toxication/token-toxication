# Relay API

[Back to the project README](../README.md)

## Endpoints

| Protocol | Method | Endpoint | Recommended client base URL |
| --- | --- | --- | --- |
| Anthropic Messages | `POST` | `/anthropic/v1/messages` | `http://localhost:3000/anthropic` |
| OpenAI Chat | `POST` | `/openai/v1/chat/completions` | `http://localhost:3000/openai/v1` |
| OpenAI Responses | `POST` | `/openai/v1/responses` | `http://localhost:3000/openai/v1` |
| Codex model catalog | `GET` | `/openai/v1/codex/models` | `http://localhost:3000/openai/v1` |
| Gemini GenerateContent | `POST` | `/gemini/v1beta/models/{model}:generateContent` | `http://localhost:3000/gemini` |
| Gemini streaming | `POST` | `/gemini/v1beta/models/{model}:streamGenerateContent` | `http://localhost:3000/gemini` |

Model discovery is available at `/anthropic/v1/models`, `/openai/v1/models`, and `/gemini/v1beta/models`. Individual model lookups use the corresponding `/models/{model}` path.

## Service endpoints

| Endpoint | Description |
| --- | --- |
| `/health` | Service status, version, uptime, and timestamp |
| `/metrics` | JSON totals for API keys, provider health, usage, session-affinity and route-selection counters, and request outcome counters by transport |
| `/openapi.json` | OpenAPI document for the admin and relay APIs |

## Authentication

Relay API keys use the configured prefix, `tokentoxication-` by default. Send a key through any supported client convention:

- `Authorization: Bearer <key>`
- `x-api-key: <key>`
- `x-goog-api-key: <key>`
- `api-key: <key>`
- Gemini's `?key=<key>` query parameter

## Session affinity

Send `Token-Toxication-Session-ID` when the client does not already expose a supported native session identifier. The value is used only to choose a stable eligible route; it is not forwarded as an upstream authentication credential or written to request logs.

The explicit header wins over native values. A request with affinity uses weighted rendezvous hashing within the Primary tier, or within the Backup tier only when no Primary is eligible. A request without affinity uses weighted random selection and may reach a different account on each request.

The relay normally does not retry the current request on a different route.
Codex Device subscription requests have the bounded overload/rate-limit policy
described below. OpenAI `previous_response_id` values are forwarded unchanged
and are not used as the affinity key.

## Codex Responses compatibility

Codex device-login accounts use the same server-side identity contract as
OpenCode v2. The generated Codex configuration enables Responses WebSockets;
matching Codex clients use `/openai/v1/responses` as a WebSocket endpoint and
older clients can continue with HTTP/SSE.

### Codex model catalog

`GET /openai/v1/codex/models` serves the subscription account's own Codex
catalog, so the client reports that account's real context window instead of its
bundled value. The relay selects one enabled, non-blocked Codex subscription
account, fetches `{base_url}/codex/models` with that account's credentials, and
returns the upstream entries with two changes:

- models outside the supported range, hidden models, and models unsupported by
  the API are dropped;
- each public model gains a `-fast` variant that keeps every upstream field and
  sets `default_service_tier = "priority"`.

`-fast` is a relay-declared alias, not an upstream model: no upstream serves a
suffixed id. Route selection therefore resolves a `-fast` request to its base
model before choosing an account, and the upstream request carries the base id.
The requested id still appears in request logs, so Fast traffic stays
distinguishable. An operator-declared route for a `-fast` id wins over the alias.

Instruction text is required. An entry without `model_messages.instructions_template`
or `base_instructions` fails the whole request with `503` rather than serving a
catalog the client would reject. The endpoint answers `503` when no Codex
subscription account is configured. Upstream bodies are never logged, and the
response size and request duration are bounded.

For Device OAuth, the relay forwards only the bounded context headers
`session-id`, `thread-id`, `x-client-request-id`, `x-codex-beta-features`,
`x-codex-turn-state`, and `x-openai-subagent`. It generates the upstream
authorization, `chatgpt-account-id`, `originator: opencode`, and routed model;
client values for those server-owned fields are ignored. Responses preserve the
Codex turn-state, retry advice, request ID, model, reasoning, model-etag, and
fixed rate-limit metadata headers while dropping cookies and arbitrary upstream
headers.

The relay keeps overload retries bounded and does not replay a continuation or a
request after output has started. HTTP compression parity for very large
fallback requests remains a separate compatibility item; WebSockets are the
preferred path for Device OAuth clients.

Responses WebSocket terminal failures such as `rate_limit_exceeded` end only
the current response. The downstream client connection remains available for
the next request; if the upstream socket closed with that failure, the relay
reconnects before sending the next request. A transport failure while a
response is still active remains scoped to that response and is reported as a
connection termination.

`/metrics.requestOutcomes` records the transport (`http`, `http_sse`, or
`websocket`), upstream-facing status code, and bounded outcome class for every
completed relay attempt. `metrics.selectionFailures` records route-selection
rejections that happened before an upstream account could be selected. When all
matching routes are rate limited, the relay returns `429` with `Retry-After: 60`.
Non-success attempts also emit a structured journal record with the same
transport, status, outcome, and retry count. These fields are metadata-only;
prompts, tokens, and upstream bodies are never logged.
Running-request saturation is handled by the FIFO account queue. Queue entry
and slot acquisition logs include bounded account, model, limit, load, and wait
time fields; a relay-generated `429` is logged as a provider-route rate-limit
rejection so it is distinguishable from queueing.

## Request examples

### Anthropic Messages

```bash
curl http://localhost:3000/anthropic/v1/messages \
  -H 'x-api-key: tokentoxication-...' \
  -H 'content-type: application/json' \
  -d '{
    "model": "claude-sonnet-4-5",
    "max_tokens": 64,
    "messages": [{"role": "user", "content": "hello"}]
  }'
```

### OpenAI Chat

```bash
curl http://localhost:3000/openai/v1/chat/completions \
  -H 'Authorization: Bearer tokentoxication-...' \
  -H 'content-type: application/json' \
  -d '{
    "model": "deepseek-v4-pro",
    "messages": [{"role": "user", "content": "hello"}]
  }'
```

### OpenAI Responses

```bash
curl http://localhost:3000/openai/v1/responses \
  -H 'Authorization: Bearer tokentoxication-...' \
  -H 'Token-Toxication-Session-ID: example-session' \
  -H 'content-type: application/json' \
  -d '{"model": "gpt-5", "input": "hello"}'
```

### Gemini GenerateContent

```bash
curl http://localhost:3000/gemini/v1beta/models/gemini-3.1-pro-high:generateContent \
  -H 'x-goog-api-key: tokentoxication-...' \
  -H 'content-type: application/json' \
  -d '{"contents": [{"parts": [{"text": "hello"}]}]}'
```
