import { describe, expect, it } from "vite-plus/test";

import {
  buildClientSetupSnippets,
  codexModelCatalogJson,
  dshDefaultModelYaml,
  dshModelEntryYaml,
  dshProviderYaml,
} from "./client-setup";
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

describe("Codex static model catalog", () => {
  it("includes every enabled Responses route and no Chat-only model", () => {
    expect(codexModelOptions(catalog, routable)).toEqual(codexModels);
  });

  it("generates a conservative catalog that Codex can use without model discovery", () => {
    const catalog = JSON.parse(codexModelCatalogJson(codexModels));

    expect(catalog.models).toHaveLength(1);
    expect(catalog.models[0]).toMatchObject({
      slug: "gpt-5",
      display_name: "gpt-5",
      visibility: "list",
      supported_in_api: true,
      input_modalities: ["text"],
    });
    expect(catalog.models[0].supported_reasoning_levels).toEqual([]);
  });
});

describe("dshModelOptions", () => {
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

  function snippetsFor(model: string, origin = "http://relay.example:3000") {
    return buildClientSetupSnippets({
      apiKey: "tokentoxication-test",
      serviceOrigin: origin,
      codexModel: "gpt-5",
      codexModels,
      claudeModel: "claude-sonnet-4-5",
      opencodeModel: "",
      opencodeModels: [],
      piModels: [],
      dshModel: model,
      dshModels,
    });
  }

  it("emits one provider route per routed protocol with the relay base URLs", () => {
    const snippets = snippetsFor("deepseek-v4-pro");

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
    const snippets = snippetsFor("deepseek-v4-pro");

    expect(snippets.dsh).toMatch(/^# Set TOKEN_TOXICATION_API_KEY/);
    expect(snippets.dsh).not.toContain("export TOKEN_TOXICATION_API_KEY");
    expect(snippets.dsh).not.toContain("tokentoxication-test");
    expect(snippets.dsh).toContain("\nllm-pi-ai:\n");
  });

  it("declares protocol-specific reasoning efforts for DeepSeek and OpenAI models", () => {
    const snippets = snippetsFor("deepseek-v4-pro");

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

  it("points the default model at its protocol route", () => {
    const deepseek = snippetsFor("deepseek-v4-pro");
    expect(deepseek.dsh).toContain(
      [
        "agent-default-model:",
        "  provider: token-toxication-chat",
        '  model: "deepseek-v4-pro"',
        "  reasoningEffort: max",
      ].join("\n"),
    );

    const responses = snippetsFor("gpt-5");
    expect(responses.dsh).toContain(
      [
        "agent-default-model:",
        "  provider: token-toxication-responses",
        '  model: "gpt-5"',
        "  reasoningEffort: high",
      ].join("\n"),
    );

    const anthropic = snippetsFor("claude-sonnet-4-5");
    expect(anthropic.dsh).toContain("provider: token-toxication-anthropic");
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
      codexModel: "gpt-5",
      codexModels,
      claudeModel: "claude-sonnet-4-5",
      opencodeModel: "",
      opencodeModels: [],
      piModels: [],
      dshModel: "deepseek-v4-pro",
      dshModels: chatOnly,
    });

    expect(snippets.dsh).toContain("token-toxication-chat:");
    expect(snippets.dsh).not.toContain("token-toxication-responses:");
    expect(snippets.dsh).not.toContain("token-toxication-anthropic:");
  });
});

describe("Codex default configuration snippet", () => {
  it("writes the static catalog and configures the base config without a profile", () => {
    const snippets = buildClientSetupSnippets({
      apiKey: "tokentoxication-test",
      serviceOrigin: "http://relay.example:3000",
      codexModel: "gpt-5",
      codexModels,
      claudeModel: "claude-sonnet-4-5",
      opencodeModel: "",
      opencodeModels: [],
      piModels: [],
      dshModel: "",
      dshModels: [],
    });

    expect(snippets.codexCatalog).toContain("token-toxication-model-catalog.json");
    expect(snippets.codexCatalog).toContain('slug": "gpt-5"');
    expect(snippets.codexConfig).toContain(
      'model_catalog_json = "~/.codex/token-toxication-model-catalog.json"',
    );
    expect(snippets.codexConfig).toContain('model_provider = "token-toxication"');
    expect(snippets.codexConfig).not.toContain("--profile");
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
    expect(dshDefaultModelYaml(gpt41Responses)).not.toContain("reasoningEffort:");
  });

  it("uses exact capabilities when the catalog family is generic", () => {
    const entry = dshModelEntryYaml(gpt56OtherFamily, "responses");

    expect(entry).toContain("input: [text, image]");
    expect(entry).toContain("reasoningEfforts:");
    expect(entry).toContain("  max: max");
    expect(dshDefaultModelYaml(gpt56OtherFamily)).toContain("reasoningEffort: medium");
  });

  it("renders OpenAI wire spellings without Chat compatibility fields", () => {
    const entry = dshModelEntryYaml(gpt55Responses, "responses");

    expect(entry).toContain("reasoningEfforts:");
    expect(entry).toContain("  low: low");
    expect(entry).toContain("  xhigh: xhigh");
    expect(entry).not.toContain("off:");
    expect(entry).not.toContain("max:");
    expect(entry).not.toContain("compat:");
    expect(dshDefaultModelYaml(gpt55Responses)).toContain("reasoningEffort: medium");
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

  it("defaults a model routed on one protocol to that protocol's provider", () => {
    expect(dshDefaultModelYaml(plainResponses)).toBe(
      [
        "agent-default-model:",
        "  provider: token-toxication-responses",
        '  model: "gpt-5"',
        "  reasoningEffort: high",
      ].join("\n"),
    );
  });
});
