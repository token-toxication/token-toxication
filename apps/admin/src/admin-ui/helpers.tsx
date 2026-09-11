import type { Dispatch, SetStateAction } from "react";
import { plural, t } from "@lingui/core/macro";
import { Trans } from "@lingui/react/macro";
import { toast } from "sonner";

import { Badge } from "@/components/ui/badge";

import type {
  CodexAccountQuotaResponse,
  GeminiAccountQuotaResponse,
  ModelCatalogEntry,
  ProviderAccount,
  ProviderModelRoute,
  ProviderPreset,
  RequestLog,
  RoutableModelCatalogEntry,
} from "../types";
import type {
  ClientModelOption,
  CreateAccountForm,
  DshModelOption,
  OpencodeModelOption,
} from "./types";

export function statusBadge(status: string, active: boolean) {
  if (!active) {
    return (
      <Badge variant="outline">
        <Trans>paused</Trans>
      </Badge>
    );
  }
  if (status === "healthy") {
    return (
      <Badge variant="secondary">
        <Trans>healthy</Trans>
      </Badge>
    );
  }
  if (status === "blocked") {
    return (
      <Badge variant="destructive">
        <Trans>blocked</Trans>
      </Badge>
    );
  }
  return <Badge variant="outline">{status}</Badge>;
}

export function routeRoleBadge(role: string) {
  if (role === "primary") {
    return (
      <Badge variant="secondary">
        <Trans>primary</Trans>
      </Badge>
    );
  }
  return <Badge variant="outline">{role}</Badge>;
}

export function routeCountForAccount(routes: ProviderModelRoute[], accountId: string) {
  return routes.filter((route) => route.providerAccountId === accountId).length;
}

export function routeCountForModel(routes: ProviderModelRoute[], modelId: string) {
  return routes.filter((route) => route.publicModelId === modelId).length;
}

export function accountName(accounts: readonly ProviderAccount[], accountId: string) {
  return accounts.find((account) => account.id === accountId)?.name ?? accountId;
}

export function uniqueSorted(values: string[]) {
  return Array.from(new Set(values)).sort((left, right) => left.localeCompare(right));
}

export function commaSeparatedValues(value: string) {
  return uniqueSorted(
    value
      .split(",")
      .map((item) => item.trim())
      .filter(Boolean),
  );
}

export function formatRoutePolicy(route: ProviderModelRoute) {
  if (route.stripParams.length === 0) {
    return t`No stripped params`;
  }
  const params = route.stripParams.join(", ");
  return t`strip ${params}`;
}

export function statusCodeBadge(status: number) {
  if (status >= 200 && status < 300) {
    return <Badge variant="secondary">{status}</Badge>;
  }
  if (status === 429 || status >= 500) {
    return <Badge variant="destructive">{status}</Badge>;
  }
  return <Badge variant="outline">{status}</Badge>;
}

export function formatLogModel(log: RequestLog) {
  const publicModel = log.model || t`unknown`;
  if (!log.upstreamModel || log.upstreamModel === publicModel) {
    return publicModel;
  }
  return `${publicModel} -> ${log.upstreamModel}`;
}

export function formatRequestSummary(log: RequestLog) {
  const summary = log.requestSummary;
  if (!summary) {
    return t`No request summary`;
  }
  const stream = summary.stream ? t`stream` : t`non-stream`;
  const keys = summary.topLevelKeys.length > 0 ? summary.topLevelKeys.join(", ") : t`no keys`;
  const stripped =
    summary.strippedParams.length > 0 ? t` · stripped ${summary.strippedParams.join(", ")}` : "";
  return t`${formatNumber(summary.bodyBytes)} bytes · ${stream} · keys ${keys}${stripped}`;
}

export function providerPresetForForm(form: CreateAccountForm, presets: ProviderPreset[]) {
  return presets.find(
    (preset) =>
      preset.provider === form.provider &&
      preset.baseUrl === form.baseUrl &&
      preset.authMode === form.authMode &&
      preset.wireApi === form.wireApi,
  );
}

export function accountPresetValue(form: CreateAccountForm, presets: ProviderPreset[]) {
  return providerPresetForForm(form, presets)?.id ?? "custom";
}

export function applyAccountPreset(
  presetId: string,
  presets: ProviderPreset[],
  setForm: Dispatch<SetStateAction<CreateAccountForm>>,
) {
  const preset = presets.find((item) => item.id === presetId);
  if (!preset) {
    return;
  }
  setForm((current) => ({
    ...current,
    name: current.name || preset.name,
    provider: preset.provider,
    baseUrl: preset.baseUrl,
    authMode: preset.authMode,
    wireApi: preset.wireApi,
  }));
}

export function routableModelIdsForWireApi(models: RoutableModelCatalogEntry[], wireApi: string) {
  return uniqueSorted(models.filter((model) => model.wireApi === wireApi).map((model) => model.id));
}

