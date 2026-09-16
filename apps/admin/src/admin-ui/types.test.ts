import { describe, expect, it } from "vite-plus/test";

import type { ProviderModelRoute } from "../types";
import { providerRouteRequestFromForm, updateProviderRouteAndRefresh } from "./use-admin-workspace";
import { providerRouteEditorKey, routeFormFromRoute, type ProviderRouteForm } from "./types";

const route: ProviderModelRoute = {
  id: "route-1",
  publicModelId: "gpt-6-astra",
  providerAccountId: "account-1",
  upstreamModelId: "gpt-6-astra-2026-09-01",
  wireApi: "openai-responses",
  role: "backup",
  enabled: false,
  weight: 25,
  status: "healthy",
  stripParams: ["temperature", "top_p"],
  createdAt: "2026-09-16T00:00:00Z",
};

const form: ProviderRouteForm = {
  publicModelId: "gpt-6-astra",
  providerAccountId: "account-2",
  upstreamModelId: "gpt-6-astra-latest",
  wireApi: "openai-responses",
  role: "primary",
  enabled: true,
  weight: "250",
  stripParams: " top_p, temperature, top_p ",
};

describe("provider route editing", () => {
  it("hydrates every editable provider route field", () => {
    expect(routeFormFromRoute(route)).toEqual({
      publicModelId: "gpt-6-astra",
      providerAccountId: "account-1",
      upstreamModelId: "gpt-6-astra-2026-09-01",
      wireApi: "openai-responses",
      role: "backup",
      enabled: false,
      weight: "25",
      stripParams: "temperature, top_p",
    });
  });

  it("keys the editor by persistent route identity", () => {
    const duplicateBinding = { ...route, id: "route-2" };

    expect(providerRouteEditorKey(duplicateBinding)).not.toBe(providerRouteEditorKey(route));
  });

  it("normalizes the shared create and update payload", () => {
    expect(providerRouteRequestFromForm(form)).toEqual({
      publicModelId: "gpt-6-astra",
      providerAccountId: "account-2",
      upstreamModelId: "gpt-6-astra-latest",
      wireApi: "openai-responses",
      role: "primary",
      enabled: true,
      weight: 250,
      stripParams: ["temperature", "top_p"],
    });
  });

  it("updates the selected route before refreshing persisted data", async () => {
    const events: string[] = [];

    await updateProviderRouteAndRefresh(
      async (id, payload) => {
        events.push(`update:${id}:${JSON.stringify(payload.stripParams)}`);
      },
      async () => {
        events.push("refresh");
      },
      route,
      form,
    );

    expect(events).toEqual(['update:route-1:["temperature","top_p"]', "refresh"]);
  });
});
