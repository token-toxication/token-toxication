import type { ModelCapabilities, ModelCatalogEntry } from "../types";
import type { ModelCatalogForm } from "./types";

export type ModelCapabilitiesForm = {
  contextWindow: string;
  maxContextWindow: string;
  reasoningLevels: string;
  defaultReasoningLevel: string;
  inputModalities: string;
  serviceTiers: string;
};

export function emptyCapabilitiesForm(): ModelCapabilitiesForm {
  return {
    contextWindow: "",
    maxContextWindow: "",
    reasoningLevels: "",
    defaultReasoningLevel: "",
    inputModalities: "",
    serviceTiers: "",
  };
}

export function capabilitiesFromForm(form: ModelCapabilitiesForm): ModelCapabilities {
  return {
    contextWindow: positiveInteger(form.contextWindow),
    maxContextWindow: positiveInteger(form.maxContextWindow),
    reasoningLevels: commaList(form.reasoningLevels),
    defaultReasoningLevel: form.defaultReasoningLevel.trim() || null,
    inputModalities: commaList(form.inputModalities),
    serviceTiers: commaList(form.serviceTiers),
  };
}

export function modelFormFromEntry(entry: ModelCatalogEntry): ModelCatalogForm {
  return {
    id: entry.id,
    displayName: entry.displayName,
    family: entry.family,
    enabled: entry.enabled,
    ...formFromCapabilities(entry.declared),
  };
}

export function formFromCapabilities(
  capabilities: ModelCapabilities | undefined,
): ModelCapabilitiesForm {
  return {
    contextWindow: capabilities?.contextWindow ? String(capabilities.contextWindow) : "",
    maxContextWindow: capabilities?.maxContextWindow ? String(capabilities.maxContextWindow) : "",
    reasoningLevels: (capabilities?.reasoningLevels ?? []).join(", "),
    defaultReasoningLevel: capabilities?.defaultReasoningLevel ?? "",
    inputModalities: (capabilities?.inputModalities ?? []).join(", "),
    serviceTiers: (capabilities?.serviceTiers ?? []).join(", "),
  };
}

export function formatTokenCapacity(value: number | null | undefined): string {
  if (value === null || value === undefined || !Number.isFinite(value) || value <= 0) {
    return "—";
  }
  if (value >= 1_000_000) {
    const millions = (value / 1_000_000).toFixed(2).replace(/\.?0+$/, "");
    return `${millions}M`;
  }
  if (value >= 1_000) {
    return `${Math.round(value / 1_000)}K`;
  }
  return String(value);
}

export function formatCapacityRange(values: readonly (number | null | undefined)[]): string {
  const present = Array.from(
    new Set(
      values.filter(
        (value): value is number =>
          typeof value === "number" && Number.isFinite(value) && value > 0,
      ),
    ),
  ).sort((left, right) => left - right);
  if (present.length === 0) return "—";
  const first = present[0];
  const last = present[present.length - 1];
  return first === last
    ? formatTokenCapacity(first)
    : `${formatTokenCapacity(first)}–${formatTokenCapacity(last)}`;
}

export function formatCapabilityList(values: readonly string[] | null | undefined): string {
  return values && values.length > 0 ? values.join(", ") : "—";
}

export type ContextDisplay = {
  context: string;
  maxContext: string;
  source: "upstream" | "declared" | "unknown";
};

/** Prefer upstream observations; fall back to operator-declared metadata. */
export function contextDisplay(model: ModelCatalogEntry): ContextDisplay {
  const upstream = model.upstream ?? [];
  if (upstream.length > 0) {
    return {
      context: formatCapacityRange(upstream.map((item) => item.capabilities.contextWindow)),
      maxContext: formatCapacityRange(upstream.map((item) => item.capabilities.maxContextWindow)),
      source: "upstream",
    };
  }
  if (model.declared.contextWindow || model.declared.maxContextWindow) {
    return {
      context: formatTokenCapacity(model.declared.contextWindow),
      maxContext: formatTokenCapacity(model.declared.maxContextWindow),
      source: "declared",
    };
  }
  return { context: "—", maxContext: "—", source: "unknown" };
}

function positiveInteger(value: string): number | null {
  const trimmed = value.trim();
  if (trimmed === "") return null;
  const parsed = Number(trimmed);
  return Number.isInteger(parsed) && parsed > 0 ? parsed : null;
}

function commaList(value: string): string[] {
  const seen = new Set<string>();
  for (const item of value.split(",")) {
    const trimmed = item.trim();
    if (trimmed.length > 0) seen.add(trimmed);
  }
  return Array.from(seen);
}