export function catalogModelIds(models: ModelCatalogEntry[]) {
  return uniqueSorted(models.filter((model) => model.enabled).map((model) => model.id));
}

export function enabledCatalogModelOptions(models: ModelCatalogEntry[]): ClientModelOption[] {
  return models
    .filter((model) => model.enabled && model.id)
    .map((model) => ({
      id: model.id,
      displayName: model.displayName || model.id,
    }))
    .sort((left, right) => left.id.localeCompare(right.id));
}

export function codexModelOptions(
  models: ModelCatalogEntry[],
  routableModels: RoutableModelCatalogEntry[],
): ClientModelOption[] {
  const responses = new Set(routableModelIdsForWireApi(routableModels, "openai-responses"));
  return enabledCatalogModelOptions(models).filter((model) => responses.has(model.id));
}

export function opencodeModelOptions(
  models: ModelCatalogEntry[],
  routableModels: RoutableModelCatalogEntry[],
): OpencodeModelOption[] {
  const chatModels = new Set(routableModelIdsForWireApi(routableModels, "openai-chat"));
  const responseModels = new Set(routableModelIdsForWireApi(routableModels, "openai-responses"));

  return enabledCatalogModelOptions(models)
    .filter((model) => chatModels.has(model.id) || responseModels.has(model.id))
    .map((model) => ({
      ...model,
      wireApi: responseModels.has(model.id) ? "openai-responses" : "openai-chat",
    }));
}

export function dshModelOptions(
  models: ModelCatalogEntry[],
  routableModels: RoutableModelCatalogEntry[],
): DshModelOption[] {
  const chat = new Set(routableModelIdsForWireApi(routableModels, "openai-chat"));
  const responses = new Set(routableModelIdsForWireApi(routableModels, "openai-responses"));
  const anthropic = new Set(routableModelIdsForWireApi(routableModels, "anthropic-messages"));

  return (
    models
      .filter((model) => model.enabled && model.id)
      // DSH is an agent harness; Astra Chat does not support tool calling.
      .filter((model) => model.id !== "gpt-6-astra" || responses.has(model.id))
      .filter((model) => chat.has(model.id) || responses.has(model.id) || anthropic.has(model.id))
      .map((model) => ({
        id: model.id,
        displayName: model.displayName || model.id,
        family: model.family,
        protocols: {
          chat: model.id !== "gpt-6-astra" && chat.has(model.id),
          responses: responses.has(model.id),
          anthropic: model.id !== "gpt-6-astra" && anthropic.has(model.id),
        },
      }))
      .sort((left, right) => left.id.localeCompare(right.id))
  );
}

export function preferredCatalogModel(current: string, catalogModels: string[], fallback: string) {
  if (catalogModels.length === 0) {
    return current || fallback;
  }
  return current && catalogModels.includes(current) ? current : catalogModels[0];
}

export function shellQuote(value: string) {
  return `'${value.replace(/'/g, "'\\''")}'`;
}

export function tomlString(value: string) {
  return JSON.stringify(value);
}

export function wireApiLabel(value: string) {
  switch (value) {
    case "openai-chat":
      return "OpenAI Chat";
    case "openai-responses":
      return "OpenAI Responses";
    case "anthropic-messages":
      return "Anthropic Messages";
    case "gemini-generate-content":
      return "Gemini GenerateContent";
    default:
      return value;
  }
}

export function upstreamPathForWireApi(value: string, authMode?: string) {
  if (isCodexSubscriptionAuth(authMode ?? "")) {
    return "/codex/responses";
  }
  if (isAntigravityAccountAuth(authMode ?? "")) {
    return "/v1internal:generateContent";
  }
  switch (value) {
    case "openai-chat":
      return "/chat/completions";
    case "openai-responses":
      return "/v1/responses";
    case "gemini-generate-content":
      return "/v1beta/models/{model}:generateContent";
    default:
      return "/v1/messages";
  }
}

export function isCodexSubscriptionAuth(value: string) {
  return value === "codex-oauth";
}

export function isAntigravityAccountAuth(value: string) {
  return value === "antigravity-oauth";
}

export function isGeminiAccount(account: ProviderAccount) {
  return account.provider === "gemini" && isAntigravityAccountAuth(account.authMode);
}

export function isCodexAccount(account: ProviderAccount) {
  return isCodexSubscriptionAuth(account.authMode);
}

