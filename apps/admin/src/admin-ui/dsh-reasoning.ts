import type { DshModelOption, DshProtocol } from "./types";

export type DshReasoningEffortId = "off" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max";

export type DshReasoningEffort = {
  id: DshReasoningEffortId;
  wireValue?: string;
};

export type DshReasoningProfile = {
  efforts: readonly DshReasoningEffort[];
  defaultEffort: DshReasoningEffortId;
  thinkingFormat?: "deepseek";
};

const effort = (id: Exclude<DshReasoningEffortId, "off">): DshReasoningEffort => ({
  id,
  wireValue: id,
});

const offByOmission: DshReasoningEffort = { id: "off" };
const offAsNone: DshReasoningEffort = { id: "off", wireValue: "none" };
const minimal = effort("minimal");
const low = effort("low");
const medium = effort("medium");
const high = effort("high");
const xhigh = effort("xhigh");
const max = effort("max");

const deepseekProfile: DshReasoningProfile = {
  efforts: [offByOmission, high, max],
  defaultEffort: "max",
  thinkingFormat: "deepseek",
};

const gpt5Profile: DshReasoningProfile = {
  efforts: [minimal, low, medium, high],
  defaultEffort: "high",
};
const gpt5ProProfile: DshReasoningProfile = {
  efforts: [high],
  defaultEffort: "high",
};
const gpt51Profile: DshReasoningProfile = {
  efforts: [offAsNone, low, medium, high],
  defaultEffort: "high",
};
const gpt52Profile: DshReasoningProfile = {
  efforts: [offAsNone, low, medium, high, xhigh],
  defaultEffort: "xhigh",
};
const gpt52ChatProfile: DshReasoningProfile = {
  efforts: [medium, xhigh],
  defaultEffort: "xhigh",
};
const openAiProProfile: DshReasoningProfile = {
  efforts: [medium, high, xhigh],
  defaultEffort: "xhigh",
};
const codexProfile: DshReasoningProfile = {
  efforts: [low, medium, high, xhigh],
  defaultEffort: "medium",
};
// Codex also advertises ultra on Sol and Terra, but DSH's canonical effort
// interface ends at max. Keep max mapped to max, matching pi-ai's catalog.
const codex56LowProfile: DshReasoningProfile = {
  efforts: [low, medium, high, xhigh, max],
  defaultEffort: "low",
};
const codex56MediumProfile: DshReasoningProfile = {
  efforts: [low, medium, high, xhigh, max],
  defaultEffort: "medium",
};
const realtimeProfile: DshReasoningProfile = {
  efforts: [minimal, low, medium, high, xhigh],
  defaultEffort: "xhigh",
};
const oSeriesProfile: DshReasoningProfile = {
  efforts: [low, medium, high],
  defaultEffort: "high",
};

// Mirrors pi-ai's OpenAI catalog and Codex's bundled model catalog. Models
// available from both keep the safe intersection because routing may choose
// either backend. Exact IDs prevent new variants from inheriting unsupported
// levels by family alone.
const openAiReasoningProfiles = new Map<string, DshReasoningProfile>([
  ...["gpt-5", "gpt-5-mini", "gpt-5-nano"].map((id) => [id, gpt5Profile] as const),
  ["gpt-5-pro", gpt5ProProfile],
  ["gpt-5.1", gpt51Profile],
  ["gpt-5.2", codexProfile],
  ["gpt-5.2-chat-latest", gpt52ChatProfile],
  ["gpt-5.2-pro", openAiProProfile],
  ["gpt-5.3-codex", codexProfile],
  ["gpt-5.3-codex-spark", codexProfile],
  ...["gpt-5.4", "gpt-5.4-mini", "gpt-5.5", "codex-auto-review"].map(
    (id) => [id, codexProfile] as const,
  ),
  ["gpt-5.4-nano", gpt52Profile],
  ...["gpt-5.4-pro", "gpt-5.5-pro"].map((id) => [id, openAiProProfile] as const),
  ["gpt-5.6-sol", codex56LowProfile],
  ...["gpt-5.6-luna", "gpt-5.6-terra"].map((id) => [id, codex56MediumProfile] as const),
  ["gpt-realtime-2.1", realtimeProfile],
  ...["o1", "o1-pro", "o3", "o3-mini", "o3-pro", "o4-mini"].map(
    (id) => [id, oSeriesProfile] as const,
  ),
]);

const openAiNonReasoningModels = new Set([
  "gpt-4",
  "gpt-4-turbo",
  "gpt-4.1",
  "gpt-4.1-mini",
  "gpt-4.1-nano",
  "gpt-4o",
  "gpt-4o-mini",
  "gpt-5-chat-latest",
  "gpt-5.3-chat-latest",
]);

function normalizedModelId(value: string) {
  const id = value.trim().toLowerCase().split("/").at(-1) ?? "";
  return id.replace(/-\d{4}-\d{2}-\d{2}$/, "");
}

/**
 * Return the reasoning levels DSH may safely offer for one generated route.
 * Unknown models stay undeclared instead of receiving guessed wire values.
 */
export function dshReasoningProfile(
  model: DshModelOption,
  protocol: DshProtocol,
): DshReasoningProfile | false | undefined {
  const family = model.family.trim().toLowerCase();
  if (protocol === "chat" && family === "deepseek") {
    return deepseekProfile;
  }
  if (protocol !== "responses" || family !== "openai") {
    return undefined;
  }
  const id = normalizedModelId(model.id);
  return openAiReasoningProfiles.get(id) ?? (openAiNonReasoningModels.has(id) ? false : undefined);
}
