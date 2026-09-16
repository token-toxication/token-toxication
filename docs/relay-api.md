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
| `/metrics` | JSON totals for API keys, provider health, usage, bounded session-affinity counters, and route-selection counters |
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

The relay never retries the current request on a different route. OpenAI `previous_response_id` values are forwarded unchanged and are not used as the affinity key.

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
