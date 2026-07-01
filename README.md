# Token Toxication

Token Toxication is a Rust + React relay service. It provides self-hosted
Anthropic Messages, OpenAI Responses, and OpenAI Chat relay endpoints,
admin-managed API keys, provider account rotation, request logging, and a Vite+
shadcn dashboard.

## Layout

- `crates/token-toxication-server` - Axum backend and SQLite persistence.
- `apps/admin` - React + Vite+ + shadcn admin UI.
- `.github/workflows/ci.yml` - Rust and Vite+ CI.

## Quick Start

```bash
cd apps/admin && vp install
cd ../..
just sdk-generate
just ui-build
cargo run -p token-toxication-server --bin token-toxication-server
```

Open `http://localhost:3000` and sign in with the configured admin username and
password. Add a provider account, catalog model, and provider model route before
using the relay.

Anthropic Messages relay:

```bash
curl http://localhost:3000/anthropic/v1/messages \
  -H "x-api-key: tokentoxication-..." \
  -H "content-type: application/json" \
  -d '{"model":"claude-sonnet-4-5","max_tokens":64,"messages":[{"role":"user","content":"hello"}]}'
```

OpenAI Chat relay, suitable for DeepSeek v4 accounts:

```bash
curl http://localhost:3000/openai/v1/chat/completions \
  -H "Authorization: Bearer tokentoxication-..." \
  -H "content-type: application/json" \
  -d '{"model":"deepseek-v4-pro","messages":[{"role":"user","content":"hello"}]}'
```

OpenAI Responses relay, suitable for Codex:

```bash
curl http://localhost:3000/openai/v1/responses \
  -H "Authorization: Bearer tokentoxication-..." \
  -H "content-type: application/json" \
  -d '{"model":"gpt-5","input":"hello"}'
```

Model discovery uses the same relay API key authentication. Concrete model names
come from enabled catalog entries with at least one active provider model route:

```bash
curl http://localhost:3000/openai/v1/models \
  -H "Authorization: Bearer tokentoxication-..."
```

## Provider Accounts and Model Routes

Each provider account has a `wire_api` protocol:

- `anthropic-messages` forwards to `{base_url}/v1/messages`.
- `openai-chat` forwards to `{base_url}/chat/completions`.
- `openai-responses` forwards to `{base_url}/v1/responses`.

For Anthropic-compatible clients, use base URL `http://localhost:3000/anthropic`.

There is no global upstream base URL. Each provider account owns its base URL,
auth mode, wire protocol, priority, and health state. Model names live in the
model catalog, and provider model routes bind each public model name to a
provider account plus exact upstream model name.

For OpenAI-compatible chat providers such as Qwen, Kimi, GLM, MiniMax, and
DeepSeek, create provider accounts with `openai-chat`, add catalog models such
as `deepseek-v4-pro`, `MiniMax-M3`, or `glm-5.2`, then create primary and backup
routes for those models. Clients can keep using `/openai/v1/chat/completions`
and switch only the model name.

Provider model routes are exact and case-preserving. If a client sends
`MiniMax-M3`, Token Toxication looks up that exact catalog model and rewrites the
forwarded JSON body to the selected route's `upstreamModelId`. If the primary
route's provider account is blocked or disabled, the next request uses an
eligible backup route. It does not retry another provider inside the same
request.

For Codex, add an OpenAI provider account with base URL
`https://api.openai.com`, Bearer auth, and `openai-responses`. Configure Codex
with a custom provider using base URL `http://localhost:3000/openai/v1`, wire API
`responses`, and the generated `tokentoxication-...` key.

For DeepSeek v4, add a DeepSeek provider account with base URL
`https://api.deepseek.com`, Bearer auth, and `openai-chat`. Use
`deepseek-v4-flash` or `deepseek-v4-pro` as the client model. OpenAI-compatible
clients should use base URL `http://localhost:3000/openai/v1`.

## Configuration

Service configuration is available as CLI flags, environment variables, or
defaults. Run `token-toxication-server --help` for the full flag and env mapping.
Tracing reads the conventional `RUST_LOG` environment variable and falls back to
an info-level server filter. Provider account API keys are stored in the local
SQLite database, so keep `data/` private.

## OpenAPI and SDK Generation

The backend emits the ignored `openapi/token-toxication.openapi.json` with
`utoipa`.
The ignored admin UI SDK under `apps/admin/src/generated/token-toxication` is
generated with `openapi-nexus`.

```bash
just sdk-generate
```
