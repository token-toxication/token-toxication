# Client setup

[Back to the project README](../README.md)

The admin **Client Setup** page generates copy-ready configuration from enabled, eligible model routes. Codex, opencode, Pi, and DSH configuration files reference the relay key through `TOKEN_TOXICATION_API_KEY`. Shell export snippets can contain the entered key: treat them as credentials and do not publish them.

## Supported clients

| Client | Route selection | Generated configuration |
| --- | --- | --- |
| Codex | OpenAI Responses | Provider base URL and API-key environment reference; Codex owns model selection |
| Claude Code | Anthropic Messages | Anthropic-compatible relay settings and native model discovery |
| opencode | OpenAI Chat or Responses | Global config and plugin with a static fallback that refreshes `/openai/v1/models` and `/openai/v1/codex/models` every five minutes |
| Pi | OpenAI Responses | Static fallback plus a provider extension that refreshes `/openai/v1/codex/models` every five minutes |
| DeepSeek Harness | Chat, Responses, and Anthropic Messages | `llm-pi-ai` routes for `$DSH_HOME/settings.yaml`, without changing the agent default |

Pi setup replaces its complete models file and writes a trusted provider extension
under `~/.pi/agent/extensions/`. Back up an existing `~/.pi/agent/models.json`
before applying the generated content and review the extension before enabling it.

Client Setup does not choose a model for Codex, Claude Code, opencode, or
DeepSeek Harness. Select a routable model in the client after adding its
provider. For Claude Code, keep an eligible route for its native default or
select a model using its own picker. opencode's generated global config lists
the routed models but leaves its `model` and `small_model` preferences unset.
The DeepSeek Harness fragment preserves any existing `agent-default-model`;
select a new default in the harness if needed. Existing client configuration
and environment overrides may still pin a model until you change them.

## Session affinity

The relay recognizes the stable session fields emitted by the generated Client Setup targets:

- Codex and opencode: `session-id`, with Responses metadata and `prompt_cache_key` fallbacks.
- Pi and DeepSeek Harness: the provider-specific session headers/body fields produced by pi-ai.
- Claude Code: `X-Claude-Code-Session-Id` or the session component in `metadata.user_id`.

Custom clients should send `Token-Toxication-Session-ID` exactly as written (without an `X-` prefix). The explicit header has highest precedence. Missing affinity does not reject a request, but selection becomes weighted random and the server emits a rate-limited warning for recognizable Client Setup clients.

## Codex

This setup targets the Codex reference inspected at commit `6cc7d9cc5c702c0ca133f7ea48432166e0b880be`.
Codex owns its model catalog, instructions, reasoning options, Responses Lite,
Code Mode, and multi-agent behavior. The relay setup supplies only a provider
endpoint and an environment-based relay key; it does not select a default model
or replace Codex's catalog.

1. Configure enabled public model IDs and OpenAI Responses routes for the
   Codex models you intend to use. Each candidate route, including fallback
   accounts, must support the request protocol sent by Codex. The routable
   model list on Client Setup is a route summary, not a replacement model picker.
2. Set `TOKEN_TOXICATION_API_KEY` in the environment that starts Codex. Keep
   the secret out of `config.toml` and shared screenshots.
3. Merge the generated provider TOML into `~/.codex/config.toml`. Keep your
   own `model` and reasoning preferences. Select models through Codex.
4. If migrating from an earlier Token Toxication setup, remove stale hand-edited
   model entries and review any `model = ...` entry that came from the old
   snippet. Keep or remove it according to your own preference. Do not remove
   unrelated settings without reviewing them.

### Codex Desktop catalog configuration

Codex Desktop has a client-side compatibility gate: it only recognizes a custom
provider catalog when `model_catalog_json` is present, but it can then use
`model_catalog_url` for the actual dynamic catalog. The Client Setup page
generates both the download command and the provider block. Run its Codex
environment snippet first; it downloads the current TT catalog into
`~/.codex/token-toxication-model-catalog.json` using the relay key, so users do
not need to write or maintain JSON by hand.

The generated provider block has this shape:

```toml
[model_providers.token-toxication]
name = "Token Toxication"
base_url = "https://tokentoxication.sensecore-maas.dev/openai/v1"
env_key = "TOKEN_TOXICATION_API_KEY"
model_catalog_json = "token-toxication-model-catalog.json"
model_catalog_url = "https://tokentoxication.sensecore-maas.dev/openai/v1/codex/models"
```

`model_catalog_json` is only the Desktop compatibility gate; TT's dynamic
endpoint remains the source of model metadata. Re-run the generated download
command when the Desktop version changes or when the account's available models
change.

Measured against the Desktop's bundled core `0.163.0-aarch64-apple-darwin`
(`~/Library/.../codex`), with the same client, key, and relay:

| Catalog configuration | Desktop model picker |
| --- | --- |
| `model_catalog_url` only | empty |
| `model_catalog_json` plus TT `model_catalog_url` | lists TT models |
| Neither field | lists bundled models |

The two-field configuration is a Desktop compatibility workaround, not a second
source of truth. The empty-picker behavior is a client-side regression; TT's
catalog endpoint remains valid and works directly for CLI clients.

`GET /openai/v1/codex/models` remains available and is unaffected for CLI
clients; see the [relay API](relay-api.md) for what it serves.

Codex's built-in model picker may list a model without a matching relay route.
Create the route before selecting that model; an unroutable request fails
rather than silently switching models.

Fast is a service tier on a model, not a separate model. The account catalog
declares `priority` as an available tier, Codex offers it on the model, and the
relay forwards it unchanged as `service_tier = "priority"`.

