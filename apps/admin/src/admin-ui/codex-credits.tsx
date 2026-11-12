import { useState } from "react";
import { t } from "@lingui/core/macro";
import { Trans } from "@lingui/react/macro";
import { LoaderCircleIcon } from "lucide-react";

import type { ProviderAccount } from "../types";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";

export function CodexCreditsControl({
  account,
  creditsUsable,
  onChange,
}: {
  account: ProviderAccount;
  creditsUsable: boolean;
  onChange: (allow: boolean) => Promise<void>;
}) {
  const [confirming, setConfirming] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const allowed = account.allowCredits === true;
  const canEnable = creditsUsable && account.isActive && account.status !== "blocked";

  async function save(allow: boolean) {
    setSaving(true);
    setError(null);
    try {
      await onChange(allow);
      setConfirming(false);
    } catch (error) {
      setError(error instanceof Error ? error.message : t`Unable to update credits permission`);
    } finally {
      setSaving(false);
    }
  }

  const failure = error ? (
    <Alert variant="destructive" role="alert">
      <AlertTitle>
        <Trans>Unable to update credits permission</Trans>
      </AlertTitle>
      <AlertDescription>{error}</AlertDescription>
    </Alert>
  ) : null;

  return (
    <Dialog open={confirming} onOpenChange={(open) => !saving && setConfirming(open)}>
      <div className="flex flex-col gap-3" aria-busy={saving}>
        <Alert>
          <AlertTitle className="flex flex-wrap items-center gap-2">
            <Trans>Credits continuation</Trans>
            <Badge variant={allowed ? "secondary" : "outline"}>
              {allowed ? <Trans>Allowed</Trans> : <Trans>Off</Trans>}
            </Badge>
          </AlertTitle>
          <AlertDescription>
            <p>
              <Trans>
                Allow this account to continue with available usage credits after its included quota
                runs out. Permission stays on until you disable it. This is not a spending cap.
              </Trans>
            </p>
            {allowed ? (
              <Button
                variant="outline"
                size="sm"
                disabled={saving}
                onClick={() => void save(false)}
              >
                {saving ? (
                  <LoaderCircleIcon data-icon="inline-start" className="animate-spin" />
                ) : null}
                <Trans>Disable credits continuation</Trans>
              </Button>
            ) : (
              <>
                <DialogTrigger asChild>
                  <Button size="sm" disabled={!canEnable || saving}>
                    <Trans>Allow credits continuation</Trans>
                  </Button>
                </DialogTrigger>
                {!canEnable ? (
                  <p>
                    <Trans>
                      Requires an active account, usable credits and no upstream spending or
                      authentication restriction. Reopen this dialog to check again.
                    </Trans>
                  </p>
                ) : null}
              </>
            )}
          </AlertDescription>
        </Alert>
        {!confirming ? failure : null}
      </div>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>
            <Trans>Allow future credits usage?</Trans>
          </DialogTitle>
          <DialogDescription>
            <Trans>
              All clients routed to this account may consume credits after included quota is
              exhausted, now and in future cycles. OpenAI can debit credits automatically. This
              permission persists across restarts and does not change OpenAI auto-reload or spend
              controls. Disabling it later does not cancel running requests or refund charges.
            </Trans>
          </DialogDescription>
        </DialogHeader>
        {failure}
        <DialogFooter>
          <Button variant="outline" disabled={saving} onClick={() => setConfirming(false)}>
            <Trans>Cancel</Trans>
          </Button>
          <Button disabled={saving} onClick={() => void save(true)}>
            {saving ? <LoaderCircleIcon data-icon="inline-start" className="animate-spin" /> : null}
            <Trans>Allow credits continuation</Trans>
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
