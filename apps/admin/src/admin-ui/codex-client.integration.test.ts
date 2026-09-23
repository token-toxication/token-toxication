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
    "uses Codex's default GPT-6 without a static catalog and forwards its native Lite request",
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
        await writeFile(path.join(configDirectory, "config.toml"), snippets.codexConfig);
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