export function numberFromInput(value: string) {
  const parsed = Number.parseInt(value, 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : 0;
}

export function formatNumber(value: number) {
  return new Intl.NumberFormat().format(value);
}

export function formatCacheHitRate(cachedInputTokens: number, inputTokens: number) {
  const rate = inputTokens > 0 ? cachedInputTokens / inputTokens : 0;
  return new Intl.NumberFormat(undefined, {
    style: "percent",
    maximumFractionDigits: 1,
  }).format(rate);
}

export function niceChartMaximum(value: number) {
  if (value <= 4) {
    return 4;
  }
  const magnitude = 10 ** Math.floor(Math.log10(value));
  const normalized = value / magnitude;
  const factor = normalized <= 2 ? 2 : normalized <= 5 ? 5 : 10;
  return factor * magnitude;
}

export function formatChartTime(value: string) {
  return new Intl.DateTimeFormat(undefined, {
    hour: "2-digit",
    minute: "2-digit",
  }).format(new Date(value));
}

export function formatChartInterval(startedAt: string, durationSeconds: number) {
  const endedAt = new Date(new Date(startedAt).getTime() + durationSeconds * 1_000);
  return `${formatChartTime(startedAt)}–${formatChartTime(endedAt.toISOString())}`;
}

export function formatRequestCount(value: number) {
  return plural(value, {
    one: "# request",
    other: "# requests",
  });
}

export function formatGeminiTier(tier: GeminiAccountQuotaResponse["currentTier"]) {
  if (!tier) {
    return t`unknown`;
  }
  return tier.name || tier.id || t`unknown`;
}

export function quotaPercent(value: number | null | undefined) {
  return value == null ? undefined : Math.max(0, Math.min(100, value * 100));
}

export function formatQuotaPercent(value: number) {
  if (value === 100) {
    return "100%";
  }
  return `${value.toFixed(value >= 99 ? 3 : 1)}%`;
}

export function codexUsedPercent(value: number | null | undefined) {
  return value == null || !Number.isFinite(value) ? undefined : Math.max(0, Math.min(100, value));
}

export function formatCodexWindow(seconds: number | null | undefined) {
  if (seconds == null || !Number.isFinite(seconds) || seconds <= 0) {
    return t`unknown`;
  }
  if (seconds >= 86_400) {
    const days = Math.round(seconds / 86_400);
    if (Math.abs(seconds - days * 86_400) <= 120) {
      return plural(days, { one: "# day", other: "# days" });
    }
    return t`${(seconds / 86_400).toFixed(1)} days`;
  }
  if (seconds >= 3_600) {
    const hours = Math.round(seconds / 3_600);
    if (Math.abs(seconds - hours * 3_600) <= 60) {
      return plural(hours, { one: "# hour", other: "# hours" });
    }
    return t`${(seconds / 3_600).toFixed(1)} hours`;
  }
  if (seconds >= 60) {
    const minutes = Math.round(seconds / 60);
    return plural(minutes, { one: "# minute", other: "# minutes" });
  }
  return plural(seconds, { one: "# second", other: "# seconds" });
}

export function formatQuotaReset(value: string | null | undefined) {
  return value ? formatDate(value) : "unknown";
}

export function codexQuotaStatus(
  allowed: boolean | null | undefined,
  limitReached: boolean | null | undefined,
) {
  if (limitReached === true || allowed === false) {
    return (
      <Badge variant="destructive">
        <Trans>limited</Trans>
      </Badge>
    );
  }
  if (allowed === true) {
    return (
      <Badge variant="secondary">
        <Trans>available</Trans>
      </Badge>
    );
  }
  return (
    <Badge variant="outline">
      <Trans>unknown</Trans>
    </Badge>
  );
}

export function formatCodexCredits(credits: NonNullable<CodexAccountQuotaResponse["credits"]>) {
  if (credits.unlimited) {
    return t`Unlimited`;
  }
  if (credits.balance) {
    return t`Balance ${credits.balance}`;
  }
  if (credits.hasCredits === true) {
    return t`Available`;
  }
  if (credits.hasCredits === false) {
    return t`None`;
  }
  return t`unknown`;
}

export function formatCodexSpendControl(
  reached: boolean | null | undefined,
  limit:
    | NonNullable<NonNullable<CodexAccountQuotaResponse["spendControl"]>["individualLimit"]>
    | null
    | undefined,
) {
  if (!limit) {
    return reached ? t`Reached` : t`unknown`;
  }
  const amount =
    limit.used && limit.limit
      ? `${limit.used} / ${limit.limit}`
      : limit.remaining
        ? t`${limit.remaining} remaining`
        : t`Configured`;
  return reached ? t`${amount}, reached` : amount;
}

export function formatOptionalNumber(value: number | null | undefined) {
  return value == null ? t`unknown` : formatNumber(value);
}

export function humanizeIdentifier(value: string) {
  const text = value.replaceAll("_", " ").trim();
  return text ? `${text.charAt(0).toUpperCase()}${text.slice(1)}` : t`Unknown`;
}

export function formatDate(value: string | null | undefined) {
  if (!value) {
    return t`never`;
  }
  return new Intl.DateTimeFormat(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  }).format(new Date(value));
}

export async function copyText(value: string) {
  await navigator.clipboard.writeText(value);
  toast.success(t`Copied`);
}
