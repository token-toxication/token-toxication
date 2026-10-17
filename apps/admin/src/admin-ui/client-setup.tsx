import { useMemo } from "react";
import { ClipboardCopyIcon, DatabaseIcon, KeyRoundIcon } from "lucide-react";
import { plural, t } from "@lingui/core/macro";
import { Trans } from "@lingui/react/macro";

import { dshModelCapabilities } from "./dsh-model-capabilities";
import {
  catalogModelIds,
  codexModelOptions,
  copyText,
  dshModelOptions,
  enabledCatalogModelOptions,
  opencodeModelOptions,
  routableModelIdsForWireApi,
  shellQuote,
  tomlString,
} from "./helpers";
import { EmptyNotice, Field, SettingRow } from "./shared";
import type { ClientModelOption, DshModelOption, DshProtocol, OpencodeModelOption } from "./types";
import type { ModelCatalogEntry, RoutableModelCatalogEntry } from "../types";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";

const dshProviderIds: Record<DshProtocol, string> = {
  chat: "token-toxication-chat",
  responses: "token-toxication-responses",
  anthropic: "token-toxication-anthropic",
};

const dshWireApis: Record<DshProtocol, string> = {
  chat: "openai-completions",
  responses: "openai-responses",
  anthropic: "anthropic-messages",
};

function withSpeedTierAliases<T extends ClientModelOption>(models: T[]): T[] {
  return models.flatMap((model) => {
    if (
      !model.id.startsWith("gpt-") ||
      model.id.endsWith("-fast") ||
      model.id.endsWith("-ultrafast")
    ) {
      return [model];
    }
    return [
      model,
      { ...model, id: `${model.id}-fast`, displayName: `${model.displayName || model.id} (Fast)` },
      {
        ...model,
        id: `${model.id}-ultrafast`,
        displayName: `${model.displayName || model.id} (Ultrafast)`,
      },
    ];
  });
}

