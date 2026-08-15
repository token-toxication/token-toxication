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

type DshModelCapabilities = {
  input?: readonly ["text", "image"];
  reasoning?: DshReasoningProfile | false;
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
const imageInput = ["text", "image"] as const;

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
const subscriptionProfile: DshReasoningProfile = {
  efforts: [low, medium, high, xhigh],
  defaultEffort: "medium",
};
const subscription56LowProfile: DshReasoningProfile = {
  efforts: [low, medium, high, xhigh, max],
  defaultEffort: "low",
};
const subscription56MediumProfile: DshReasoningProfile = {
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

function withImage(reasoning: DshReasoningProfile | false): DshModelCapabilities {
  return { input: imageInput, reasoning };
}

function textOnly(reasoning: DshReasoningProfile | false): DshModelCapabilities {
  return { reasoning };
}

const gpt5Capabilities = withImage(gpt5Profile);
const gpt5ProCapabilities = withImage(gpt5ProProfile);
const gpt51Capabilities = withImage(gpt51Profile);
const gpt52Capabilities = withImage(gpt52Profile);
const gpt52ChatCapabilities = withImage(gpt52ChatProfile);
const openAiProCapabilities = withImage(openAiProProfile);
const subscriptionCapabilities = withImage(subscriptionProfile);
const subscription56LowCapabilities = withImage(subscription56LowProfile);
const subscription56MediumCapabilities = withImage(subscription56MediumProfile);
const realtimeCapabilities = withImage(realtimeProfile);
const oSeriesCapabilities = withImage(oSeriesProfile);

const modelCapabilities = new Map<string, DshModelCapabilities>([
  ...["gpt-5", "gpt-5-mini", "gpt-5-nano"].map((id) => [id, gpt5Capabilities] as const),
  ["gpt-5-pro", gpt5ProCapabilities],
  ["gpt-5.1", gpt51Capabilities],
  ["gpt-5.2", subscriptionCapabilities],
  ["gpt-5.2-chat-latest", gpt52ChatCapabilities],
  ["gpt-5.2-pro", openAiProCapabilities],
  ["gpt-5.3-codex", subscriptionCapabilities],
  ["gpt-5.3-codex-spark", textOnly(subscriptionProfile)],
  ...["gpt-5.4", "gpt-5.4-mini", "gpt-5.5", "codex-auto-review"].map(
    (id) => [id, subscriptionCapabilities] as const,
  ),
  ["gpt-5.4-nano", gpt52Capabilities],
  ...["gpt-5.4-pro", "gpt-5.5-pro"].map((id) => [id, openAiProCapabilities] as const),
  ["gpt-5.6-sol", subscription56LowCapabilities],
  ...["gpt-5.6-luna", "gpt-5.6-terra"].map((id) => [id, subscription56MediumCapabilities] as const),
  ["gpt-realtime-2.1", realtimeCapabilities],
  ...["o1", "o1-pro", "o3", "o3-pro", "o4-mini"].map((id) => [id, oSeriesCapabilities] as const),
  ["o3-mini", textOnly(oSeriesProfile)],
  ["gpt-4", textOnly(false)],
  ...["gpt-4-turbo", "gpt-4.1", "gpt-4.1-mini", "gpt-4.1-nano", "gpt-4o", "gpt-4o-mini"].map(
    (id) => [id, withImage(false)] as const,
  ),
  ...["gpt-5-chat-latest", "gpt-5.3-chat-latest"].map((id) => [id, withImage(false)] as const),
]);

function normalizedModelId(value: string) {
  const id = value.trim().toLowerCase().split("/").at(-1) ?? "";
  return id.replace(/-\d{4}-\d{2}-\d{2}$/, "");
}

/** Return the capabilities DSH may safely offer for one generated route. */
export function dshModelCapabilities(
  model: DshModelOption,
  protocol: DshProtocol,
): DshModelCapabilities | undefined {
  if (protocol === "chat" && model.family.trim().toLowerCase() === "deepseek") {
    return { reasoning: deepseekProfile };
  }
  if (protocol === "anthropic") {
    return undefined;
  }
  const capabilities = modelCapabilities.get(normalizedModelId(model.id));
  if (!capabilities) {
    return undefined;
  }
  if (protocol === "responses") {
    return capabilities;
  }
  return capabilities.input ? { input: capabilities.input } : undefined;
}
