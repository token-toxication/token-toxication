# Client setup

[Back to the project README](../README.md)

The admin **Client Setup** page generates copy-ready configuration from enabled, eligible model routes. Codex, opencode, Pi, and DSH configuration files reference the relay key through `TOKEN_TOXICATION_API_KEY`. Shell export snippets can contain the entered key: treat them as credentials and do not publish them.

## Supported clients

| Client | Route selection | Generated configuration |
| --- | --- | --- |
| Codex | OpenAI Responses | Provider base URL and API-key environment reference; Codex owns model selection |
| Claude Code | Anthropic Messages | Anthropic-compatible relay settings and native model discovery |
| opencode | OpenAI Chat or Responses | The matching AI SDK and routed models, without a default override |
| Pi | OpenAI Responses | A complete `~/.pi/agent/models.json` file |
| DeepSeek Harness | Chat, Responses, and Anthropic Messages | `llm-pi-ai` routes for `$DSH_HOME/settings.yaml`, without changing the agent default |

Pi setup replaces its complete models file. Back up an existing `~/.pi/agent/models.json` before applying the generated content.

Client Setup does not choose a model for Codex, Claude Code, opencode, or
DeepSeek Harness. Select a routable model in the client after adding its
provider. For Claude Code, keep an eligible route for its native default or
select a model using its own picker. opencode's generated project file lists
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

This setup targets the latest Codex reference inspected at commit `c44deff7b1`.
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
4. If migrating from an earlier Token Toxication setup, remove its
   `model_catalog_json = "~/.codex/token-toxication-model-catalog.json"`
   entry. The old catalog file can then be removed after backing it up. Review
   any `model = ...` entry that came from the old snippet; keep or remove it
   according to your own preference. Do not remove unrelated catalog settings
   without reviewing them.

Codex's built-in model picker may list a model without a matching relay route.
Create the route before selecting that model; an unroutable request fails
rather than silently switching models. The relay does not claim that every
upstream supports the features declared by Codex's model catalog.

### Protocol compatibility

Current Codex GPT-6 profiles use Responses Lite and Code Mode; Codex controls
these model-level choices. The relay forwards the explicit Lite header and
request layout, including `additional_tools` and developer messages. It
rejects malformed Lite markers and never retries a Lite request as ordinary
Responses. Qualify all candidate upstream routes with the intended protocol
before using them; isolated mock tests do not establish live upstream support.

The provider TOML defaults to HTTP/SSE. To use Responses WebSockets, confirm
all candidate routes support the transport, then set `supports_websockets = true`
inside `[model_providers.token-toxication]` according to your Codex version.
The first request binds a connection to one model, route, and account;
there is no cross-connection response resume or automatic replay. See relay
transport tests for connection lifecycle and error handling.

The opt-in client test uses a real Codex binary and an isolated relay/mock;
run `just codex-compat /absolute/path/to/matching/codex`. A local build from
`c44deff7b1` (`codex-cli 0.0.0`, source build) passed the test without a
configured default model or static catalog: the built-in picker listed Astra
and the default model completed a Lite Code Mode tool round trip. The
system-installed `codex-cli 0.147.0` is older and was not used for this
claim. The test does not use production credentials or prove a live
upstream's Lite eligibility.

## DeepSeek Harness

The generated fragment adds one provider route for each eligible protocol.
Merge it into the existing `$DSH_HOME/settings.yaml` rather than replacing
unrelated settings. It does not write `agent-default-model`; the harness
retains its existing selection until changed in the client.

Model capabilities are generated conservatively:

- DeepSeek-family Chat models declare `thinkingFormat: deepseek` and their supported reasoning levels.
- Recognized GPT and OpenAI o-series models use exact model IDs, even when the catalog family remains `other`.
- Models available through multiple upstream account types use only the reasoning levels safe across those routes.
- Known multimodal models declare `input: [text, image]`.
- Known non-reasoning models declare `reasoningEfforts: false`.
- Unknown model IDs remain undeclared instead of inheriting guessed capabilities.

The exact `gpt-6-astra`, `gpt-6-sol`, and `gpt-6-luna` IDs are offered to DSH only on Responses routes, which provide their coding-tool contract. Chat-only and Anthropic-only routes are not offered. All three declare image input and the `low`, `medium`, `high`, `xhigh`, and `max` reasoning effort options. The generated fragment does not set a default model or reasoning effort. These IDs do not inherit the older models' prefix/date normalization.

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
