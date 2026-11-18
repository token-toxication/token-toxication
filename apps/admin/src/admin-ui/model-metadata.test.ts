import { describe, expect, it } from "vite-plus/test";

import type { ModelCatalogEntry, ModelUpstreamObservation } from "../types";
import {
  capabilitiesFromForm,
  contextDisplay,
  emptyCapabilitiesForm,
  formatCapacityRange,
  formatTokenCapacity,
  formFromCapabilities,
  modelFormFromEntry,
} from "./model-metadata";

function observation(values: Partial<ModelUpstreamObservation> = {}): ModelUpstreamObservation {
  return {
    capabilities: {},
    minimalClientVersion: null,
    observedAt: "2026-11-18T00:00:00Z",
    providerAccountId: "account",
    providerAccountName: "Account",
    supportedInApi: true,
    upstreamModelId: "model",
    visibility: "list",
    ...values,
  };
}

function entry(values: Partial<ModelCatalogEntry> = {}): ModelCatalogEntry {
  return {
    createdAt: "2026-11-18T00:00:00Z",
    declared: {},
    displayName: "Model",
    enabled: true,
    family: "openai",
    id: "model",
    managed: false,
    upstream: [],
    ...values,
  };
}

describe("declared model capabilities", () => {
  it("parses optional numbers and comma-separated lists", () => {
    expect(
      capabilitiesFromForm({
        ...emptyCapabilitiesForm(),
        contextWindow: " 500000 ",
        maxContextWindow: "872000",
        reasoningLevels: "low, medium, low, ",
        defaultReasoningLevel: " low ",
        inputModalities: "text,image",
        serviceTiers: "priority, ultrafast",
      }),
    ).toEqual({
      contextWindow: 500_000,
      maxContextWindow: 872_000,
      reasoningLevels: ["low", "medium"],
      defaultReasoningLevel: "low",
      inputModalities: ["text", "image"],
      serviceTiers: ["priority", "ultrafast"],
    });
  });

  it("leaves empty fields unknown", () => {
    expect(capabilitiesFromForm(emptyCapabilitiesForm())).toEqual({
      contextWindow: null,
      maxContextWindow: null,
      reasoningLevels: [],
      defaultReasoningLevel: null,
      inputModalities: [],
      serviceTiers: [],
    });
  });

  it("hydrates a form and catalog entry from declared metadata", () => {
    const model = entry({
      declared: {
        contextWindow: 128_000,
        maxContextWindow: 256_000,
        reasoningLevels: ["low", "high"],
        defaultReasoningLevel: "high",
        inputModalities: ["text"],
        serviceTiers: ["priority"],
      },
    });
    expect(formFromCapabilities(model.declared)).toEqual({
      contextWindow: "128000",
      maxContextWindow: "256000",
      reasoningLevels: "low, high",
      defaultReasoningLevel: "high",
      inputModalities: "text",
      serviceTiers: "priority",
    });
    expect(modelFormFromEntry(model)).toMatchObject({
      id: "model",
      displayName: "Model",
      contextWindow: "128000",
      maxContextWindow: "256000",
    });
  });
});

describe("catalog capacity display", () => {
  it("formats capacities compactly", () => {
    expect(formatTokenCapacity(500_000)).toBe("500K");
    expect(formatTokenCapacity(1_000_000)).toBe("1M");
    expect(formatTokenCapacity(1_050_000)).toBe("1.05M");
    expect(formatTokenCapacity(null)).toBe("—");
    expect(formatCapacityRange([500_000, 272_000, 500_000])).toBe("272K–500K");
    expect(formatCapacityRange([])).toBe("—");
  });

  it("prefers upstream observations and falls back to declared values", () => {
    expect(
      contextDisplay(
        entry({
          declared: { contextWindow: 128_000 },
          upstream: [
            observation({
              capabilities: { contextWindow: 500_000, maxContextWindow: 872_000 },
            }),
            observation({
              capabilities: { contextWindow: 272_000, maxContextWindow: 872_000 },
            }),
          ],
        }),
      ),
    ).toEqual({ context: "272K–500K", maxContext: "872K", source: "upstream" });
    expect(contextDisplay(entry({ declared: { contextWindow: 128_000 } }))).toEqual({
      context: "128K",
      maxContext: "—",
      source: "declared",
    });
    expect(contextDisplay(entry())).toEqual({
      context: "—",
      maxContext: "—",
      source: "unknown",
    });
  });
});