function opencodeCatalogPluginSource(openaiBaseUrl: string, fallbackModels: OpencodeModelOption[]) {
  const chatUrl = `${openaiBaseUrl}/models?wire_api=openai-chat`;
  const responsesUrl = `${openaiBaseUrl}/models?wire_api=openai-responses`;
  const fallbackEntries = JSON.stringify(
    withSpeedTierAliases(fallbackModels).map((model) => ({
      id: model.id,
      name: model.displayName || model.id,
      wireApi: model.wireApi,
    })),
  );
  return `import { Plugin } from "@opencode/plugin";

const PROVIDER_ID = "token-toxication";
const OPENAI_BASE_URL = ${JSON.stringify(openaiBaseUrl)};
const REFRESH_INTERVAL_MS = 5 * 60 * 1000;
const FALLBACK_ENTRIES = ${fallbackEntries};
const endpoints = [
  { wireApi: "openai-chat", url: ${JSON.stringify(chatUrl)} },
  { wireApi: "openai-responses", url: ${JSON.stringify(responsesUrl)} },
];

function withSpeedTierAliases(entries) {
  return entries.flatMap((entry) => {
    if (!entry.id.startsWith("gpt-") || entry.id.endsWith("-fast") || entry.id.endsWith("-ultrafast")) {
      return [entry];
    }
    return [
      entry,
      { ...entry, id: entry.id + "-fast", name: entry.name + " (Fast)" },
      { ...entry, id: entry.id + "-ultrafast", name: entry.name + " (Ultrafast)" },
    ];
  });
}

async function fetchCatalog() {
  const key = process.env.TOKEN_TOXICATION_API_KEY;
  if (!key) return undefined;
  const responses = await Promise.all(
    endpoints.map(async ({ wireApi, url }) => {
      const response = await fetch(url, {
        signal: AbortSignal.timeout(4_000),
        headers: { accept: "application/json", authorization: "Bearer " + key },
      });
      if (!response.ok) throw new Error("Token Toxication model catalog failed: " + response.status);
      const body = await response.json();
      return (Array.isArray(body.data) ? body.data : [])
        .filter((entry) => typeof entry?.id === "string" && entry.id.length > 0)
        .map((entry) => ({ id: entry.id, name: entry.id, wireApi }));
    }),
  );
  const byId = new Map();
  for (const entry of responses.flat()) {
    const previous = byId.get(entry.id);
    if (!previous || entry.wireApi === "openai-responses") byId.set(entry.id, entry);
  }
  return withSpeedTierAliases([...byId.values()]);
}

function modelDefinition(entry, template) {
  const packageName =
    entry.wireApi === "openai-responses"
      ? "aisdk:@ai-sdk/openai"
      : "aisdk:@ai-sdk/openai-compatible";
  const base =
    template ||
    {
      id: entry.id,
      modelID: entry.id,
      providerID: PROVIDER_ID,
      name: entry.name,
      package: packageName,
      capabilities: { tools: true, input: ["text", "image"], output: ["text"] },
      variants: [],
      time: { released: 0 },
      cost: [],
      status: "active",
      enabled: true,
      limit: { context: 200000, output: 32000 },
    };
  return {
    ...structuredClone(base),
    id: entry.id,
    modelID: entry.id,
    providerID: PROVIDER_ID,
    name: entry.name,
    package: packageName,
    settings: { ...(base.settings || {}), baseURL: OPENAI_BASE_URL },
  };
}

function modelDefinitions(entries, provider) {
  const templates = provider ? [...provider.models.values()] : [];
  return entries.map((entry) => {
    const template = templates.find((model) => {
      const packageName = String(model.package || "");
      const isCompatible = packageName.includes("openai-compatible");
      return entry.wireApi === "openai-chat" ? isCompatible : !isCompatible && packageName.includes("openai");
    });
    return modelDefinition(entry, template);
  });
}

export default Plugin.define({
  id: "token-toxication-model-catalog",
  setup: async (ctx) => {
    let discovered = FALLBACK_ENTRIES;
    try {
      const next = await fetchCatalog();
      if (next !== undefined) discovered = next;
    } catch {
      // The generated fallback entries remain available.
    }

    await ctx.provider.transform((editor) => {
      const provider = editor.get(PROVIDER_ID);
      if (provider) {
        editor.models.set(PROVIDER_ID, modelDefinitions(discovered, provider));
      } else {
        editor.add({
          info: {
            id: PROVIDER_ID,
            name: "Token Toxication",
            activation: "enabled",
            package: "aisdk:@ai-sdk/openai-compatible",
            settings: { baseURL: OPENAI_BASE_URL },
          },
          models: modelDefinitions(discovered, undefined),
        });
      }
    });

    const refresh = async () => {
      try {
        const next = await fetchCatalog();
        if (next === undefined) return;
        discovered = next;
        await ctx.provider.reload();
      } catch {
        // Keep the last successful catalog or generated fallback.
      }
    };
    if (discovered.length === 0) await refresh();
    const timer = setInterval(() => void refresh(), REFRESH_INTERVAL_MS);
    return () => clearInterval(timer);
  },
});
`;
}

