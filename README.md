# Token Toxication

[![CI](https://github.com/token-toxication/token-toxication/actions/workflows/ci.yml/badge.svg)](https://github.com/token-toxication/token-toxication/actions/workflows/ci.yml)

**A self-hosted control plane and relay for Anthropic, OpenAI, and Gemini-compatible AI providers.**

Token Toxication gives clients one stable API surface while operators manage provider credentials, exact model mappings, primary and backup routes, health state, request logs, and token usage from a web console.

The application ships as a Rust server with an embedded React admin interface and SQLite persistence.

## Features

| Capability | Description |
| --- | --- |
| Multi-protocol relay | Anthropic Messages, OpenAI Chat Completions, OpenAI Responses, and Gemini GenerateContent endpoints |
| Model routing | Exact public-to-upstream model mappings with primary and backup routes |
| Provider management | Per-account credentials, base URLs, wire protocols, priority, and health state |
| Reliability controls | Account blocking, route cooldowns, and automatic selection of the next eligible route |
| Usage analytics | Provider-reported input, cached-input, and output token accounting |
| Privacy-conscious logs | Operational metadata without prompts, messages, credentials, or raw upstream bodies |
| Self-hosted administration | API-key management, provider setup, route configuration, quotas, and request logs |
| Client setup | Generated configurations for Codex, Claude Code, opencode, Pi, and DeepSeek Harness |
| Production TLS | Plain HTTP, certificate files, or managed ACME HTTP-01 certificates |

## Quick start

### Prerequisites

- Rust 1.95 or newer
- [just](https://github.com/casey/just)
- Vite+ with the `vp` command available
- `openapi-nexus` for SDK generation

### Run locally

```bash
just ui-install
just sdk-generate
just ui-build

TT_BIND_ADDR=127.0.0.1:3000 \
TT_ADMIN_PASSWORD='choose-a-strong-password' \
just dev-server
```

Open [http://localhost:3000](http://localhost:3000) and sign in as `admin` with the password supplied through `TT_ADMIN_PASSWORD`.

> [!IMPORTANT]
> Set a strong admin password before exposing the service. Provider credentials are stored in the local SQLite database, so protect `data/` and its backups.

### Configure the first model

Use the admin interface to create these records in order:

1. A **provider account** containing the upstream URL, protocol, and credential.
2. A **catalog model** containing the public model ID clients will request.
3. A **provider model route** mapping that public ID to the upstream model ID.
4. A **relay API key** for client authentication.

The model becomes discoverable after the catalog entry, route, and provider account are enabled and eligible.

Open **Client Setup** to generate configuration for a supported client. See the [client setup guide](docs/client-setup.md) for route selection, credentials, reasoning levels, and image input support.

## Documentation

| Guide | Contents |
| --- | --- |
| [Client setup](docs/client-setup.md) | Generated client configuration, credentials, and model capabilities |
| [Relay API](docs/relay-api.md) | Endpoints, authentication, model discovery, and request examples |
| [Routing and providers](docs/routing-and-providers.md) | Routing records, selection policy, wire protocols, and provider-specific setup |
| [Operations](docs/operations.md) | Reliability, request-log privacy, usage accounting, configuration, and TLS |
| [Development](docs/development.md) | Repository layout, prerequisites, generation, tests, and build commands |

The OpenAPI document is served at `/openapi.json` and can also be regenerated locally with `just openapi-generate`.
