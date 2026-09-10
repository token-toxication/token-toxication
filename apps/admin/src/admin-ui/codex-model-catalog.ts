import { CODEX_ASTRA_INSTRUCTIONS } from "./codex-astra-instructions";
import type { ClientModelOption } from "./types";

type ReasoningEffort = "low" | "medium" | "high" | "xhigh" | "max";

type CodexModelProfile = {
  supported_reasoning_levels: { effort: ReasoningEffort; description: string }[];
  default_reasoning_level?: ReasoningEffort;
  shell_type: "shell_command" | "unified_exec";
  base_instructions: string;
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
};

const fallbackProfile: CodexModelProfile = {
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

// Astra capabilities offered by this application's ordinary Responses profile.
// Context values are client limits, not a guarantee about arbitrary upstreams.
// Ultra, Lite, code mode, experimental context, and multi-agent need separate
// transport/harness validation; do not enable them by copying the full catalog.
const astraProfile: CodexModelProfile = {
  supported_reasoning_levels: [
    { effort: "low", description: "Fast responses with lighter reasoning" },
    { effort: "medium", description: "Balances speed and reasoning depth" },
    { effort: "high", description: "Greater reasoning depth for complex problems" },
    { effort: "xhigh", description: "Extra high reasoning depth for complex problems" },
    { effort: "max", description: "Maximum reasoning depth for the hardest problems" },
  ],
  default_reasoning_level: "low",
  shell_type: "unified_exec",
  base_instructions: CODEX_ASTRA_INSTRUCTIONS,
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
};

export function codexModelCatalogJson(models: ClientModelOption[]) {
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
        ...(model.id === "gpt-6-astra" ? astraProfile : fallbackProfile),
      })),
    },
    null,
    2,
  );
}
