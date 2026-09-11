import { Trans, useLingui } from "@lingui/react/macro";

import { SettingRow } from "./shared";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";

export function SettingsView() {
  const { t } = useLingui();

  return (
    <Tabs defaultValue="runtime">
      <TabsList>
        <TabsTrigger value="runtime">
          <Trans>Runtime</Trans>
        </TabsTrigger>
        <TabsTrigger value="headers">
          <Trans>Headers</Trans>
        </TabsTrigger>
      </TabsList>
      <TabsContent value="runtime">
        <Card>
          <CardHeader>
            <CardTitle>
              <Trans>Runtime</Trans>
            </CardTitle>
            <CardDescription>
              <Trans>Current service assumptions exposed by the frontend.</Trans>
            </CardDescription>
          </CardHeader>
          <CardContent className="grid gap-4 md:grid-cols-2">
            <SettingRow label={t`Anthropic Messages`} value="/anthropic/v1/messages" />
            <SettingRow label={t`Codex Responses`} value="/openai/v1/responses" />
            <SettingRow label={t`OpenAI Chat`} value="/openai/v1/chat/completions" />
            <SettingRow
              label={t`Gemini GenerateContent`}
              value="/gemini/v1beta/models/{model}:generateContent"
            />
            <SettingRow label={t`Admin API`} value="/admin/api" />
            <SettingRow label={t`Storage`} value="SQLite" />
          </CardContent>
        </Card>
      </TabsContent>
      <TabsContent value="headers">
        <Card>
          <CardHeader>
            <CardTitle>
              <Trans>Forwarded headers</Trans>
            </CardTitle>
            <CardDescription>
              <Trans>Headers preserved or supplied by the Rust relay.</Trans>
            </CardDescription>
          </CardHeader>
          <CardContent className="flex flex-col gap-3">
            {[
              "x-api-key",
              "x-goog-api-key",
              "authorization: Bearer",
              "anthropic-version",
              "anthropic-beta",
            ].map((item) => (
              <div key={item} className="rounded-md border p-3 font-mono text-sm">
                {item}
              </div>
            ))}
          </CardContent>
        </Card>
      </TabsContent>
    </Tabs>
  );
}
