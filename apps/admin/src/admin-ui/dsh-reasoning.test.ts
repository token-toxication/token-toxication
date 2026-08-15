import { describe, expect, it } from "vite-plus/test";

import { dshReasoningProfile } from "./dsh-reasoning";
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
  return dshReasoningProfile(model(id), protocol);
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

const codexStandardEfforts = ["low:low", "medium:medium", "high:high", "xhigh:xhigh"];
const codex56Efforts = [...codexStandardEfforts, "max:max"];

describe("dshReasoningProfile", () => {
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
    ["gpt-5.6-sol", codex56Efforts, "low"],
    ["gpt-5.6-terra", codex56Efforts, "medium"],
    ["gpt-5.6-luna", codex56Efforts, "medium"],
    ["gpt-5.5", codexStandardEfforts, "medium"],
    ["gpt-5.4", codexStandardEfforts, "medium"],
    ["gpt-5.4-mini", codexStandardEfforts, "medium"],
    ["gpt-5.2", codexStandardEfforts, "medium"],
    ["codex-auto-review", codexStandardEfforts, "medium"],
  ])("covers current Codex model %s", (id, efforts, defaultEffort) => {
    expect(effortsFor(id as string)).toEqual(efforts);
    expect(reasoningProfileFor(id as string)?.defaultEffort).toBe(defaultEffort);
  });

  it.each(["gpt-5.3-codex", "gpt-5.3-codex-spark"])("keeps Codex 5.3 model %s supported", (id) => {
    expect(effortsFor(id)).toEqual(codexStandardEfforts);
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

  it("only applies OpenAI profiles to Responses routes in the OpenAI family", () => {
    expect(profileFor("gpt-5.5", "chat")).toBeUndefined();
    expect(dshReasoningProfile(model("gpt-5.5", "custom"), "responses")).toBeUndefined();
  });

  it("preserves DeepSeek Chat reasoning and wire compatibility", () => {
    const profile = dshReasoningProfile(model("deepseek-v4-pro", "deepseek"), "chat");
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
