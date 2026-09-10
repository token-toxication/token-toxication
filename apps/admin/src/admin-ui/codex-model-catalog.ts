import { CODEX_ASTRA_INSTRUCTIONS } from "./codex-astra-instructions";
import { CODEX_GPT56_INSTRUCTIONS } from "./codex-gpt56-instructions";
import type { ClientModelOption } from "./types";

type ReasoningEffort = "low" | "medium" | "high" | "xhigh" | "max";

type CodexModelProfile = {
  supported_reasoning_levels: { effort: ReasoningEffort; description: string }[];
  default_reasoning_level?: ReasoningEffort;
  shell_type: "shell_command" | "unified_exec";
  support_verbosity: boolean;
  default_verbosity: "low" | null;
  apply_patch_tool_type: "freeform" | null;
  truncation_policy: { mode: "bytes" | "tokens"; limit: number };
  supports_parallel_tool_calls: boolean;
  experimental_supported_tools: string[];
  input_modalities: ("text" | "image")[];
  supports_image_detail_original?: boolean;
  context_window?: number;
  max_context_window?: number;
  default_reasoning_summary?: "none";
  use_responses_lite?: boolean;
  web_search_tool_type?: "text_and_image";
  supports_reasoning_summary_parameter?: boolean;
  effective_context_window_percent?: number;
  include_skills_usage_instructions?: boolean;
  include_apps_usage_instructions?: boolean;
  include_plugin_usage_instructions?: boolean;
  node_repl_auto_review_required?: boolean;
  supports_experimental_context?: boolean;
  supports_search_tool?: boolean;
};

const fallbackProfile: CodexModelProfile & { base_instructions: string } = {
  supported_reasoning_levels: [],
  shell_type: "shell_command",
  base_instructions: "You are Codex, a coding agent.",
  support_verbosity: false,
  default_verbosity: null,
  apply_patch_tool_type: null,
  truncation_policy: { mode: "bytes", limit: 10_000 },
  supports_parallel_tool_calls: false,
  experimental_supported_tools: [],
  input_modalities: ["text"],
};

// Capabilities offered by this application's ordinary Responses coding profile.
// Context values are client limits, not a guarantee about arbitrary upstreams.
// Ultra, Lite, code mode, experimental context, and multi-agent need separate
// transport/harness validation; do not enable them by copying the full catalog.
const codingProfile: CodexModelProfile = {
  supported_reasoning_levels: [
    { effort: "low", description: "Fast responses with lighter reasoning" },
    { effort: "medium", description: "Balances speed and reasoning depth" },
    { effort: "high", description: "Greater reasoning depth for complex problems" },
    { effort: "xhigh", description: "Extra high reasoning depth for complex problems" },
    { effort: "max", description: "Maximum reasoning depth for the hardest problems" },
  ],
  shell_type: "unified_exec",
  support_verbosity: true,
  default_verbosity: "low",
  apply_patch_tool_type: "freeform",
  truncation_policy: { mode: "tokens", limit: 10_000 },
  supports_parallel_tool_calls: true,
  experimental_supported_tools: [],
  input_modalities: ["text", "image"],
  supports_image_detail_original: true,
  context_window: 272_000,
  max_context_window: 872_000,
  default_reasoning_summary: "none",
  use_responses_lite: false,
  web_search_tool_type: "text_and_image",
  supports_reasoning_summary_parameter: true,
  effective_context_window_percent: 95,
  include_skills_usage_instructions: false,
  supports_experimental_context: false,
  // Deferred tool discovery is separate from hosted web search.
  supports_search_tool: false,
};

type KnownModelProfile = CodexModelProfile & {
  base_instructions: string;
  default_reasoning_level: ReasoningEffort;
  include_apps_usage_instructions: boolean;
  include_plugin_usage_instructions: boolean;
  node_repl_auto_review_required: boolean;
};

const modelProfiles = new Map<string, KnownModelProfile>([
  [
    "gpt-6-astra",
    {
      ...codingProfile,
      base_instructions: CODEX_ASTRA_INSTRUCTIONS,
      default_reasoning_level: "low",
      include_apps_usage_instructions: false,
      include_plugin_usage_instructions: false,
      node_repl_auto_review_required: true,
    },
  ],
  [
    "gpt-5.6-sol",
    {
      ...codingProfile,
      base_instructions: CODEX_GPT56_INSTRUCTIONS,
      default_reasoning_level: "low",
      include_apps_usage_instructions: true,
      include_plugin_usage_instructions: true,
      node_repl_auto_review_required: false,
    },
  ],
  [
    "gpt-5.6-terra",
    {
      ...codingProfile,
      base_instructions: CODEX_GPT56_INSTRUCTIONS,
      default_reasoning_level: "medium",
      include_apps_usage_instructions: true,
      include_plugin_usage_instructions: true,
      node_repl_auto_review_required: false,
    },
  ],
  [
    "gpt-5.6-luna",
    {
      ...codingProfile,
      base_instructions: CODEX_GPT56_INSTRUCTIONS,
      default_reasoning_level: "medium",
      include_apps_usage_instructions: true,
      include_plugin_usage_instructions: true,
      node_repl_auto_review_required: false,
    },
  ],
]);

export type CodexAdvancedOptions = {
  responsesLite?: boolean;
  codeMode?: boolean;
  multiAgent?: boolean;
};

type AgentProfile = { version: "v1" } | { version: "v2"; effort: ReasoningEffort };
const agentProfiles = new Map<string, AgentProfile>([
  ["gpt-6-astra", { version: "v2", effort: "xhigh" }],
  ["gpt-5.6-sol", { version: "v2", effort: "max" }],
  ["gpt-5.6-terra", { version: "v2", effort: "max" }],
  ["gpt-5.6-luna", { version: "v1" }],
]);

export function supportsAdvancedCodexProfile(id: string) {
  return modelProfiles.has(id) && agentProfiles.has(id);
}

function advancedProfile(id: string, options: CodexAdvancedOptions = {}) {
  const profile = modelProfiles.get(id);
  const agents = agentProfiles.get(id);
  if (!profile || !agents) return {};
  return {
    ...(options.responsesLite ? { use_responses_lite: true } : {}),
    ...(options.codeMode ? { tool_mode: "code_mode_only" } : {}),
    ...(options.multiAgent
      ? {
          multi_agent_version: agents.version,
          ...(agents.version === "v2"
            ? {
                supported_reasoning_levels: [
                  ...profile.supported_reasoning_levels,
                  {
                    effort: "ultra",
                    description: "Multi-agent mode using the model's supported reasoning effort",
                  },
                ],
                multi_agent_reasoning_effort: agents.effort,
              }
            : {}),
        }
      : {}),
  };
}

export function codexModelCatalogJson(
  models: ClientModelOption[],
  advanced: Record<string, CodexAdvancedOptions> = {},
) {
  return JSON.stringify(
    {
      models: models.map((model, index) => ({
        slug: model.id,
        display_name: model.displayName,
        description: "Routed through Token Toxication using the OpenAI Responses API.",
        visibility: "list",
        supported_in_api: true,
        priority: index + 1,
        availability_nux: null,
        upgrade: null,
        ...(modelProfiles.get(model.id) ?? fallbackProfile),
        ...advancedProfile(model.id, advanced[model.id]),
      })),
    },
    null,
    2,
  );
}
