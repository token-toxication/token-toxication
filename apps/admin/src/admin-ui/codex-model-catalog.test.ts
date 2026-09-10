import { describe, expect, it } from "vite-plus/test";

import { CODEX_ASTRA_INSTRUCTIONS } from "./codex-astra-instructions";
import { CODEX_GPT56_INSTRUCTIONS } from "./codex-gpt56-instructions";
import { codexModelCatalogJson } from "./codex-model-catalog";
import { codexModelOptions } from "./helpers";

describe("Codex model profiles", () => {
  it("keeps family instructions independent in a mixed catalog", () => {
    const ids = ["gpt-5.6-terra", "unknown", "gpt-6-astra", "gpt-5.6-sol", "gpt-5.6-luna"];
    const catalog = JSON.parse(
      codexModelCatalogJson(ids.map((id) => ({ id, displayName: "same label" }))),
    );
    expect(
      catalog.models.map((model: { base_instructions: string }) => model.base_instructions),
    ).toEqual([
      CODEX_GPT56_INSTRUCTIONS,
      "You are Codex, a coding agent.",
      CODEX_ASTRA_INSTRUCTIONS,
      CODEX_GPT56_INSTRUCTIONS,
      CODEX_GPT56_INSTRUCTIONS,
    ]);
    expect(CODEX_ASTRA_INSTRUCTIONS).not.toBe(CODEX_GPT56_INSTRUCTIONS);
    expect(CODEX_GPT56_INSTRUCTIONS).toContain("# Destructive actions");
    expect(CODEX_ASTRA_INSTRUCTIONS).toContain("# Authorization and preparation");
    for (const instructions of [CODEX_ASTRA_INSTRUCTIONS, CODEX_GPT56_INSTRUCTIONS]) {
      expect(instructions).not.toMatch(
        /https?:\/\/|\/Volumes\/|\/Users\/|\{\{|functions\.|clock\./,
      );
    }
  });

  it.each([
    ["gpt-5.6-sol", "low"],
    ["gpt-5.6-terra", "medium"],
    ["gpt-5.6-luna", "medium"],
  ])("gives %s its coding capabilities and %s default", (id, defaultEffort) => {
    const {
      models: [profile],
    } = JSON.parse(codexModelCatalogJson([{ id, displayName: id }]));
    expect(profile).toMatchObject({
      slug: id,
      default_reasoning_level: defaultEffort,
      include_apps_usage_instructions: true,
      include_plugin_usage_instructions: true,
      node_repl_auto_review_required: false,
      web_search_tool_type: "text_and_image",
      supports_reasoning_summary_parameter: true,
      effective_context_window_percent: 95,
      supports_experimental_context: false,
      supports_search_tool: false,
      include_skills_usage_instructions: false,
      base_instructions: CODEX_GPT56_INSTRUCTIONS,
      shell_type: "unified_exec",
      apply_patch_tool_type: "freeform",
      input_modalities: ["text", "image"],
      supports_image_detail_original: true,
      context_window: 272_000,
      max_context_window: 872_000,
      default_reasoning_summary: "none",
      use_responses_lite: false,
      support_verbosity: true,
      default_verbosity: "low",
      supports_parallel_tool_calls: true,
      truncation_policy: { mode: "tokens", limit: 10_000 },
    });
    expect(
      profile.supported_reasoning_levels.map((level: { effort: string }) => level.effort),
    ).toEqual(["low", "medium", "high", "xhigh", "max"]);
    expect(profile).not.toHaveProperty("tool_mode");
    expect(profile).not.toHaveProperty("multi_agent_version");
  });

  it.each(["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"])(
    "does not infer %s capabilities from aliases or labels",
    (id) => {
      for (const alias of [
        "gpt-5.6",
        `openai/${id}`,
        `${id}-2026-09-10`,
        id.toUpperCase(),
        "custom",
      ]) {
        const {
          models: [profile],
        } = JSON.parse(codexModelCatalogJson([{ id: alias, displayName: id }]));
        expect(profile.supported_reasoning_levels).toEqual([]);
        expect(profile.input_modalities).toEqual(["text"]);
        expect(profile.base_instructions).toBe("You are Codex, a coding agent.");
        expect(profile).not.toHaveProperty("context_window");
      }
    },
  );

  it("uses the Astra profile without changing public IDs, labels, or order", () => {
    const models = [
      { id: "custom-astra", displayName: "GPT-6 Astra" },
      { id: "gpt-6-astra", displayName: 'Astra "relay"' },
    ];
    const catalog = JSON.parse(codexModelCatalogJson(models));
    expect(catalog.models.map((entry: { slug: string }) => entry.slug)).toEqual([
      "custom-astra",
      "gpt-6-astra",
    ]);
    expect(catalog.models[1]).toMatchObject({
      slug: "gpt-6-astra",
      display_name: 'Astra "relay"',
      priority: 2,
      default_reasoning_level: "low",
      include_apps_usage_instructions: false,
      include_plugin_usage_instructions: false,
      node_repl_auto_review_required: true,
      web_search_tool_type: "text_and_image",
      supports_reasoning_summary_parameter: true,
      effective_context_window_percent: 95,
      supports_experimental_context: false,
      supports_search_tool: false,
      include_skills_usage_instructions: false,
      base_instructions: CODEX_ASTRA_INSTRUCTIONS,
      shell_type: "unified_exec",
      apply_patch_tool_type: "freeform",
      input_modalities: ["text", "image"],
      supports_image_detail_original: true,
      support_verbosity: true,
      default_verbosity: "low",
      default_reasoning_summary: "none",
      context_window: 272_000,
      max_context_window: 872_000,
      truncation_policy: { mode: "tokens", limit: 10_000 },
      supports_parallel_tool_calls: true,
      experimental_supported_tools: [],
      use_responses_lite: false,
    });
    expect(
      catalog.models[1].supported_reasoning_levels.map((level: { effort: string }) => level.effort),
    ).toEqual(["low", "medium", "high", "xhigh", "max"]);
    expect(catalog.models[1]).not.toHaveProperty("tool_mode");
    expect(catalog.models[1]).not.toHaveProperty("multi_agent_version");
    expect(codexModelCatalogJson(models)).toBe(codexModelCatalogJson(models));
  });

  it.each(["custom-astra", "gpt-6-astra-2026-09-10", "openai/gpt-6-astra", "GPT-6-ASTRA"])(
    "keeps unverified ID %s conservative even with an Astra display name",
    (id) => {
      const catalog = JSON.parse(codexModelCatalogJson([{ id, displayName: "GPT-6 Astra" }]));
      expect(catalog.models[0]).toMatchObject({
        slug: id,
        supported_reasoning_levels: [],
        shell_type: "shell_command",
        base_instructions: "You are Codex, a coding agent.",
        input_modalities: ["text"],
        truncation_policy: { mode: "bytes", limit: 10_000 },
      });
      expect(catalog.models[0]).not.toHaveProperty("context_window");
    },
  );

  it("preserves Responses eligibility instead of adding unrouted known models", () => {
    const models = ["gpt-6-astra", "unknown", "disabled", "chat-only"].map((id) => ({
      id,
      displayName: id,
      family: "other",
      enabled: id !== "disabled",
      createdAt: "2026-09-10T00:00:00Z",
    }));
    const routes = [
      { id: "unknown", wireApi: "openai-responses" },
      { id: "disabled", wireApi: "openai-responses" },
      { id: "chat-only", wireApi: "openai-chat" },
    ];
    expect(codexModelOptions(models, routes)).toEqual([{ id: "unknown", displayName: "unknown" }]);
    expect(
      codexModelOptions(models, [...routes, { id: "gpt-6-astra", wireApi: "openai-responses" }]),
    ).toEqual([
      { id: "gpt-6-astra", displayName: "gpt-6-astra" },
      { id: "unknown", displayName: "unknown" },
    ]);
  });
});
