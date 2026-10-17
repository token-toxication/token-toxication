/// <reference types="node" />

import { execFileSync, spawn, type ChildProcess } from "node:child_process";
import { once } from "node:events";
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { createServer, type Server } from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import { createInterface } from "node:readline";

import { describe, expect, it } from "vite-plus/test";

import { buildClientSetupSnippets } from "./client-setup";

// Opt-in contract test: an isolated Codex home, relay, and synthetic upstream.
// Run with a Codex binary built from the reference revision recorded in the docs.
const codexBinary = process.env.TT_CODEX_BIN;
const relayBinary = process.env.TT_RELAY_BIN;
if (codexBinary && !path.isAbsolute(codexBinary)) {
  throw new Error("TT_CODEX_BIN must be an absolute path");
}
if (relayBinary && !path.isAbsolute(relayBinary)) {
  throw new Error("TT_RELAY_BIN must be an absolute path");
}

async function listen(server: Server) {
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const address = server.address();
  if (!address || typeof address === "string") throw new Error("missing server address");
  return `http://127.0.0.1:${address.port}`;
}

async function close(server: Server) {
  const closed = once(server, "close");
  server.close();
  server.closeAllConnections();
  await closed;
}

async function stop(child: ChildProcess) {
  if (!child.pid || child.exitCode !== null || child.signalCode !== null) return;
  const closed = once(child, "close");
  child.kill("SIGTERM");
  const timer = setTimeout(() => child.kill("SIGKILL"), 3000);
  try {
    await closed;
  } finally {
    clearTimeout(timer);
  }
}

async function modelList(binary: string, env: NodeJS.ProcessEnv, cwd: string) {
  const client = spawn(binary, ["app-server", "-c", "features.plugins=false"], { cwd, env });
  const lines = createInterface({ input: client.stdout });
  const replies = new Map<number, { result?: { data: { model: string }[] }; error?: unknown }>();
  lines.on("line", (line) => {
    const reply = JSON.parse(line);
    if (typeof reply.id === "number") replies.set(reply.id, reply);
  });
  client.stderr.resume();
  try {
    client.stdin.write(
      JSON.stringify({
        id: 1,
        method: "initialize",
        params: { clientInfo: { name: "relay-test", version: "1" } },
      }) + "\n",
    );
    await expect.poll(() => replies.has(1), { timeout: 10000 }).toBe(true);
    expect(replies.get(1)?.error).toBeUndefined();
    client.stdin.write(JSON.stringify({ method: "initialized" }) + "\n");
    client.stdin.write(JSON.stringify({ id: 2, method: "model/list", params: {} }) + "\n");
    await expect.poll(() => replies.has(2), { timeout: 10000 }).toBe(true);
    expect(replies.get(2)?.error).toBeUndefined();
    return replies.get(2)?.result?.data ?? [];
  } finally {
    lines.close();
    await stop(client);
  }
}

function responseStream(index: number) {
  const item =
    index === 1
      ? {
          type: "custom_tool_call",
          id: "tool_mock",
          call_id: "call_mock_exec",
          name: "exec",
          input: 'text(await tools.exec_command({cmd:"pwd",login:false,max_output_tokens:100}));',
        }
      : {
          type: "message",
          id: `msg_${index}`,
          role: "assistant",
          status: "completed",
          content: [{ type: "output_text", text: "synthetic relay complete", annotations: [] }],
        };
  const response = {
    id: `resp_${index}`,
    status: "completed",
    output: [item],
    usage: { input_tokens: 10, output_tokens: 2, total_tokens: 12 },
  };
  return [
    { type: "response.created", response: { ...response, status: "in_progress", output: [] } },
    { type: "response.output_item.added", output_index: 0, item },
    { type: "response.output_item.done", output_index: 0, item },
    { type: "response.completed", response },
  ]
    .map((event) => `event: ${event.type}\ndata: ${JSON.stringify(event)}\n\n`)
    .join("");
}

