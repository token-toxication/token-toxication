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
| `/metrics` | JSON totals for API keys, provider health, and usage |
| `/openapi.json` | OpenAPI document for the admin and relay APIs |

## Authentication

Relay API keys use the configured prefix, `tokentoxication-` by default. Send a key through any supported client convention:

- `Authorization: Bearer <key>`
- `x-api-key: <key>`
- `x-goog-api-key: <key>`
- `api-key: <key>`
- Gemini's `?key=<key>` query parameter

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
