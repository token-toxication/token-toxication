# Client setup

[Back to the project README](../README.md)

The admin **Client Setup** page generates copy-ready configuration from enabled, eligible model routes. Codex, opencode, Pi, and DSH configuration files reference the relay key through `TOKEN_TOXICATION_API_KEY`. Shell export snippets can contain the entered key: treat them as credentials and do not publish them.

## Supported clients

| Client | Route selection | Generated configuration |
| --- | --- | --- |
| Codex | OpenAI Responses | Static model catalog, relay base URL, model, and API-key environment reference |
| Claude Code | Anthropic Messages | Anthropic-compatible relay settings |
| opencode | OpenAI Chat or Responses | The matching AI SDK and routed models |
| Pi | OpenAI Responses | A complete `~/.pi/agent/models.json` file |
| DeepSeek Harness | Chat, Responses, and Anthropic Messages | `llm-pi-ai` routes for `$DSH_HOME/settings.yaml` |

Pi setup replaces its complete models file. Back up an existing `~/.pi/agent/models.json` before applying the generated content.

## Codex

1. Configure an enabled public model and an enabled OpenAI Responses provider route.
2. Back up `~/.codex/config.toml` and any existing `~/.codex/token-toxication-model-catalog.json`.
3. Copy the generated catalog setup, reviewing the key export before running it. The catalog command replaces that catalog file only.
4. Merge the separate TOML fragment into `~/.codex/config.toml`. Replace matching keys and the `token-toxication` provider table instead of appending duplicates. Keep unrelated settings.
5. Ensure `TOKEN_TOXICATION_API_KEY` is available to the process launching Codex, then start a new session.

The catalog is an authoritative static file, not an overlay on Codex's bundled models. Regenerate it after changing eligible routes or upgrading Token Toxication's model profiles. Updating the server alone does not update files already copied to clients. To roll back, restore the backed-up catalog and TOML configuration and start a new session; no database migration is required.

### GPT-6 Astra

The exact public ID `gpt-6-astra` receives the ordinary Responses profile defined in [codex-model-catalog.ts](../apps/admin/src/admin-ui/codex-model-catalog.ts). Make sure every upstream route for that public ID actually targets Astra and supports these capabilities.

| Capability | Generated setting |
| --- | --- |
| Reasoning | `low`, `medium`, `high`, `xhigh`, `max`; default `low` |
| Input | Text and images; original image detail capability |
| Verbosity | Supported; default `low` |
| Tools | Unified exec, freeform apply_patch, parallel tool calls |
| Tool output truncation | 10,000 tokens |
| Context | 272,000 default, 872,000 maximum configurable tokens |
| Reasoning summary | Omitted by default |
| Instructions | Complete workspace, task-scope, persistence, validation, and communication guidance |

Context sizes describe Codex's client configuration, not a guarantee about arbitrary upstream deployments. Codex reserves headroom, so the default usable context is 258,400 tokens. Larger configured contexts are bounded by the profile's maximum. Actual long-context inference and automatic compaction have not been live-qualified by the mock tests.

Unknown IDs retain the existing conservative text-only profile and short fallback instructions. Display names, provider family labels, custom aliases, provider prefixes, and dated Astra aliases do not select the Astra profile. The relay still routes those IDs normally; their client capabilities are not inferred from the upstream name.

The setup does not overwrite an explicit `model_reasoning_effort`. If a previous configuration specifies `none` or `minimal`, change it to a supported Astra effort such as `low`. Existing user instruction overrides also remain authoritative. The relay does not inject or rewrite the caller's `instructions`.

Responses Lite, Ultra, code-mode-only, experimental context management, and multi-agent v2 are not enabled by this profile. There is no WebSocket upgrade or mid-turn steering support in the relay. Ultra is a Codex orchestration mode, not a wire effort that other clients should forward literally. This profile does not introduce Fast/priority processing defaults.

For other clients, use Responses for tool calling and omit unsupported sampling parameters. The relay preserves client parameters except for configured route `stripParams` and the existing Codex OAuth `max_output_tokens` removal; it does not silently repair every invalid Astra request.

### GPT-5.6 Sol, Terra, and Luna

The exact public IDs below receive complete ordinary Responses coding profiles
from [codex-model-catalog.ts](../apps/admin/src/admin-ui/codex-model-catalog.ts).
Sol, Terra, and Luna share their own standalone instructions in
[codex-gpt56-instructions.ts](../apps/admin/src/admin-ui/codex-gpt56-instructions.ts).
Astra uses separate instructions in
[codex-astra-instructions.ts](../apps/admin/src/admin-ui/codex-astra-instructions.ts).

The GPT-5.6 instructions retain explicit task-type boundaries, workspace editing
and destructive-action rules, a structured skill workflow, and concise collaborative
communication. Astra emphasizes carrying forward authorization, completing reviewable
preparation before requesting approval, continuity across user steering and compaction,
connected prose, and judgment about optional skills. Both remain bounded by user
authorization and the tools available in the session. Neither template enables
persistent mode, automatic review, or multi-agent orchestration. These are
product-owned instructions for ordinary Responses, not an entire client runtime.