function piCatalogExtensionSource(openaiBaseUrl: string) {
  const endpoint = `${openaiBaseUrl}/models?wire_api=openai-responses`;
  return `const PROVIDER_ID = "token-toxication";
const REFRESH_INTERVAL_MS = 5 * 60 * 1000;
const CATALOG_URL = ${JSON.stringify(endpoint)};
const API_KEY = "$TOKEN_TOXICATION_API_KEY";

function withSpeedTierAliases(entries) {
  return entries.flatMap((entry) => {
    if (!entry.id.startsWith("gpt-") || entry.id.endsWith("-fast") || entry.id.endsWith("-ultrafast")) {
      return [entry];
    }
    return [
      entry,
      { ...entry, id: entry.id + "-fast", name: entry.name + " (Fast)" },
      { ...entry, id: entry.id + "-ultrafast", name: entry.name + " (Ultrafast)" },
    ];
  });
}

async function fetchCatalog(signal) {
  const key = process.env.TOKEN_TOXICATION_API_KEY;
  if (!key) return undefined;
  const timeout = AbortSignal.timeout(4_000);
  const requestSignal = signal ? AbortSignal.any([signal, timeout]) : timeout;
  const response = await fetch(CATALOG_URL, {
    signal: requestSignal,
    headers: { accept: "application/json", authorization: "Bearer " + key },
  });
  if (!response.ok) throw new Error("Token Toxication model catalog failed: " + response.status);
  const body = await response.json();
  const entries = (Array.isArray(body.data) ? body.data : [])
    .filter((entry) => typeof entry?.id === "string" && entry.id.length > 0)
    .map((entry) => ({
      id: entry.id,
      name: entry.id,
      reasoning: true,
      input: ["text"],
      contextWindow: 200000,
      maxTokens: 32768,
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
    }));
  return withSpeedTierAliases(entries);
}

function providerConfig(models) {
  return {
    name: "Token Toxication",
    baseUrl: ${JSON.stringify(openaiBaseUrl)},
    api: "openai-responses",
    apiKey: API_KEY,
    ...(models !== undefined ? { models } : {}),
    refreshModels: async ({ signal }) => (await fetchCatalog(signal)) ?? [],
  };
}

export default async function tokenToxicationModels(pi) {
  let models;
  try {
    models = await fetchCatalog();
  } catch {
    // models.json remains the offline fallback until the next refresh.
  }
  pi.registerProvider(PROVIDER_ID, providerConfig(models));
  const timer = setInterval(async () => {
    try {
      const next = await fetchCatalog();
      if (next !== undefined) pi.registerProvider(PROVIDER_ID, providerConfig(next));
    } catch {
      // Keep the last successful catalog.
    }
  }, REFRESH_INTERVAL_MS);
  timer.unref?.();
  pi.on("session_shutdown", () => clearInterval(timer));
}
`;
}

export function dshModelEntryYaml(model: DshModelOption, protocol: DshProtocol) {
  const entryIndent = "          ";
  const lines = [`- id: ${JSON.stringify(model.id)}`];
  const compat: string[] = [];
  if (model.displayName && model.displayName !== model.id) {
    lines.push(`${entryIndent}name: ${JSON.stringify(model.displayName)}`);
  }
  const capabilities = dshModelCapabilities(model, protocol);
  if (capabilities?.input) {
    lines.push(`${entryIndent}input: [${capabilities.input.join(", ")}]`);
  }
  const reasoning = capabilities?.reasoning;
  if (reasoning === false) {
    lines.push(`${entryIndent}reasoningEfforts: false`);
  } else if (reasoning) {
    lines.push(`${entryIndent}reasoningEfforts:`);
    for (const effort of reasoning.efforts) {
      const wireValue = effort.wireValue === undefined ? "" : ` ${effort.wireValue}`;
      lines.push(`${entryIndent}  ${effort.id}:${wireValue}`);
    }
    if (reasoning.thinkingFormat) {
      compat.push(`thinkingFormat: ${reasoning.thinkingFormat}`);
    }
  }
  if (protocol === "chat") {
    // DSH passes its session ID to pi-ai, but custom OpenAI-compatible providers
    // do not receive pi-ai's OpenAI-only session headers. Emitting the standard
    // prompt cache key gives the relay a stable native-body affinity source.
    compat.push("supportsPromptCacheKey: true");
  }
  if (compat.length > 0) {
    lines.push(`${entryIndent}compat:`);
    lines.push(...compat.map((value) => `${entryIndent}  ${value}`));
  }
  return lines.join("\n");
}

export function dshProviderYaml(
  protocol: DshProtocol,
  models: DshModelOption[],
  openaiBaseUrl: string,
  anthropicBaseUrl: string,
) {
  if (models.length === 0) {
    return "";
  }
  const baseUrl = protocol === "anthropic" ? anthropicBaseUrl : openaiBaseUrl;
  return [
    `    ${dshProviderIds[protocol]}:`,
    '      displayName: "Token Toxication"',
    "      apiKeyEnv: TOKEN_TOXICATION_API_KEY",
    `      api: ${dshWireApis[protocol]}`,
    `      baseURL: ${baseUrl}`,
    "      models:",
    ...models.map((model) => `        ${dshModelEntryYaml(model, protocol)}`),
  ].join("\n");
}

