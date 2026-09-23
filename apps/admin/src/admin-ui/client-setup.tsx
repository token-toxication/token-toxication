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
                  description={t`Set the relay key in the environment that starts Codex. Do not put it in config.toml.`}
                  endpoint="TOKEN_TOXICATION_API_KEY"
                  model={t`Chosen in Codex`}
                  snippet={snippets.codexEnvironment}
                />
                <ClientSnippetCard
                  title={t`Codex provider configuration`}
                  description={t`Merge this TOML into ~/.codex/config.toml. Keep your own model preferences and remove any earlier Token Toxication model_catalog_json override.`}
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
                description={t`Binds each model to the AI SDK matching its configured OpenAI route.`}
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
                description={t`Writes a complete Pi models.json file using the OpenAI Responses API. Back up any existing Pi configuration first.`}
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
          models: Object.fromEntries(
            opencodeModels.map((model) => [
              model.id,
              {
                name: model.displayName,
                provider: {
                  npm:
                    model.wireApi === "openai-responses"
                      ? "@ai-sdk/openai"
                      : "@ai-sdk/openai-compatible",
                },
              },
            ]),
          ),
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
          models: piModels.map((model) => ({
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
        dshModels.filter((model) => model.protocols[protocol]),
        openaiBaseUrl,
        anthropicBaseUrl,
      ),
    )
    .filter((section) => section !== "");
  const dshSnippet = [
    "# Set TOKEN_TOXICATION_API_KEY in the DeepSeek Harness process environment.",
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
    codexEnvironment: `export TOKEN_TOXICATION_API_KEY=${shellQuote(relayApiKey)}`,
    codexConfig: [
      "# Set TOKEN_TOXICATION_API_KEY in the Codex process environment.",
      "# Merge into ~/.codex/config.toml; keep your own model and reasoning settings.",
      "# Remove an earlier Token Toxication model_catalog_json override to use Codex's catalog.",
      "",
      `model_provider = ${tomlString("token-toxication")}`,
      "",
      "[model_providers.token-toxication]",
      `name = ${tomlString("Token Toxication")}`,
      `base_url = ${tomlString(openaiBaseUrl)}`,
      `env_key = ${tomlString("TOKEN_TOXICATION_API_KEY")}`,
      `wire_api = ${tomlString("responses")}`,
    ].join("\n"),
    claudeCode: [
      `export ANTHROPIC_BASE_URL=${shellQuote(anthropicBaseUrl)}`,
      `export ANTHROPIC_AUTH_TOKEN=${shellQuote(relayApiKey)}`,
      "export CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=1",
    ].join("\n"),
    opencode: [
      `export TOKEN_TOXICATION_API_KEY=${shellQuote(relayApiKey)}`,
      "cat > opencode.json <<'JSON'",
      opencodeConfig,
      "JSON",
      "",
      "opencode",
    ].join("\n"),
    pi: [
      `export TOKEN_TOXICATION_API_KEY=${shellQuote(relayApiKey)}`,
      "mkdir -p ~/.pi/agent",
      "cat > ~/.pi/agent/models.json <<'JSON'",
      piModelsConfig,
      "JSON",
      "pi",
    ].join("\n"),
    dsh: dshSnippet,
  };
}