| Public model ID | Default reasoning | Available reasoning |
| --- | --- | --- |
| `gpt-5.6-sol` | `low` | `low`, `medium`, `high`, `xhigh`, `max` |
| `gpt-5.6-terra` | `medium` | `low`, `medium`, `high`, `xhigh`, `max` |
| `gpt-5.6-luna` | `medium` | `low`, `medium`, `high`, `xhigh`, `max` |

All three profiles declare text/image input, original image detail, low verbosity,
unified exec, freeform apply_patch, parallel tool calls, and 10,000-token tool output
truncation. The default context is 272,000 tokens, with a configurable maximum of
872,000; these are client limits, not live upstream guarantees. Reasoning summaries
are omitted by default. Lite, Ultra, code mode, multi-agent orchestration, and
WebSockets are not enabled by these profiles.

Regenerate an existing static catalog to receive these settings. Explicit user
reasoning and instruction overrides still take precedence. The `gpt-5.6` family
alias, provider-prefixed IDs, dated variants, and custom aliases keep the conservative
Codex fallback; display labels do not select a profile. Routing eligibility is unchanged.

DSH retains its existing model recognition and protocol handling. Exact GPT-5.6
IDs take precedence over a conflicting family label. Responses entries
offer images and the reasoning levels/defaults above; when both Chat and Responses
routes exist, the default selects Responses. This change does not add reasoning or
agent-tool guarantees to Chat routes.

### Capability boundaries

The four coding profiles explicitly select text-and-image hosted web search when
the client enables search, accept an explicit reasoning summary while omitting it
by default, and reserve 5% of the configured context for overhead. Search availability
still depends on the client provider and upstream route; declaring its request shape
does not establish that an arbitrary upstream can execute it.

| Client policy | Astra | Sol, Terra, Luna |
| --- | --- | --- |
| Additional app usage instructions | Disabled | Enabled when apps are available |
| Additional plugin usage instructions | Disabled | Enabled when plugins are available |
| Additional skill usage instructions | Disabled | Disabled |
| Node REPL automatic review requirement | Required if that tool is enabled | No model-specific requirement |

These flags do not install integrations, enable Node REPL, or bypass normal
permission checks. Deferred tool discovery is a separate capability from hosted
web search and remains disabled in these profiles. Experimental context management
and Responses Lite also remain explicitly disabled. No paid speed tier is selected
or advertised. Client-version gates, compaction compatibility hashes, advanced tool
modes, and account-plan metadata are not copied into this static relay catalog.
The tested client versions below qualify the ordinary Responses subset only.

### Compatibility checks

The suite covers Astra, Sol, Terra, and Luna separately: default reasoning, each
of the five explicit reasoning levels, a bounded context override, and hosted web
search with an explicit summary. Web search is exercised as a request contract
against the mock, not as a live search service.

The isolated mock suite has passed with Codex CLI **0.146.0** and **0.153.4** on macOS. These are tested versions, not a promise that every version between them, or every future version, is compatible. The test starts the real relay, uses its admin API to create temporary routes, loads the generated catalog in Codex, checks the model picker, and completes shell and patch tool round trips. It also checks image input, default/explicit reasoning, and runtime context limits. No production credentials or remote model responses are used.

Run the opt-in suite with an absolute path to a Codex binary:

```bash
just codex-compat /absolute/path/to/codex
```

This builds the relay, uses temporary configuration and a disposable workspace, and removes test data afterward. It does not change your usual Codex configuration. `just ci` runs the regular contract tests but skips this external-binary suite unless explicitly enabled. A passing mock suite establishes client/relay compatibility, not model quality, instruction-following quality, live account availability, or production readiness. Real API-key and Codex OAuth upstream smoke tests remain separate.

## DeepSeek Harness

The generated fragment adds one provider route for each eligible protocol and selects the preferred routed model as the default. Merge the fragment into the existing `$DSH_HOME/settings.yaml` rather than replacing unrelated settings.

Model capabilities are generated conservatively:

- DeepSeek-family Chat models declare `thinkingFormat: deepseek` and their supported reasoning levels.
- Recognized GPT and OpenAI o-series models use exact model IDs, even when the catalog family remains `other`.
- Models available through multiple upstream account types use only the reasoning levels safe across those routes.
- Known multimodal models declare `input: [text, image]`.
- Known non-reasoning models declare `reasoningEfforts: false`.
- Unknown model IDs remain undeclared instead of inheriting guessed capabilities.

The exact `gpt-6-astra` ID is offered only on Responses routes because DSH requires tool calling. A Chat-only Astra route is not offered; when both routes exist, the generated default uses Responses. Astra declares image input and `low`, `medium`, `high`, `xhigh`, and `max`, defaulting to `low`. It does not offer `off`, `none`, `minimal`, or `ultra`, and it does not inherit the older models' prefix/date normalization.

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

The selected default model also receives a supported `reasoningEffort` value.

## Credentials

Set `TOKEN_TOXICATION_API_KEY` in the client process environment. Do not paste relay keys into generated files, commit them, or expose them in screenshots and logs.