export function ClientSetupView({
  models,
  routableModels,
  apiKey,
  setApiKey,
}: {
  models: ModelCatalogEntry[];
  routableModels: RoutableModelCatalogEntry[];
  apiKey: string;
  setApiKey: React.Dispatch<React.SetStateAction<string>>;
}) {
  const serviceOrigin = useMemo(
    () => (typeof window === "undefined" ? "http://127.0.0.1:3000" : window.location.origin),
    [],
  );
  const catalogModels = useMemo(() => catalogModelIds(models), [models]);
  const codexModels = useMemo(
    () => codexModelOptions(models, routableModels),
    [models, routableModels],
  );
  const chatModels = useMemo(
    () => routableModelIdsForWireApi(routableModels, "openai-chat"),
    [routableModels],
  );
  const claudeModels = useMemo(
    () => routableModelIdsForWireApi(routableModels, "anthropic-messages"),
    [routableModels],
  );
  const opencodeModels = useMemo(
    () => opencodeModelOptions(models, routableModels),
    [models, routableModels],
  );
  const piModels = useMemo(() => {
    const responseModels = new Set(codexModels.map((model) => model.id));
    return enabledCatalogModelOptions(models).filter((model) => responseModels.has(model.id));
  }, [codexModels, models]);
  const dshModels = useMemo(
    () => dshModelOptions(models, routableModels),
    [models, routableModels],
  );

  const snippets = useMemo(
    () =>
      buildClientSetupSnippets({
        apiKey,
        serviceOrigin,
        opencodeModels,
        piModels,
        dshModels,
      }),
    [apiKey, serviceOrigin, opencodeModels, piModels, dshModels],
  );
  const keyLooksValid = apiKey.trim() === "" || apiKey.trim().startsWith("tokentoxication-");

  return (
    <div className="flex flex-col gap-5">
      <Card>
        <CardHeader>
          <CardTitle>
            <Trans>Client Setup</Trans>
          </CardTitle>
          <CardDescription>
            <Trans>Generate copy-paste configuration for local AI coding clients.</Trans>
          </CardDescription>
        </CardHeader>
        <CardContent className="grid gap-4 lg:grid-cols-[1fr_1fr]">
          <Alert className="lg:col-span-2">
            <KeyRoundIcon className="size-4" />
            <AlertTitle>
              <Trans>Use a relay API key secret</Trans>
            </AlertTitle>
            <AlertDescription>
              <Trans>
                Newly created keys are prefilled here once. Existing rows only show previews, so
                paste the original tokentoxication-* value before copying a setup block.
              </Trans>
            </AlertDescription>
          </Alert>
          {!keyLooksValid ? (
            <Alert variant="destructive" className="lg:col-span-2">
              <KeyRoundIcon className="size-4" />
              <AlertTitle>
                <Trans>Unexpected key prefix</Trans>
              </AlertTitle>
              <AlertDescription>
                <Trans>Client keys should start with tokentoxication-.</Trans>
              </AlertDescription>
            </Alert>
          ) : null}
          <Field label={t`Relay API key`} htmlFor="setup-api-key">
            <Input
              id="setup-api-key"
              type="password"
              value={apiKey}
              onChange={(event) => setApiKey(event.target.value)}
              placeholder="tokentoxication-..."
              autoComplete="off"
            />
          </Field>
          <div className="flex flex-col gap-3">
            <SettingRow label={t`OpenAI base`} value={snippets.openaiBaseUrl} />
            <SettingRow label={t`Anthropic base`} value={snippets.anthropicBaseUrl} />
            <div className="grid gap-3 sm:grid-cols-2">
              <SettingRow label={t`Catalog models`} value={String(catalogModels.length)} />
              <SettingRow label={t`Chat routed`} value={String(chatModels.length)} />
              <SettingRow label={t`Responses routed`} value={String(codexModels.length)} />
              <SettingRow label={t`Messages routed`} value={String(claudeModels.length)} />
            </div>
          </div>
          {catalogModels.length === 0 ? (
            <Alert className="lg:col-span-2">
              <DatabaseIcon className="size-4" />
              <AlertTitle>
                <Trans>No catalog models yet</Trans>
              </AlertTitle>
              <AlertDescription>
                <Trans>
                  Add exact model names in Model Catalog, then bind them to provider routes. Client
                  setup will populate from that catalog.
                </Trans>
              </AlertDescription>
            </Alert>
          ) : null}
        </CardContent>
      </Card>

      {catalogModels.length > 0 ? (
        <Tabs defaultValue="codex">
          <TabsList>
            <TabsTrigger value="codex">Codex</TabsTrigger>
            <TabsTrigger value="claude">Claude Code</TabsTrigger>
            <TabsTrigger value="opencode">opencode</TabsTrigger>
            <TabsTrigger value="pi">Pi</TabsTrigger>
            <TabsTrigger value="dsh">DeepSeek Harness</TabsTrigger>
          </TabsList>
          <TabsContent value="codex">
            {codexModels.length > 0 ? (
              <div className="grid gap-5">
                <Card>
                  <CardHeader>
                    <CardTitle>
                      <Trans>Routable Responses models</Trans>
                    </CardTitle>
                    <CardDescription>
                      <Trans>
                        Codex uses its own model catalog and chooses the model. Check that the
                        selected model has a compatible relay route; every fallback route must
                        support the protocol Codex sends.
                      </Trans>
                    </CardDescription>
                  </CardHeader>
                  <CardContent className="flex flex-wrap gap-2">
                    {codexModels.map((model) => (
                      <Badge key={model.id} variant="secondary">
                        {model.id}
                      </Badge>
                    ))}
                  </CardContent>
                </Card>
                <ClientSnippetCard
                  title={t`Codex environment`}
                  description={t`Set the relay key and download the Desktop compatibility catalog before starting Codex.`}
                  endpoint="TOKEN_TOXICATION_API_KEY"
                  model={t`Chosen in Codex`}
                  snippet={snippets.codexEnvironment}
                />
                <ClientSnippetCard
                  title={t`Codex provider configuration`}
                  description={t`Merge this TOML into ~/.codex/config.toml. It uses TT's dynamic catalog while the downloaded file keeps the Desktop picker populated.`}
                  endpoint="/openai/v1/responses"
                  model={t`Chosen in Codex`}
                  snippet={snippets.codexConfig}
                />
              </div>
            ) : (
              <EmptyNotice
                title={t`No Codex routes`}
                body={t`Add an enabled OpenAI Responses route to configure Codex.`}
              />
            )}
          </TabsContent>
          <TabsContent value="claude">
            {claudeModels.length > 0 ? (
              <ClientSnippetCard
                title={t`Claude Code environment`}
                description={t`Points Claude Code at the Anthropic Messages namespace.`}
                endpoint="/anthropic/v1/messages"
                model={t`Chosen in Claude Code`}
                snippet={snippets.claudeCode}
              />
            ) : (
              <EmptyNotice
                title={t`No Claude Code routes`}
                body={t`Add an enabled Anthropic Messages route to configure Claude Code.`}
              />
            )}
          </TabsContent>
          <TabsContent value="opencode">
            {opencodeModels.length > 0 ? (
              <ClientSnippetCard
                title={t`opencode project config`}
                description={t`Installs a project plugin that refreshes the relay model catalog, with the generated list as a fallback.`}
                endpoint="/openai/v1"
                model={t`Chosen in opencode`}
                snippet={snippets.opencode}
              />
            ) : (
              <EmptyNotice
                title={t`No opencode routes`}
                body={t`Add an eligible OpenAI Chat or Responses route to generate an opencode config.`}
              />
            )}
          </TabsContent>
          <TabsContent value="pi">
            {piModels.length > 0 ? (
              <ClientSnippetCard
                title={t`Pi custom provider`}
                description={t`Writes a fallback models.json file and a provider extension that refreshes the relay catalog. Back up existing Pi configuration first.`}
                endpoint="/openai/v1/responses"
                model={plural(piModels.length, {
                  one: "# routed model",
                  other: "# routed models",
                })}
                snippet={snippets.pi}
              />
            ) : (
              <EmptyNotice
                title={t`No Pi routes`}
                body={t`Add an eligible OpenAI Responses route to generate a Pi provider config.`}
              />
            )}
          </TabsContent>
          <TabsContent value="dsh">
            {dshModels.length > 0 ? (
              <ClientSnippetCard
                title={t`DeepSeek Harness provider`}
                description={t`Set TOKEN_TOXICATION_API_KEY in the harness process environment, then merge these llm-pi-ai provider routes into ~/.dsh/settings.yaml. Keep existing keys; the harness applies changes on the next request.`}
                endpoint="settings.yaml · llm-pi-ai"
                model={t`Chosen in DeepSeek Harness`}
                snippet={snippets.dsh}
              />
            ) : (
              <EmptyNotice
                title={t`No DeepSeek Harness routes`}
                body={t`Add an eligible OpenAI Chat, OpenAI Responses, or Anthropic Messages route to generate a DeepSeek Harness config.`}
              />
            )}
          </TabsContent>
        </Tabs>
      ) : null}
    </div>
  );
}

