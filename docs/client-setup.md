# Client setup

[Back to the project README](../README.md)

The admin **Client Setup** page generates copy-ready configuration from enabled, eligible model routes. Generated files reference the relay key through `TOKEN_TOXICATION_API_KEY`; they never include a literal key.

## Supported clients

| Client | Route selection | Generated configuration |
| --- | --- | --- |
| Codex | OpenAI Responses | Relay base URL, model, and API-key environment reference |
| Claude Code | Anthropic Messages | Anthropic-compatible relay settings |
| opencode | OpenAI Chat or Responses | The matching AI SDK and routed models |
| Pi | OpenAI Responses | A complete `~/.pi/agent/models.json` file |
| DeepSeek Harness | Chat, Responses, and Anthropic Messages | `llm-pi-ai` routes for `$DSH_HOME/settings.yaml` |

Pi setup replaces its complete models file. Back up an existing `~/.pi/agent/models.json` before applying the generated content.

## DeepSeek Harness

The generated fragment adds one provider route for each eligible protocol and selects the preferred routed model as the default. Merge the fragment into the existing `$DSH_HOME/settings.yaml` rather than replacing unrelated settings.

Model capabilities are generated conservatively:

- DeepSeek-family Chat models declare `thinkingFormat: deepseek` and their supported reasoning levels.
- Recognized GPT and OpenAI o-series models use exact model IDs, even when the catalog family remains `other`.
- Models available through multiple upstream account types use only the reasoning levels safe across those routes.
- Known multimodal models declare `input: [text, image]`.
- Known non-reasoning models declare `reasoningEfforts: false`.
- Unknown model IDs remain undeclared instead of inheriting guessed capabilities.

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
