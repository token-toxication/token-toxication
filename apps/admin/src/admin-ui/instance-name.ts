import { useEffect, useState } from "react";

const PRODUCT_NAME = "Token Toxication";

// Reads the optional TT_INSTANCE_NAME label from /health so operators can tell
// deployments apart; also prefixes the browser tab title with it.
export function useInstanceName(): string | undefined {
  const [instanceName, setInstanceName] = useState<string>();

  useEffect(() => {
    let cancelled = false;
    fetch("/health")
      .then((response) => (response.ok ? response.json() : undefined))
      .then((body: { instanceName?: unknown } | undefined) => {
        if (cancelled || typeof body?.instanceName !== "string") return;
        setInstanceName(body.instanceName);
        document.title = `${body.instanceName} · ${PRODUCT_NAME}`;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);

  return instanceName;
}
