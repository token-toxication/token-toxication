# Routing and providers

[Back to the project README](../README.md)

## Routing model

Every relay request resolves through three records:

| Record | Responsibility |
| --- | --- |
| Provider account | Owns the upstream base URL, credential, authentication mode, wire protocol, priority, and account health |
| Catalog model | Defines the exact, case-preserving model ID exposed to clients |
| Provider model route | Maps a public model to one account and upstream model ID, with a primary or backup role |

There is no global upstream URL. Each account owns its connection settings, and each route can remove configured top-level request fields before forwarding.

Routing policy uses the top-level model ID and configured `stripParams`. It does not inspect nested prompts, messages, or input content. Provider adapters handle protocol-specific transformations separately.

Eligible primary routes are selected before backup routes. Within the same role, higher-priority accounts are preferred and least-recently-used routes rotate first.

Each request receives one upstream attempt. A failure does not trigger another provider in the same request; later requests skip accounts or routes that are blocked or cooling down.

### Wire protocols

| Wire API | Upstream request |
| --- | --- |
| `anthropic-messages` | `{base_url}/v1/messages` |
| `openai-chat` | `{base_url}/chat/completions` |
| `openai-responses` | `{base_url}/v1/responses` |
| `gemini-generate-content` | Gemini Code Assist GenerateContent or streaming endpoint |

If a base URL already ends with the required API version, the relay avoids duplicating that version segment.

## Provider setup

The admin interface loads provider presets from the backend. Presets supply the expected base URL, authentication mode, wire protocol, credential guidance, and provider aliases.

| Wire protocol | Included presets |
| --- | --- |
| Anthropic Messages | Anthropic, MiniMax, MiniMax Token Plan, and China-region MiniMax variants |
| OpenAI Responses | OpenAI API key and Codex subscription |
| OpenAI Chat | DeepSeek v4, Qwen, Kimi, Moonshot AI, Z.AI, Zhipu AI, coding-plan variants, and China-region variants |
| Gemini GenerateContent | Antigravity OAuth |

### OpenAI API keys

Choose **OpenAI API key**, keep `https://api.openai.com` as the base URL, and use the `openai-responses` wire protocol. Add catalog models and routes for the exact upstream model IDs you intend to expose.

### Codex subscriptions

Create one **Codex subscription** account per ChatGPT Plus or Pro subscription. Use `https://chatgpt.com/backend-api`, `codex-oauth`, and the `openai-responses` wire protocol.

Paste only the raw refresh token. Codex CLI stores it in `~/.codex/auth.json` at `tokens.refresh_token`; opencode stores it in `~/.local/share/opencode/auth.json` at `openai.refresh`.

Subscription requests are sent to `{base_url}/codex/responses`, while quota data comes from `{base_url}/wham/usage`. Legacy base URLs ending in `/codex` or `/codex/responses` are normalized automatically.

Codex subscription routes remove `max_output_tokens` to match the Codex CLI request shape. OpenAI API-key routes continue forwarding that field.

Configure Codex clients with base URL `http://localhost:3000/openai/v1`, the Responses API, and a generated relay key.

### OpenAI-compatible providers

Use `openai-chat` for providers such as DeepSeek, Qwen, Kimi, Moonshot AI, Z.AI, and Zhipu AI. Create exact catalog IDs and map each route to the model ID expected by its account.

OpenAI-compatible clients use base URL `http://localhost:3000/openai/v1` and change only the requested model name.

### Anthropic-compatible providers

Use `anthropic-messages` for Anthropic, MiniMax, and MiniMax token-plan accounts. The built-in presets include global and China-region MiniMax base URLs.

Anthropic-compatible clients use base URL `http://localhost:3000/anthropic`.

### Gemini with Antigravity OAuth

Set `TT_ANTIGRAVITY_OAUTH_CLIENT_SECRET`, choose **Antigravity OAuth**, and select **Sign in with Antigravity** in the admin interface.

The backend runs the PKCE OAuth flow, receives the loopback callback at `/oauth-callback`, and stores the resulting tokens without returning them to the browser.

A public OAuth client ID is built in. Set `TT_ANTIGRAVITY_OAUTH_CLIENT_ID` only when using a different registered client.

After sign-in, inspect the account's available models and quota data. Create routes with the exact upstream IDs returned for that account rather than relying on a fixed model alias.

Gemini clients may send native `contents`, `generationConfig`, `safetySettings`, `tools`, and `toolConfig` fields. Client authentication is consumed by the relay and is not forwarded to Google.
