import { lazy, Suspense } from "react";
import { Link } from "react-router";
import { PlusIcon } from "lucide-react";
import { Trans, useLingui } from "@lingui/react/macro";

import { adminPaths } from "../admin-routes";
import { statusBadge, wireApiLabel } from "./helpers";
import { RequestLogsView } from "./logs";
import { EmptyNotice, LoadingState } from "./shared";
import type { ApiKey, ProviderAccount, Dashboard } from "../types";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";

const AnalyticsPanel = lazy(() => import("./analytics-panel"));

export function Overview({
  dashboard,
  apiKeys,
  accounts,
  onCreateKey,
  onCreateAccount,
}: {
  dashboard: Dashboard;
  apiKeys: readonly ApiKey[];
  accounts: readonly ProviderAccount[];
  onCreateKey: () => void;
  onCreateAccount: () => void;
}) {
  const { t } = useLingui();
  return (
    <div className="flex min-w-0 flex-col gap-5">
      <Suspense fallback={<LoadingState />}>
        <AnalyticsPanel />
      </Suspense>
      <Card>
        <CardHeader className="flex-row flex-wrap items-start justify-between gap-4">
          <div className="flex flex-col gap-1">
            <CardTitle>
              <Trans>Provider pool</Trans>
            </CardTitle>
            <CardDescription>
              <Trans>
                {dashboard.healthyAccounts}/{dashboard.totalAccounts} healthy accounts ·{" "}
                {dashboard.activeApiKeys}/{dashboard.totalApiKeys} active API keys
              </Trans>
            </CardDescription>
          </div>
          <div className="flex gap-2">
            <Button variant="outline" onClick={onCreateAccount}>
              <PlusIcon data-icon="inline-start" />
              <Trans>Account</Trans>
            </Button>
            <Button onClick={onCreateKey}>
              <PlusIcon data-icon="inline-start" />
              <Trans>API Key</Trans>
            </Button>
          </div>
        </CardHeader>
        <CardContent className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
          {dashboard.accounts.length === 0 ? (
            <EmptyNotice
              title={t`No provider accounts`}
              body={t`Add a provider account to relay traffic.`}
            />
          ) : (
            dashboard.accounts.map((account) => (
              <div key={account.id} className="flex items-center justify-between gap-4">
                <div className="min-w-0">
                  <Link
                    to={adminPaths.account(account.id)}
                    className="block truncate text-sm font-medium underline-offset-4 hover:underline"
                  >
                    {account.name}
                  </Link>
                  <div className="truncate text-xs text-muted-foreground">
                    {account.provider} · {wireApiLabel(account.wireApi)}
                  </div>
                </div>
                {statusBadge(account.status, account.isActive)}
              </div>
            ))
          )}
        </CardContent>
      </Card>
      <RequestLogsView
        logs={dashboard.recentRequests}
        apiKeys={apiKeys}
        accounts={accounts}
        compact
      />
    </div>
  );
}