function ClientSnippetCard({
  title,
  description,
  endpoint,
  model,
  snippet,
}: {
  title: string;
  description: string;
  endpoint: string;
  model: string;
  snippet: string;
}) {
  return (
    <Card>
      <CardHeader>
        <CardTitle>{title}</CardTitle>
        <CardDescription>{description}</CardDescription>
        <CardAction>
          <Button type="button" onClick={() => copyText(snippet)}>
            <ClipboardCopyIcon data-icon="inline-start" />
            <Trans>Copy</Trans>
          </Button>
        </CardAction>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        <div className="grid gap-3 md:grid-cols-2">
          <SettingRow label={t`Route`} value={endpoint} />
          <SettingRow label={t`Model`} value={model || t`not set`} />
        </div>
        <pre className="max-h-[560px] overflow-auto rounded-md border bg-muted/40 p-3 text-xs leading-5">
          <code>{snippet}</code>
        </pre>
      </CardContent>
    </Card>
  );
}

export function buildClientSetupSnippets({
  apiKey,
  serviceOrigin,
  opencodeModels,
  piModels,
  dshModels,
}: {
  apiKey: string;
  serviceOrigin: string;
  opencodeModels: OpencodeModelOption[];
  piModels: ClientModelOption[];
  dshModels: DshModelOption[];
}) {
  const origin = serviceOrigin.replace(/\/+$/, "");
  const relayApiKey = apiKey.trim() || "tokentoxication-REPLACE_ME";
  const openaiBaseUrl = `${origin}/openai/v1`;
  const anthropicBaseUrl = `${origin}/anthropic`;
  const opencodeProvider = "token-toxication";
  const opencodeCatalogPlugin = opencodeCatalogPluginSource(openaiBaseUrl, opencodeModels);
  const piCatalogExtension = piCatalogExtensionSource(openaiBaseUrl);
  const opencodeConfig = JSON.stringify(
    {
      $schema: "https://opencode.ai/config.json",
      provider: {
        [opencodeProvider]: {
          name: "Token Toxication",
          options: {
            baseURL: openaiBaseUrl,
            apiKey: "{env:TOKEN_TOXICATION_API_KEY}",
          },
        },
      },
    },
    null,
    2,
  );
  const piModelsConfig = JSON.stringify(
    {
      providers: {
        "token-toxication": {
          name: "Token Toxication",
          baseUrl: openaiBaseUrl,
          api: "openai-responses",
          apiKey: "$TOKEN_TOXICATION_API_KEY",
          models: withSpeedTierAliases(piModels).map((model) => ({
            id: model.id,
            name: model.displayName,
            reasoning: true,
          })),
        },
      },
    },
    null,
    2,
  );
  const dshProviderSections = (Object.keys(dshProviderIds) as DshProtocol[])
    .map((protocol) =>
      dshProviderYaml(
        protocol,
        (protocol === "anthropic" ? dshModels : withSpeedTierAliases(dshModels)).filter(
          (model) => model.protocols[protocol],
        ),
        openaiBaseUrl,
        anthropicBaseUrl,
      ),
    )
    .filter((section) => section !== "");
  const dshSnippet = [
    "# Set TOKEN_TOXICATION_API_KEY in the DeepSeek Harness process environment.",
    `# Model discovery endpoints: ${openaiBaseUrl}/models and ${anthropicBaseUrl}/v1/models`,
    "# Merge the sections below into ~/.dsh/settings.yaml, then reload the",
    "# harness page. Keep your existing keys (ui-theme, permission, ...).",
    "",
    "llm-pi-ai:",
    "  providers:",
    ...dshProviderSections,
  ].join("\n");

  return {
    openaiBaseUrl,
    anthropicBaseUrl,
    codexEnvironment: [
      `export TOKEN_TOXICATION_API_KEY=${shellQuote(relayApiKey)}`,
      "mkdir -p ~/.codex",
      `curl --fail --silent --show-error --location \\
  --header "Authorization: Bearer $TOKEN_TOXICATION_API_KEY" \\
  ${shellQuote(`${openaiBaseUrl}/codex/models`)} \\
  > ~/.codex/token-toxication-model-catalog.json`,
    ].join("\n"),
    codexConfig: [
      "# Set TOKEN_TOXICATION_API_KEY in the Codex process environment.",
      "# Run the Codex environment snippet first to download the Desktop compatibility catalog.",
      "# Merge into ~/.codex/config.toml; keep your own model and reasoning settings.",
      "",
      `model_provider = ${tomlString("token-toxication")}`,
      "",
      "[model_providers.token-toxication]",
      `name = ${tomlString("Token Toxication")}`,
      `base_url = ${tomlString(openaiBaseUrl)}`,
      `env_key = ${tomlString("TOKEN_TOXICATION_API_KEY")}`,
      `wire_api = ${tomlString("responses")}`,
      "supports_websockets = true",
      `model_catalog_json = ${tomlString("token-toxication-model-catalog.json")}`,
      `model_catalog_url = ${tomlString(`${openaiBaseUrl}/codex/models`)}`,
    ].join("\n"),
    claudeCode: [
      `export ANTHROPIC_BASE_URL=${shellQuote(anthropicBaseUrl)}`,
      `export ANTHROPIC_AUTH_TOKEN=${shellQuote(relayApiKey)}`,
      "export CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=1",
    ].join("\n"),
    opencode: [
      `export TOKEN_TOXICATION_API_KEY=${shellQuote(relayApiKey)}`,
      "mkdir -p .opencode/plugins",
      "cat > .opencode/plugins/token-toxication-models.ts <<'TS'",
      opencodeCatalogPlugin,
      "TS",
      "cat > opencode.json <<'JSON'",
      opencodeConfig,
      "JSON",
      "",
      "opencode",
    ].join("\n"),
    pi: [
      `export TOKEN_TOXICATION_API_KEY=${shellQuote(relayApiKey)}`,
      "mkdir -p ~/.pi/agent",
      "mkdir -p ~/.pi/agent/extensions",
      "cat > ~/.pi/agent/models.json <<'JSON'",
      piModelsConfig,
      "JSON",
      "cat > ~/.pi/agent/extensions/token-toxication-models.mjs <<'JS'",
      piCatalogExtension,
      "JS",
      "pi",
    ].join("\n"),
    dsh: dshSnippet,
  };
}
