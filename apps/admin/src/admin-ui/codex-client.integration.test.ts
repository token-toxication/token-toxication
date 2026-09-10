/// <reference types="node" />

import { spawn, type ChildProcess } from "node:child_process";
import { once } from "node:events";
import { mkdtemp, mkdir, readFile, readdir, realpath, rm, writeFile } from "node:fs/promises";
import { createServer, type Server } from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import { createInterface } from "node:readline";

import { describe, expect, it } from "vite-plus/test";

import { buildClientSetupSnippets } from "./client-setup";
import { CODEX_ASTRA_INSTRUCTIONS } from "./codex-astra-instructions";

// Explicit opt-in: no real OpenAI endpoint, inherited credentials, or daily
// Codex configuration. The mock asks for pwd and a patch in a disposable cwd.
const codexBinary = process.env.TT_CODEX_BIN;
const relayBinary = process.env.TT_RELAY_BIN;

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

type Item = Record<string, unknown>;

async function checkModelPicker(binary: string, env: NodeJS.ProcessEnv, cwd: string) {
  const child = spawn(binary, ["app-server", "-c", "features.plugins=false"], { cwd, env });
  const lines = createInterface({ input: child.stdout });
  const replies = new Map<number, { result?: { data: Item[] }; error?: unknown }>();
  lines.on("line", (line) => {
    const reply = JSON.parse(line);
    if (typeof reply.id === "number") replies.set(reply.id, reply);
  });
  child.stderr.resume();
  try {
    child.stdin.write(
      JSON.stringify({
        id: 1,
        method: "initialize",
        params: {
          clientInfo: { name: "token-toxication-test", version: "1.0" },
        },
      }) + "\n",
    );
    await expect.poll(() => replies.has(1), { timeout: 10_000 }).toBe(true);
    expect(replies.get(1)?.error).toBeUndefined();
    child.stdin.write(JSON.stringify({ method: "initialized" }) + "\n");
    child.stdin.write(JSON.stringify({ id: 2, method: "model/list", params: {} }) + "\n");
    await expect.poll(() => replies.has(2), { timeout: 10_000 }).toBe(true);
    expect(replies.get(2)?.error).toBeUndefined();
    expect(replies.get(2)?.result?.data).toEqual([
      expect.objectContaining({
        model: "gpt-6-astra",
        defaultReasoningEffort: "low",
        inputModalities: ["text", "image"],
        supportedReasoningEfforts: ["low", "medium", "high", "xhigh", "max"].map(
          (reasoningEffort) => expect.objectContaining({ reasoningEffort }),
        ),
      }),
    ]);
  } finally {
    lines.close();
    await stop(child);
  }
}

type RequestBody = {
  model: string;
  instructions: string;
  reasoning: { effort: string; summary?: string };
  text: { verbosity: string };
  tools: Item[];
  input: Item[];
  parallel_tool_calls: boolean;
};