describe.skipIf(!codexBinary)("Codex built-in catalog with relay provider", () => {
  it(
    "uses a bundled GPT-6 model without a static catalog and forwards its native Lite request",
    { timeout: 60000 },
    async () => {
      if (!codexBinary || !relayBinary) throw new Error("Set TT_CODEX_BIN and TT_RELAY_BIN");
      const version = execFileSync(codexBinary, ["--version"], {
        encoding: "utf8",
        timeout: 10000,
      }).trim();
      console.info(`Testing ${version} with Codex's built-in model catalog`);
      const root = await mkdtemp(path.join(tmpdir(), "token-toxication-codex-"));
      const configDirectory = path.join(root, "codex");
      const workspace = path.join(root, "workspace");
      await mkdir(configDirectory);
      await mkdir(workspace);
      const requests: { body: Record<string, unknown>; lite: string | string[] | undefined }[] = [];
      const upstream = createServer(async (request, response) => {
        if (request.method !== "POST" || request.url !== "/v1/responses") {
          response.writeHead(404).end();
          return;
        }
        const chunks: Buffer[] = [];
        for await (const chunk of request) chunks.push(Buffer.from(chunk));
        requests.push({
          body: JSON.parse(Buffer.concat(chunks).toString()),
          lite: request.headers["x-openai-internal-codex-responses-lite"],
        });
        response
          .writeHead(200, { "content-type": "text/event-stream" })
          .end(responseStream(requests.length));
      });
      let relay: ChildProcess | undefined;
      let client: ChildProcess | undefined;
      try {
        const upstreamUrl = await listen(upstream);
        const reservation = createServer();
        const relayUrl = await listen(reservation);
        await close(reservation);
        relay = spawn(
          relayBinary,
          [
            "--bind-addr",
            new URL(relayUrl).host,
            "--database-path",
            path.join(root, "relay.sqlite3"),
            "--admin-password",
            "synthetic-admin-password",
            "--https-mode",
            "off",
          ],
          { cwd: workspace, env: { PATH: process.env.PATH }, stdio: "ignore" },
        );
        const relayProcess = relay;
        await expect
          .poll(
            async () => {
              if (relayProcess.exitCode !== null) throw new Error("relay exited before readiness");
              return fetch(`${relayUrl}/health`, { signal: AbortSignal.timeout(2000) })
                .then((res) => res.ok)
                .catch(() => false);
            },
            { timeout: 10000 },
          )
          .toBe(true);
        async function admin(endpoint: string, body: Record<string, unknown>, token?: string) {
          const response = await fetch(`${relayUrl}/admin/api/${endpoint}`, {
            method: "POST",
            headers: {
              "content-type": "application/json",
              ...(token ? { authorization: `Bearer ${token}` } : {}),
            },
            body: JSON.stringify(body),
            signal: AbortSignal.timeout(5000),
          });
          if (!response.ok) throw new Error(`admin ${endpoint}: HTTP ${response.status}`);
          return response.json();
        }
        const login = await admin("auth/login", {
          username: "admin",
          password: "synthetic-admin-password",
        });
        const key = await admin("api-keys", { name: "mock client" }, login.token);
        const account = await admin(
          "provider-accounts",
          {
            name: "mock upstream",
            baseUrl: upstreamUrl,
            provider: "openai-compatible",
            authMode: "bearer",
            wireApi: "openai-responses",
            apiKey: "synthetic-provider-key",
          },
          login.token,
        );
        await admin("model-catalog", { id: "gpt-6-astra", family: "other" }, login.token);
        await admin(
          "provider-model-routes",
          {
            publicModelId: "gpt-6-astra",
            providerAccountId: account.data.id,
            upstreamModelId: "mock-coding-upstream",
            wireApi: "openai-responses",
          },
          login.token,
        );
        const snippets = buildClientSetupSnippets({
          apiKey: key.secret,
          serviceOrigin: relayUrl,
          opencodeModels: [],
          piModels: [],
          dshModels: [],
        });
        // This fixture exercises the HTTP Responses/Lite contract below. The
        // setup snippet enables WebSockets for production, but Codex 0.162.1
        // otherwise selects the relay WebSocket path and bypasses this HTTP
        // mock upstream entirely.
        await writeFile(
          path.join(configDirectory, "config.toml"),
          snippets.codexConfig.replace("supports_websockets = true", "supports_websockets = false"),
        );
        const env = {
          PATH: process.env.PATH,
          HOME: root,
          CODEX_HOME: configDirectory,
          TOKEN_TOXICATION_API_KEY: key.secret,
        };
        const listed = await modelList(codexBinary, env, workspace);
        expect(listed.map((entry) => entry.model)).toContain("gpt-6-astra");
        const config = await readFile(path.join(configDirectory, "config.toml"), "utf8");
        expect(config).not.toMatch(/^\s*(?:model|model_catalog_json)\s*=/m);
        client = spawn(
          codexBinary,
          [
            "exec",
            "--skip-git-repo-check",
            "--json",
            "--cd",
            workspace,
            "-c",
            "features.plugins=false",
            "-m",
            "gpt-6-astra",
            "Reply with the synthetic response.",
          ],
          { cwd: workspace, env, stdio: ["ignore", "pipe", "pipe"] },
        );
        let output = "";
        client.stdout?.on("data", (data) => {
          output += data;
        });
        client.stderr?.on("data", (data) => {
          output += data;
        });
        const timer = setTimeout(() => client?.kill("SIGTERM"), 45000);
        try {
          const [code] = await once(client, "close");
          expect(code, output).toBe(0);
        } finally {
          clearTimeout(timer);
        }
        expect(output).toContain("synthetic relay complete");
        expect(requests).toHaveLength(2);
        for (const request of requests) {
          expect(request.lite).toBe("true");
          expect(request.body.model).toBe("mock-coding-upstream");
          expect(request.body).not.toHaveProperty("tools");
        }
        expect(requests[0].body.input).toEqual(
          expect.arrayContaining([
            expect.objectContaining({ type: "additional_tools", role: "developer" }),
          ]),
        );
        expect(requests[1].body.input).toEqual(
          expect.arrayContaining([
            expect.objectContaining({
              type: "custom_tool_call_output",
              call_id: "call_mock_exec",
            }),
          ]),
        );
        expect(JSON.stringify(requests[1].body.input)).toContain(workspace);
      } finally {
        if (client) await stop(client);
        if (relay) await stop(relay);
        if (upstream.listening) await close(upstream);
        await rm(root, { recursive: true, force: true, maxRetries: 3, retryDelay: 100 });
      }
    },
  );
});

