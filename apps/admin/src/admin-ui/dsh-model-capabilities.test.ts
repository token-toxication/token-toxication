import { describe, expect, it } from "vite-plus/test";

import { dshModelCapabilities } from "./dsh-model-capabilities";
import type { DshModelOption, DshProtocol } from "./types";

function model(id: string, family = "openai"): DshModelOption {
  return {
    id,
    displayName: id,
    family,
    protocols: { chat: false, responses: true, anthropic: false },
  };
}

function profileFor(id: string, protocol: DshProtocol = "responses") {
  return dshModelCapabilities(model(id), protocol)?.reasoning;
}

function reasoningProfileFor(id: string) {
  const profile = profileFor(id);
  return profile === false ? undefined : profile;
}

function effortsFor(id: string) {
  return reasoningProfileFor(id)?.efforts.map(
    (effort) => effort.id + ":" + (effort.wireValue ?? ""),
  );
}

const subscriptionStandardEfforts = ["low:low", "medium:medium", "high:high", "xhigh:xhigh"];
const subscription56Efforts = [...subscriptionStandardEfforts, "max:max"];

describe("dshModelCapabilities", () => {
  it.each([
    ["gpt-5", ["minimal:minimal", "low:low", "medium:medium", "high:high"], "high"],
    ["gpt-5-pro", ["high:high"], "high"],
    ["gpt-5.1", ["off:none", "low:low", "medium:medium", "high:high"], "high"],
    ["gpt-5.5-pro", ["medium:medium", "high:high", "xhigh:xhigh"], "xhigh"],
    ["o3", ["low:low", "medium:medium", "high:high"], "high"],
  ])("describes the supported wire values for %s", (id, efforts, defaultEffort) => {
    expect(effortsFor(id as string)).toEqual(efforts);
    expect(reasoningProfileFor(id as string)?.defaultEffort).toBe(defaultEffort);
  });

  it.each([
    ["gpt-5.6-sol", subscription56Efforts, "low"],
    ["gpt-5.6-terra", subscription56Efforts, "medium"],
    ["gpt-5.6-luna", subscription56Efforts, "medium"],
    ["gpt-5.5", subscriptionStandardEfforts, "medium"],
    ["gpt-5.4", subscriptionStandardEfforts, "medium"],
    ["gpt-5.4-mini", subscriptionStandardEfforts, "medium"],
    ["gpt-5.2", subscriptionStandardEfforts, "medium"],
    ["codex-auto-review", subscriptionStandardEfforts, "medium"],
  ])("covers current bundled model %s", (id, efforts, defaultEffort) => {
    expect(effortsFor(id as string)).toEqual(efforts);
    expect(reasoningProfileFor(id as string)?.defaultEffort).toBe(defaultEffort);
  });

  it.each(["gpt-5.3-codex", "gpt-5.3-codex-spark"])("keeps 5.3 model %s supported", (id) => {
    expect(effortsFor(id)).toEqual(subscriptionStandardEfforts);
    expect(reasoningProfileFor(id)?.defaultEffort).toBe("medium");
  });

  it("normalizes provider prefixes, casing, and dated model aliases", () => {
    expect(effortsFor("OpenAI/GPT-5.5-2026-03-17")).toEqual(effortsFor("gpt-5.5"));
  });

  it.each(["gpt-4.1", "gpt-4o", "gpt-5-chat-latest", "gpt-5.3-chat-latest"])(
    "marks known non-reasoning model %s explicitly",
    (id) => {
      expect(profileFor(id)).toBe(false);
    },
  );

  it("keeps an unknown OpenAI model undeclared", () => {
    expect(profileFor("gpt-5.7")).toBeUndefined();
  });

  it("uses exact model IDs on Responses routes regardless of the catalog family", () => {
    expect(profileFor("gpt-5.5", "chat")).toBeUndefined();
    expect(dshModelCapabilities(model("gpt-5.5", "other"), "responses")?.reasoning).toEqual(
      reasoningProfileFor("gpt-5.5"),
    );
  });

  it("declares image input for exact multimodal model IDs", () => {
    expect(dshModelCapabilities(model("gpt-5.6-luna", "other"), "responses")?.input).toEqual([
      "text",
      "image",
    ]);
    expect(dshModelCapabilities(model("gpt-5.6-luna", "other"), "chat")?.input).toEqual([
      "text",
      "image",
    ]);
  });

  it.each(["gpt-4", "o3-mini", "gpt-5.3-codex-spark", "gpt-5.7"])(
    "keeps text-only or unknown model %s on the default input",
    (id) => {
      expect(dshModelCapabilities(model(id), "responses")?.input).toBeUndefined();
    },
  );

  it("does not apply OpenAI input metadata to Anthropic routes", () => {
    expect(dshModelCapabilities(model("gpt-5.6-luna"), "anthropic")).toBeUndefined();
  });

  it("preserves DeepSeek Chat reasoning and wire compatibility", () => {
    const profile = dshModelCapabilities(model("deepseek-v4-pro", "deepseek"), "chat")?.reasoning;
    if (!profile) {
      throw new Error("expected DeepSeek reasoning profile");
    }

    expect(profile.efforts).toEqual([
      { id: "off" },
      { id: "high", wireValue: "high" },
      { id: "max", wireValue: "max" },
    ]);
    expect(profile.defaultEffort).toBe("max");
    expect(profile.thinkingFormat).toBe("deepseek");
  });
});
