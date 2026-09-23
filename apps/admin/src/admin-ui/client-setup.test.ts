import { describe, expect, it } from "vite-plus/test";

import { buildClientSetupSnippets, dshModelEntryYaml, dshProviderYaml } from "./client-setup";
import { codexModelOptions, dshModelOptions } from "./helpers";
import type { ClientModelOption, DshModelOption } from "./types";
import type { ModelCatalogEntry, RoutableModelCatalogEntry } from "../types";

function catalogEntry(
  id: string,
  family: string,
  enabled = true,
  displayName = "",
): ModelCatalogEntry {
  return {
    createdAt: "2026-07-23T00:00:00Z",
    displayName,
    enabled,
    family,
    id,
  };
}

const catalog: ModelCatalogEntry[] = [
  catalogEntry("deepseek-v4-pro", "deepseek", true, "DeepSeek V4 Pro"),
  catalogEntry("qwen-max", "qwen", true, "Qwen Max"),
  catalogEntry("gpt-5", "openai", true),
  catalogEntry("claude-sonnet-4-5", "anthropic", true, "Claude Sonnet 4.5"),
  catalogEntry("gemini-2.5-pro", "gemini", true),
  catalogEntry("deepseek-disabled", "deepseek", false),
];

const routable: RoutableModelCatalogEntry[] = [
  { id: "deepseek-v4-pro", wireApi: "openai-chat" },
  { id: "qwen-max", wireApi: "openai-chat" },
  { id: "gpt-5", wireApi: "openai-responses" },
  { id: "claude-sonnet-4-5", wireApi: "anthropic-messages" },
  { id: "gemini-2.5-pro", wireApi: "gemini-generate-content" },
  { id: "deepseek-disabled", wireApi: "openai-chat" },
];

const codexModels: ClientModelOption[] = [{ id: "gpt-5", displayName: "gpt-5" }];

describe("Codex route summary", () => {
  it("includes enabled Responses routes without Chat-only models", () => {
    expect(codexModelOptions(catalog, routable)).toEqual(codexModels);
  });
});

describe("dshModelOptions", () => {
  it.each([
    ["gpt-5.6-sol", "low"],
    ["gpt-5.6-terra", "medium"],
    ["gpt-5.6-luna", "medium"],
  ])("keeps %s Responses capabilities", (id) => {
    const [model] = dshModelOptions(
      [catalogEntry(id, "other")],
      [
        { id, wireApi: "openai-responses" },
        { id, wireApi: "openai-chat" },
      ],
    );
    expect(dshModelEntryYaml(model, "responses")).toContain("input: [text, image]");
    expect(dshModelEntryYaml(model, "responses")).toContain("max: max");
  });

  it.each(["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"])(
    "does not let an incorrect family label override %s Chat capabilities",
    (id) => {
      const [model] = dshModelOptions(
        [catalogEntry(id, "deepseek")],
        [{ id, wireApi: "openai-chat" }],
      );
      expect(dshModelEntryYaml(model, "chat")).toContain("input: [text, image]");
      expect(dshModelEntryYaml(model, "chat")).not.toContain("reasoningEfforts");
      expect(dshModelEntryYaml(model, "chat")).not.toContain("thinkingFormat");
      expect(dshModelEntryYaml(model, "chat")).toContain("supportsPromptCacheKey: true");
    },
  );

  it.each([
    ["gpt-6-astra", "low"],
    ["gpt-6-sol", "medium"],
    ["gpt-6-luna", "medium"],
  ])("requires Responses for %s agent routes", (id) => {
    const models = [catalogEntry(id, "other")];
    const chat = { id, wireApi: "openai-chat" };
    expect(dshModelOptions(models, [chat])).toEqual([]);
    const [gpt6] = dshModelOptions(models, [
      chat,
      { id, wireApi: "openai-responses" },
      { id, wireApi: "anthropic-messages" },
    ]);
    expect(gpt6.protocols).toEqual({ chat: false, responses: true, anthropic: false });
    expect(dshModelEntryYaml(gpt6, "responses")).toContain("input: [text, image]");
    expect(dshModelEntryYaml(gpt6, "responses")).toContain("max: max");
  });

  it("keeps enabled models routed through a DeepSeek Harness protocol", () => {
    const options = dshModelOptions(catalog, routable);

    expect(options.map((model) => model.id)).toEqual([
      "claude-sonnet-4-5",
      "deepseek-v4-pro",
      "gpt-5",
      "qwen-max",
    ]);
    expect(options.find((model) => model.id === "deepseek-v4-pro")?.protocols).toEqual({
      chat: true,
      responses: false,
      anthropic: false,
    });
    expect(options.find((model) => model.id === "gpt-5")?.protocols.responses).toBe(true);
    expect(options.find((model) => model.id === "claude-sonnet-4-5")?.protocols.anthropic).toBe(
      true,
    );
  });

  it("passes the catalog family through for reasoning dispatch", () => {
    const options = dshModelOptions(catalog, routable);

    expect(options.find((model) => model.id === "deepseek-v4-pro")?.family).toBe("deepseek");
    expect(options.find((model) => model.id === "qwen-max")?.family).toBe("qwen");
  });
});