function responseStream(index: number, item: Item) {
  const response = {
    id: `resp_mock_${index}`,
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

describe.skipIf(!codexBinary)("Codex client through the Responses relay", () => {
  it.each([
    { name: "default effort", effort: undefined, contextWindow: undefined, usableContext: 258_400 },
    {
      name: "explicit max effort",
      effort: "max",
      contextWindow: undefined,
      usableContext: 258_400,
    },
    {
      name: "bounded context override",
      effort: "high",
      contextWindow: 900_000,
      usableContext: 828_400,
    },
  ])(
    "loads the generated setup with $name",
    async ({ effort, contextWindow, usableContext }) => {
      if (!codexBinary || !relayBinary) throw new Error("Set both TT_CODEX_BIN and TT_RELAY_BIN");
      if (!path.isAbsolute(codexBinary) || !path.isAbsolute(relayBinary)) {
        throw new Error("TT_CODEX_BIN and TT_RELAY_BIN must be absolute paths");
      }
      const root = await mkdtemp(path.join(tmpdir(), "token-toxication-codex-"));
      const configDirectory = path.join(root, "codex");
      const workspace = path.join(root, "workspace");
      await mkdir(configDirectory);
      await mkdir(workspace);
      const captures: RequestBody[] = [];
      const upstream = createServer(async (request, response) => {
        try {
          if (request.method !== "POST" || request.url !== "/v1/responses") {
            response.writeHead(404).end();
            return;
          }
          const chunks: Buffer[] = [];
          for await (const chunk of request) chunks.push(Buffer.from(chunk));
          const body: RequestBody = JSON.parse(Buffer.concat(chunks).toString());
          captures.push(body);
          const index = captures.length;
          let item: Item;
          if (index === 1) {
            item = {
              type: "function_call",
              id: "fc_mock",
              call_id: "call_mock_exec",
              name: "exec_command",
              arguments: JSON.stringify({ cmd: "pwd", login: false, max_output_tokens: 100 }),
            };
          } else if (index === 2) {
            item = {
              type: "custom_tool_call",
              id: "ct_mock",
              call_id: "call_mock_patch",
              name: "apply_patch",
              input:
                "*** Begin Patch\n*** Add File: compatibility.txt\n+mock tool completed\n*** End Patch",
            };
          } else if (index === 3) {
            item = {
              type: "function_call",
              id: "fc_image_mock",
              call_id: "call_mock_image",
              name: "view_image",
              arguments: JSON.stringify({
                path: path.join(workspace, "pixel.png"),
                detail: "original",
              }),
            };
          } else if (index === 4) {
            item = {
              type: "message",
              id: "msg_mock",
              role: "assistant",
              status: "completed",
              content: [
                { type: "output_text", text: "mock compatibility complete", annotations: [] },
              ],
            };
          } else {
            response.writeHead(500).end("unexpected extra request");
            return;
          }
          response
            .writeHead(200, { "content-type": "text/event-stream" })
            .end(responseStream(index, item));
        } catch {
          response.writeHead(500).end("invalid mock request");
        }
      });
      let relay: ChildProcess | undefined;
      let client: ChildProcess | undefined;
      try {
        const upstreamUrl = await listen(upstream);
        // Reserve then release an ephemeral port; readiness also checks child exit
        // so a rare bind race fails explicitly rather than using another process.
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
        let relayError: Error | undefined;
        relay.on("error", (error) => {
          relayError = error;
        });
        await expect
          .poll(
            async () => {
              if (relayError) throw relayError;
              if (relayProcess.exitCode !== null) throw new Error("relay exited before readiness");
              return fetch(`${relayUrl}/health`, { signal: AbortSignal.timeout(2000) })
                .then((r) => r.ok)
                .catch(() => false);
            },
            { timeout: 10_000 },
          )
          .toBe(true);
        async function admin(endpoint: string, body: Item, token?: string) {
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
            upstreamModelId: "mock-astra-upstream",
            wireApi: "openai-responses",
          },
          login.token,
        );
        const snippets = buildClientSetupSnippets({
          apiKey: key.secret,
          serviceOrigin: relayUrl,
          codexModel: "gpt-6-astra",
          codexModels: [{ id: "gpt-6-astra", displayName: "Astra" }],
          claudeModel: "",
          opencodeModel: "",
          opencodeModels: [],
          piModels: [],
          dshModel: "",
          dshModels: [],
        });
        // Extract the actual generated heredoc without executing a setup shell.
        const catalog = snippets.codexCatalog.split("<<'JSON'\n")[1]?.split("\nJSON")[0];
        if (!catalog) throw new Error("generated catalog heredoc missing");
        const catalogPath = path.join(configDirectory, "token-toxication-model-catalog.json");
        await writeFile(catalogPath, catalog);
        // Only relocate the generated catalog path to the isolated test home.
        await writeFile(
          path.join(configDirectory, "config.toml"),
          snippets.codexConfig.replace(
            '"~/.codex/token-toxication-model-catalog.json"',
            JSON.stringify(catalogPath),
          ),
        );
        const imagePath = path.join(workspace, "pixel.png");
        await writeFile(
          imagePath,
          Buffer.from(
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=",
            "base64",
          ),
        );
        const childEnv = {
          PATH: process.env.PATH,
          // Isolate the child's user-level skill discovery as well as Codex
          // configuration. Never modify the invoking process's environment.
          HOME: root,
          CODEX_HOME: configDirectory,
          TOKEN_TOXICATION_API_KEY: key.secret,
        };
        if (!effort) await checkModelPicker(codexBinary, childEnv, workspace);
        const versionProcess = spawn(codexBinary, ["--version"], { env: childEnv });
        let version = "";
        versionProcess.stdout.on("data", (data) => {
          version += data;
        });
        await once(versionProcess, "close");
        console.info(
          `Testing ${version.trim()} against a local mock, effort=${effort ?? "default"}`,
        );
        const args = [
          "exec",
          "--skip-git-repo-check",
          "--sandbox",
          "workspace-write",
          "--json",
          "--cd",
          workspace,
          "--image",
          imagePath,
          "-c",
          "check_for_update_on_startup=false",
          "-c",
          'web_search="disabled"',
          // Prevent background marketplace clones unrelated to this contract.
          "-c",
          "features.plugins=false",
          ...(effort ? ["-c", `model_reasoning_effort="${effort}"`] : []),
          ...(contextWindow ? ["-c", `model_context_window=${contextWindow}`] : []),
          "Create compatibility.txt in this disposable workspace and verify the synthetic tool flow.",
        ];
        client = spawn(codexBinary, args, {
          cwd: workspace,
          env: childEnv,
          stdio: ["ignore", "pipe", "pipe"],
        });
        let output = "";
        client.stdout?.on("data", (data) => {
          output += data;
        });
        client.stderr?.on("data", (data) => {
          output += data;
        });
        const timer = setTimeout(() => client?.kill("SIGTERM"), 45_000);
        try {
          const [code] = await once(client, "close");
          expect(code, output).toBe(0);
        } finally {
          clearTimeout(timer);
        }
        expect(captures, output).toHaveLength(4);
        const first = captures[0];
        expect(first.model).toBe("mock-astra-upstream");
        expect(first.instructions).toBe(CODEX_ASTRA_INSTRUCTIONS);
        expect(first.reasoning).toEqual({ effort: effort ?? "low" });
        expect(first.text.verbosity).toBe("low");
        expect(first.parallel_tool_calls).toBe(true);
        for (const unsupported of [
          "temperature",
          "top_p",
          "top_logprobs",
          "prompt_cache_retention",
        ]) {
          expect(first).not.toHaveProperty(unsupported);
        }
        expect(first.tools).toEqual(
          expect.arrayContaining([
            expect.objectContaining({ type: "function", name: "exec_command" }),
            expect.objectContaining({ type: "custom", name: "apply_patch" }),
            expect.objectContaining({ type: "function", name: "view_image" }),
          ]),
        );
        const imageItems = first.input.flatMap((item) =>
          Array.isArray(item.content)
            ? item.content.filter((part) => part.type === "input_image")
            : [],
        );
        expect(imageItems).toHaveLength(1);
        expect(imageItems[0].image_url).toMatch(/^data:image\//);
        expect(first.input.some((item) => item.type === "additional_tools")).toBe(false);
        expect(captures[1].input).toEqual(
          expect.arrayContaining([
            expect.objectContaining({ type: "function_call_output", call_id: "call_mock_exec" }),
          ]),
        );
        const shellOutput = captures[1].input.find(
          (item) => item.call_id === "call_mock_exec" && item.type === "function_call_output",
        );
        expect(JSON.stringify(shellOutput?.output)).toContain(await realpath(workspace));
        expect(captures[2].input).toEqual(
          expect.arrayContaining([
            expect.objectContaining({
              type: "custom_tool_call_output",
              call_id: "call_mock_patch",
            }),
          ]),
        );
        expect(await readFile(path.join(workspace, "compatibility.txt"), "utf8")).toBe(
          "mock tool completed\n",
        );
        const originalImageOutput = captures[3].input.find(
          (item) => item.type === "function_call_output" && item.call_id === "call_mock_image",
        );
        expect(originalImageOutput?.output).toEqual(
          expect.arrayContaining([
            expect.objectContaining({ type: "input_image", detail: "original" }),
          ]),
        );
        expect(output).toContain("mock compatibility complete");
        const sessionsDirectory = path.join(configDirectory, "sessions");
        const rolloutPaths = (await readdir(sessionsDirectory, { recursive: true })).filter(
          (entry) => entry.endsWith(".jsonl"),
        );
        const events = (
          await Promise.all(
            rolloutPaths.map(async (entry) =>
              (await readFile(path.join(sessionsDirectory, entry), "utf8"))
                .trim()
                .split("\n")
                .map((line) => JSON.parse(line)),
            ),
          )
        ).flat();
        expect(events).toEqual(
          expect.arrayContaining([
            expect.objectContaining({
              type: "event_msg",
              payload: expect.objectContaining({
                type: "token_count",
                info: expect.objectContaining({ model_context_window: usableContext }),
              }),
            }),
          ]),
        );
      } finally {
        if (client) await stop(client);
        if (relay) await stop(relay);
        if (upstream.listening) await close(upstream);
        await rm(root, { recursive: true, force: true, maxRetries: 3, retryDelay: 100 });
      }
    },
    60_000,
  );
});