The tier a request starts with comes from the model's `default_service_tier`,
which the Codex TUI honours and `codex exec` does not. Setting `service_tier`
in `config.toml` applies it to every request that model supports, for both:

```toml
service_tier = "fast"
```

Both `fast` and `priority` are accepted and reach the relay as
`service_tier = "priority"`.

### Protocol compatibility

Current Codex GPT-6 profiles use Responses Lite and Code Mode; Codex controls
these model-level choices. The relay forwards the explicit Lite header and
request layout, including `additional_tools` and developer messages. It
rejects malformed Lite markers and never retries a Lite request as ordinary
Responses. Qualify all candidate upstream routes with the intended protocol
before using them; isolated mock tests do not establish live upstream support.

The generated provider TOML enables `supports_websockets = true` so matching
Codex versions prefer the Responses WebSocket path. Keep the setting only when
all candidate routes support the transport; older Codex versions continue to
use the HTTP/SSE path.
The first request binds a connection to one model, route, and account;
there is no cross-connection response resume or automatic replay. See relay
transport tests for connection lifecycle and error handling.

The opt-in client test uses a real Codex binary and an isolated relay/mock;
run `just codex-compat /absolute/path/to/matching/codex`. The installed
`codex-cli 0.162.1` was used to verify the built-in catalog's Lite round trip, the
relay-served account catalog, a Fast turn on a catalog model, and a `-fast`
request resolved to its base model. The test does not use production credentials
or prove a live upstream's Lite eligibility.

## Harness Fast and Ultrafast

For GPT routes that support service tiers, Client Setup generates independent
model ids for every harness:

```text
gpt-6.1-sol
gpt-6.1-sol-fast
gpt-6.1-sol-ultrafast
```

TT routes the suffixed ids to `gpt-6.1-sol` and sends the native
`service_tier` value (`priority` for `-fast`, `ultrafast` for `-ultrafast`).
This keeps the configuration usable by clients that only select models by id.

## opencode

The generated global `~/.config/opencode/opencode.jsonc` configures the relay
endpoint and writes `~/.config/opencode/plugins/token-toxication-models.ts`.
The shell snippet uses `OPENCODE_CONFIG_DIR` when it is already set, otherwise
it follows `XDG_CONFIG_HOME` and defaults to `~/.config/opencode`. Back up an
existing global OpenCode config before running it. The plugin source is carried
as a base64 payload and decoded locally so shell copy/paste handling cannot
rewrite TypeScript punctuation. The plugin embeds the
current model map as a fallback, uses the relay key from
`TOKEN_TOXICATION_API_KEY`, fetches the Chat list and Codex Responses catalog,
preserves the `-fast`/`-ultrafast` aliases, and refreshes every five minutes.
Responses models use the catalog's `context_window` value for OpenCode's context
limit instead of a fixed client-side estimate.
If a refresh fails, the last successful list (or the generated fallback on
first launch) remains available.

The plugin is loaded through opencode's automatic global plugin discovery.
This setup targets the inspected opencode reference at commit
`8ffbcc2c81d1ff558618137204c36954797d7a6f` (`vscode-v1.0.0-5-g8ffbcc2c81`).

## Pi

The generated `~/.pi/agent/models.json` remains an offline fallback. The setup
also writes `~/.pi/agent/extensions/token-toxication-models.mjs`; Pi loads it at
startup, fetches the Codex Responses catalog, refreshes it every five minutes, and
keeps the last successful list during transient failures. This setup targets the
inspected Pi reference at commit `f1b2e77f5b13b2a199b1052cb79c235451afe7d7`
(`v1.1.0-11-gf1b2e77f5`).

## DeepSeek Harness

The generated fragment adds one provider route for each eligible protocol.
Merge it into the existing `$DSH_HOME/settings.yaml` rather than replacing
unrelated settings. It does not write `agent-default-model`; the harness
retains its existing selection until changed in the client.

The inspected DSH reference at commit `d743267388641bc76f17c45ce8b4c231aed1d32c`
(`dsh-v0.2.1-alpha.2`) supports model discovery through **Fetch available
models** in Settings → Models, but its persistent provider schema has no
startup `modelCatalogUrl` or polling field. The generated relay URL is therefore
usable for that action, while the YAML model list remains static until DSH adds
an automatic catalog-refresh hook.

Model capabilities are generated conservatively:

- DeepSeek-family Chat models declare `thinkingFormat: deepseek` and their supported reasoning levels.
- Recognized GPT and OpenAI o-series models use exact model IDs, even when the catalog family remains `other`.
- Models available through multiple upstream account types use only the reasoning levels safe across those routes.
- Known multimodal models declare `input: [text, image]`.
- Known non-reasoning models declare `reasoningEfforts: false`.
- Unknown model IDs remain undeclared instead of inheriting guessed capabilities.

The exact `gpt-6-astra`, `gpt-6.1-sol`, `gpt-6-sol`, and `gpt-6-luna` IDs are offered to DSH only on Responses routes, which provide their coding-tool contract. Chat-only and Anthropic-only routes are not offered. These models declare image input and the `low`, `medium`, `high`, `xhigh`, and `max` reasoning effort options. The generated fragment does not set a default model or reasoning effort. These IDs do not inherit the older models' prefix/date normalization.

For example, a recognized multimodal reasoning model receives both declarations:

```yaml
- id: "gpt-5.6-luna"
  input: [text, image]
  reasoningEfforts:
    low: low
    medium: medium
    high: high
    xhigh: xhigh
    max: max
```

Reasoning effort options are declared for each eligible model. Client model
selection and the default effort are managed by the harness.

## Credentials

Set `TOKEN_TOXICATION_API_KEY` in the client process environment. Do not paste relay keys into generated files, commit them, or expose them in screenshots and logs.