// The relay serves a Codex-native catalog by proxying the subscription account's
// own upstream catalog. This proves the generated `model_catalog_url` config is
// accepted by a real Codex build and that the model list matches what TT serves.
describe.skipIf(!codexBinary)("Codex relay model catalog", () => {
  it("serves the account catalog through model_catalog_url", { timeout: 60000 }, async () => {
    if (!codexBinary || !relayBinary) throw new Error("Set TT_CODEX_BIN and TT_RELAY_BIN");
    const version = execFileSync(codexBinary, ["--version"], {
      encoding: "utf8",
      timeout: 10000,
    }).trim();
    console.info(`Testing ${version} against the relay model catalog`);
    const root = await mkdtemp(path.join(tmpdir(), "token-toxication-catalog-"));
    const configDirectory = path.join(root, "codex");
    const workspace = path.join(root, "workspace");
    await mkdir(configDirectory);
    await mkdir(workspace);

    const catalogRequests: {
      url: string;
      headers: Record<string, string | string[] | undefined>;
    }[] = [];
    const turnRequests: Record<string, unknown>[] = [];
    const instructions = "You are Codex, a coding agent.".repeat(4);
    // Codex's `ModelInfo` has several required fields with no serde default, so a
    // partial entry makes the client reject the whole catalog. This mirrors the
    // shape the real upstream catalog serves.
    const codexModel = (
      slug: string,
      displayName: string,
      contextWindow: number,
      priority = 2,
    ) => ({
      slug,
      display_name: displayName,
      description: null,
      supported_reasoning_levels: [{ effort: "low", description: "Fast" }],
      shell_type: "unified_exec",
      visibility: "list",
      supported_in_api: true,
      priority,
      availability_nux: null,
      upgrade: null,
      support_verbosity: false,
      default_verbosity: null,
      apply_patch_tool_type: null,
      truncation_policy: { mode: "tokens", limit: 10000 },
      experimental_supported_tools: [],
      context_window: contextWindow,
      max_context_window: 872_000,
      // Upstream declares Fast as a native service tier; the relay must pass
      // this through so Codex can offer the tier at all.
      service_tiers: [{ id: "priority", name: "Fast", description: "2x speed, increased usage" }],
      additional_speed_tiers: ["fast"],
      model_messages: { instructions_template: instructions },
    });
    const upstream = createServer(async (request, response) => {
      if (request.method === "GET" && request.url?.startsWith("/codex/models")) {
        catalogRequests.push({ url: request.url ?? "", headers: request.headers });
        const withoutInstructions = codexModel("gpt-reserve", "GPT-Reserve", 272_000, 9);
        delete (withoutInstructions as { model_messages?: unknown }).model_messages;
        response.writeHead(200, { "content-type": "application/json" }).end(
          JSON.stringify({
            models: [
              codexModel("gpt-6.1-sol", "GPT-6.1-Sol", 500_000),
              codexModel("gpt-6-astra", "GPT-6-Astra", 272_000),
              // TT has no relay route for this one, but the client's picker needs
              // it: a catalog that omits account models breaks the whole list.
              codexModel("gpt-5.6-sol", "GPT-5.6-Sol", 272_000, 3),
              // The client cannot parse a catalog containing this entry, so the
              // relay must drop the entry rather than the account catalog.
              withoutInstructions,
            ],
          }),
        );
        return;
      }
      if (request.method === "POST" && request.url === "/codex/responses") {
        const chunks: Buffer[] = [];
        for await (const chunk of request) chunks.push(Buffer.from(chunk));
        turnRequests.push(JSON.parse(Buffer.concat(chunks).toString()));
        // The first response asks for a tool call and the second finishes the
        // turn, mirroring a real Code Mode round trip.
        response
          .writeHead(200, { "content-type": "text/event-stream" })
          .end(responseStream(turnRequests.length));
        return;
      }
      response.writeHead(404).end();
    });

    let relay: ChildProcess | undefined;
    try {
      const upstreamUrl = await listen(upstream);
      const reservation = createServer();
      const relayUrl = await listen(reservation);
      await close(reservation);
      relay = spawn(
        relayBinary,
        [
          "--bind-addr",
          new URL(relayUrl).host,
          "--database-path",
          path.join(root, "relay.sqlite3"),
          "--admin-password",
          "synthetic-admin-password",
          "--https-mode",
          "off",
        ],
        { cwd: workspace, env: { PATH: process.env.PATH }, stdio: "ignore" },
      );
      const relayProcess = relay;
      await expect
        .poll(
          async () => {
            if (relayProcess.exitCode !== null) throw new Error("relay exited before readiness");
            return fetch(`${relayUrl}/health`, { signal: AbortSignal.timeout(2000) })
              .then((res) => res.ok)
              .catch(() => false);
          },
          { timeout: 10000 },
        )
        .toBe(true);

      async function admin(endpoint: string, body: Record<string, unknown>, token?: string) {
        const response = await fetch(`${relayUrl}/admin/api/${endpoint}`, {
          method: "POST",
          headers: {
            "content-type": "application/json",
            ...(token ? { authorization: `Bearer ${token}` } : {}),
          },
          body: JSON.stringify(body),
          signal: AbortSignal.timeout(5000),
        });
        if (!response.ok) throw new Error(`admin ${endpoint}: HTTP ${response.status}`);
        return response.json();
      }

      const login = await admin("auth/login", {
        username: "admin",
        password: "synthetic-admin-password",
      });
      const key = await admin("api-keys", { name: "mock client" }, login.token);
      const account = await admin(
        "provider-accounts",
        {
          name: "mock subscription",
          baseUrl: upstreamUrl,
          provider: "codex-subscription",
          authMode: "codex-oauth",
          wireApi: "openai-responses",
          apiKey: JSON.stringify({
            type: "token-toxication-codex-oauth-v1",
            refresh: "synthetic-refresh",
            access: "synthetic-access",
            expires: Date.now() + 3_600_000,
            accountId: "synthetic-account",
          }),
        },
        login.token,
      );
      await admin("model-catalog", { id: "gpt-6.1-sol", family: "openai" }, login.token);
      await admin(
        "provider-model-routes",
        {
          publicModelId: "gpt-6.1-sol",
          providerAccountId: account.data.id,
          upstreamModelId: "gpt-6.1-sol",
          wireApi: "openai-responses",
        },
        login.token,
      );

      const snippets = buildClientSetupSnippets({
        apiKey: key.secret,
        serviceOrigin: relayUrl,
        opencodeModels: [],
        piModels: [],
        dshModels: [],
      });
      const catalogResponse = await fetch(`${relayUrl}/openai/v1/codex/models`, {
        headers: { Authorization: `Bearer ${key.secret}` },
      });
      expect(catalogResponse.ok).toBe(true);
      await writeFile(
        path.join(configDirectory, "token-toxication-model-catalog.json"),
        await catalogResponse.text(),
      );
      await writeFile(
        path.join(configDirectory, "config.toml"),
        snippets.codexConfig
          // The mock upstream speaks HTTP only; the setup snippet enables
          // WebSockets for production.
          .replace("supports_websockets = true", "supports_websockets = false"),
      );

      const env = {
        PATH: process.env.PATH,
        HOME: root,
        CODEX_HOME: configDirectory,
        TOKEN_TOXICATION_API_KEY: key.secret,
      };
      const listed = await modelList(codexBinary, env, workspace);
      const ids = listed.map((entry) => entry.model);
      expect(ids).toContain("gpt-6.1-sol");
      expect(ids).toContain("gpt-6-astra");
      // Account models beyond TT's relay routes still reach the client, because
      // the client's model picker is rendered from this list.
      expect(ids).toContain("gpt-5.6-sol");
      // Fast is a service tier on the base model, not a separate model TT invents.
      expect(ids).not.toContain("gpt-6.1-sol-fast");
      // Dropped for the client's sake, not because of TT policy.
      expect(ids).not.toContain("gpt-reserve");

      expect(catalogRequests).toHaveLength(1);
      expect(catalogRequests[0].url).toContain("client_version=");
      expect(catalogRequests[0].headers.authorization).toBe("Bearer synthetic-access");
      expect(catalogRequests[0].headers["chatgpt-account-id"]).toBe("synthetic-account");

      // A Code Mode turn is two upstream requests. This is the ordinary path: a
      // model the catalog advertises, selected with the client's own Fast tier.
      const turn = spawn(
        codexBinary,
        [
          "exec",
          "--skip-git-repo-check",
          "--json",
          "--cd",
          workspace,
          "-c",
          "features.plugins=false",
          // The TUI derives this from the model's `default_service_tier`;
          // `codex exec` does not, so state it to prove the relay forwards it.
          "-c",
          "service_tier=priority",
          "-m",
          "gpt-6.1-sol",
          "Reply with the synthetic response.",
        ],
        { cwd: workspace, env, stdio: ["ignore", "pipe", "pipe"] },
      );
      let output = "";
      turn.stdout?.on("data", (data) => {
        output += data;
      });
      turn.stderr?.on("data", (data) => {
        output += data;
      });
      const turnTimer = setTimeout(() => turn.kill("SIGTERM"), 45000);
      try {
        const [code] = await once(turn, "close");
        expect(code, output).toBe(0);
      } finally {
        clearTimeout(turnTimer);
      }
      expect(turnRequests).toHaveLength(2);
      for (const request of turnRequests) {
        expect(request.model).toBe("gpt-6.1-sol");
        expect(request.service_tier).toBe("priority");
      }

      // A client may still ask for a `-fast` id even though the catalog does not
      // publish one. Routing resolves the alias to the base model, so the turn
      // does not fail with "no active provider account".
      const aliasTurn = spawn(
        codexBinary,
        [
          "exec",
          "--skip-git-repo-check",
          "--json",
          "--cd",
          workspace,
          "-c",
          "features.plugins=false",
          "-m",
          "gpt-6.1-sol-fast",
          "Reply with the synthetic response.",
        ],
        { cwd: workspace, env, stdio: ["ignore", "pipe", "pipe"] },
      );
      let aliasOutput = "";
      aliasTurn.stdout?.on("data", (data) => {
        aliasOutput += data;
      });
      aliasTurn.stderr?.on("data", (data) => {
        aliasOutput += data;
      });
      const aliasTimer = setTimeout(() => aliasTurn.kill("SIGTERM"), 45000);
      try {
        const [code] = await once(aliasTurn, "close");
        expect(code, aliasOutput).toBe(0);
      } finally {
        clearTimeout(aliasTimer);
      }
      const aliasRequests = turnRequests.slice(2);
      expect(aliasRequests.length).toBeGreaterThan(0);
      for (const request of aliasRequests) {
        expect(request.model).toBe("gpt-6.1-sol");
      }
    } finally {
      if (relay) await stop(relay);
      if (upstream.listening) await close(upstream);
      await rm(root, { recursive: true, force: true, maxRetries: 3, retryDelay: 100 });
    }
  });
});
