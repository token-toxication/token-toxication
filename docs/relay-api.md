# Relay API

[Back to the project README](../README.md)

## Endpoints

| Protocol | Method | Endpoint | Recommended client base URL |
| --- | --- | --- | --- |
| Anthropic Messages | `POST` | `/anthropic/v1/messages` | `http://localhost:3000/anthropic` |
| OpenAI Chat | `POST` | `/openai/v1/chat/completions` | `http://localhost:3000/openai/v1` |
| OpenAI Responses | `POST` | `/openai/v1/responses` | `http://localhost:3000/openai/v1` |
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
completed relay attempt. Non-success attempts also emit a structured journal
record with the same transport, status, outcome, and retry count. These fields
are metadata-only; prompts, tokens, and upstream bodies are never logged.

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