describe("DeepSeek Harness settings snippet", () => {
  const dshModels = dshModelOptions(catalog, routable);

  function snippetsFor(origin = "http://relay.example:3000") {
    return buildClientSetupSnippets({
      apiKey: "tokentoxication-test",
      serviceOrigin: origin,
      opencodeModels: [],
      piModels: [],
      dshModels,
    });
  }

  it("enables a stable session body field for generated Chat providers", () => {
    const snippets = snippetsFor();

    expect(snippets.dsh).toContain("token-toxication-chat:");
    expect(snippets.dsh).toContain("supportsPromptCacheKey: true");
  });

  it("emits one provider route per routed protocol with the relay base URLs", () => {
    const snippets = snippetsFor();

    expect(snippets.dsh).toContain("token-toxication-chat:");
    expect(snippets.dsh).toContain('displayName: "Token Toxication"');
    expect(snippets.dsh).not.toContain("Token Toxication Chat");
    expect(snippets.dsh).toContain("api: openai-completions");
    expect(snippets.dsh).toContain("baseURL: http://relay.example:3000/openai/v1");
    expect(snippets.dsh).toContain("token-toxication-responses:");
    expect(snippets.dsh).toContain("api: openai-responses");
    expect(snippets.dsh).toContain("token-toxication-anthropic:");
    expect(snippets.dsh).toContain("api: anthropic-messages");
    expect(snippets.dsh).toContain("baseURL: http://relay.example:3000/anthropic");
    expect(snippets.dsh).not.toContain("gemini");
    expect(snippets.dsh).not.toContain("deepseek-disabled");
  });

  it("emits a secret-free settings.yaml fragment without shell commands", () => {
    const snippets = snippetsFor();

    expect(snippets.dsh).toMatch(/^# Set TOKEN_TOXICATION_API_KEY/);
    expect(snippets.dsh).not.toContain("export TOKEN_TOXICATION_API_KEY");
    expect(snippets.dsh).not.toContain("tokentoxication-test");
    expect(snippets.dsh).toContain("\nllm-pi-ai:\n");
  });

  it("declares protocol-specific reasoning efforts for DeepSeek and OpenAI models", () => {
    const snippets = snippetsFor();

    expect(snippets.dsh).toContain(
      [
        '        - id: "deepseek-v4-pro"',
        '          name: "DeepSeek V4 Pro"',
        "          reasoningEfforts:",
        "            off:",
        "            high: high",
        "            max: max",
        "          compat:",
        "            thinkingFormat: deepseek",
      ].join("\n"),
    );
    expect(snippets.dsh).toContain(
      [
        '        - id: "gpt-5"',
        "          input: [text, image]",
        "          reasoningEfforts:",
        "            minimal: minimal",
        "            low: low",
        "            medium: medium",
        "            high: high",
      ].join("\n"),
    );
    // A non-deepseek Chat model carries no reasoning override.
    expect(snippets.dsh).toContain('        - id: "qwen-max"\n          name: "Qwen Max"');
    expect(snippets.dsh.match(/thinkingFormat: deepseek/g)).toHaveLength(1);
    const responsesProvider = snippets.dsh
      .split("token-toxication-responses:")[1]
      ?.split("token-toxication-anthropic:")[0];
    expect(responsesProvider).not.toContain("thinkingFormat:");
  });

  it("preserves the harness's existing default model", () => {
    expect(snippetsFor().dsh).not.toContain("agent-default-model:");
  });

  it("omits protocols with no routable models", () => {
    const chatOnly: DshModelOption[] = [
      {
        id: "deepseek-v4-pro",
        displayName: "DeepSeek V4 Pro",
        family: "deepseek",
        protocols: { chat: true, responses: false, anthropic: false },
      },
    ];
    const snippets = buildClientSetupSnippets({
      apiKey: "tokentoxication-test",
      serviceOrigin: "http://relay.example:3000",
      opencodeModels: [],
      piModels: [],
      dshModels: chatOnly,
    });

    expect(snippets.dsh).toContain("token-toxication-chat:");
    expect(snippets.dsh).not.toContain("token-toxication-responses:");
    expect(snippets.dsh).not.toContain("token-toxication-anthropic:");
  });
});

describe("Codex provider configuration snippet", () => {
  it("configures the provider while leaving models and Codex capabilities to the client", () => {
    const snippets = buildClientSetupSnippets({
      apiKey: "tokentoxication-test",
      serviceOrigin: "http://relay.example:3000",
      opencodeModels: [],
      piModels: [],
      dshModels: [],
    });

    expect(snippets.codexEnvironment).toBe(
      "export TOKEN_TOXICATION_API_KEY='tokentoxication-test'",
    );
    expect(snippets.codexConfig).toContain('model_provider = "token-toxication"');
    expect(snippets.codexConfig).toContain('env_key = "TOKEN_TOXICATION_API_KEY"');
    expect(snippets.codexConfig).not.toContain("tokentoxication-test");
    expect(snippets.codexConfig).not.toMatch(/^\s*(?:model|model_catalog_json)\s*=/m);
    expect(snippets.codexConfig).not.toMatch(
      /^\s*(?:use_responses_lite|tool_mode|multi_agent_version)\s*=/m,
    );
    expect(snippets.codexConfig).not.toContain("--profile");
  });
});

describe("client model selection", () => {
  it("leaves default model choice with Claude Code, opencode, and DeepSeek Harness", () => {
    const snippets = buildClientSetupSnippets({
      apiKey: "tokentoxication-test",
      serviceOrigin: "https://relay.example",
      opencodeModels: [{ id: "gpt-6-astra", displayName: "Astra", wireApi: "openai-responses" }],
      piModels: [],
      dshModels: dshModelOptions(catalog, routable),
    });
    expect(snippets.claudeCode).toContain("CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=1");
    expect(snippets.claudeCode).not.toContain("ANTHROPIC_MODEL=");
    const json = JSON.parse(snippets.opencode.split("<<'JSON'\n")[1].split("\nJSON")[0]);
    expect(json.provider["token-toxication"].models).toHaveProperty("gpt-6-astra");
    expect(json).not.toHaveProperty("model");
    expect(json).not.toHaveProperty("small_model");
    expect(snippets.dsh).toContain("token-toxication-responses:");
    expect(snippets.dsh).not.toContain("agent-default-model:");
  });
});

describe("DeepSeek Harness YAML builders", () => {
  const deepseekChat: DshModelOption = {
    id: "deepseek-v4-pro",
    displayName: "DeepSeek V4 Pro",
    family: "deepseek",
    protocols: { chat: true, responses: false, anthropic: false },
  };
  const plainResponses: DshModelOption = {
    id: "gpt-5",
    displayName: "",
    family: "openai",
    protocols: { chat: false, responses: true, anthropic: false },
  };
  const deepseekMultiProtocol: DshModelOption = {
    id: "deepseek-v4-pro",
    displayName: "DeepSeek V4 Pro",
    family: "deepseek",
    protocols: { chat: true, responses: true, anthropic: true },
  };
  const gpt55Responses: DshModelOption = {
    id: "gpt-5.5",
    displayName: "GPT-5.5",
    family: "openai",
    protocols: { chat: false, responses: true, anthropic: false },
  };
  const gpt41Responses: DshModelOption = {
    id: "gpt-4.1",
    displayName: "GPT-4.1",
    family: "openai",
    protocols: { chat: false, responses: true, anthropic: false },
  };
  const gpt56OtherFamily: DshModelOption = {
    id: "gpt-5.6-luna",
    displayName: "gpt-5.6 (Luna)",
    family: "other",
    protocols: { chat: false, responses: true, anthropic: false },
  };

  it("indents model entries under the provider models list", () => {
    const provider = dshProviderYaml(
      "chat",
      [deepseekChat],
      "http://relay.example:3000/openai/v1",
      "http://relay.example:3000/anthropic",
    );

    expect(provider).toContain('        - id: "deepseek-v4-pro"');
    expect(provider).toContain("          reasoningEfforts:");
    expect(provider).toContain("            off:");
    expect(provider).toContain("          compat:");
    expect(provider).toContain("            thinkingFormat: deepseek");
  });

  it("omits the display name when it duplicates the id", () => {
    const entry = dshModelEntryYaml(plainResponses, "responses");

    expect(entry).toContain('- id: "gpt-5"');
    expect(entry).not.toContain("name:");
  });

  it("marks known non-reasoning OpenAI models explicitly", () => {
    const entry = dshModelEntryYaml(gpt41Responses, "responses");

    expect(entry).toContain("input: [text, image]");
    expect(entry).toContain("reasoningEfforts: false");
  });

  it("uses exact capabilities when the catalog family is generic", () => {
    const entry = dshModelEntryYaml(gpt56OtherFamily, "responses");

    expect(entry).toContain("input: [text, image]");
    expect(entry).toContain("reasoningEfforts:");
    expect(entry).toContain("  max: max");
  });

  it("renders OpenAI wire spellings without Chat compatibility fields", () => {
    const entry = dshModelEntryYaml(gpt55Responses, "responses");

    expect(entry).toContain("reasoningEfforts:");
    expect(entry).toContain("  low: low");
    expect(entry).toContain("  xhigh: xhigh");
    expect(entry).not.toContain("off:");
    expect(entry).not.toContain("max:");
    expect(entry).not.toContain("compat:");
  });

  it("keeps DeepSeek reasoning compatibility on Chat provider entries", () => {
    const chat = dshProviderYaml(
      "chat",
      [deepseekMultiProtocol],
      "http://relay.example:3000/openai/v1",
      "http://relay.example:3000/anthropic",
    );
    expect(chat).toContain("reasoningEfforts:");
    expect(chat).toContain("thinkingFormat: deepseek");

    for (const protocol of ["responses", "anthropic"] as const) {
      const provider = dshProviderYaml(
        protocol,
        [deepseekMultiProtocol],
        "http://relay.example:3000/openai/v1",
        "http://relay.example:3000/anthropic",
      );
      expect(provider).not.toContain("reasoningEfforts:");
      expect(provider).not.toContain("thinkingFormat: deepseek");
    }
  });
});
