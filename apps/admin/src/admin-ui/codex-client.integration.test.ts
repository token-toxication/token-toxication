/// <reference types="node" />

import { execFileSync, spawn, type ChildProcess } from "node:child_process";
import { once } from "node:events";
import { mkdtemp, mkdir, readFile, readdir, realpath, rm, writeFile } from "node:fs/promises";
import { createServer, type Server } from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import { createInterface } from "node:readline";
import { WebSocketServer } from "ws";

import { describe, expect, it } from "vite-plus/test";

import { buildClientSetupSnippets } from "./client-setup";
import { CODEX_ASTRA_INSTRUCTIONS } from "./codex-astra-instructions";
import { CODEX_GPT56_INSTRUCTIONS } from "./codex-gpt56-instructions";

// Explicit opt-in: no real OpenAI endpoint, inherited credentials, or daily
// Codex configuration. The mock asks for pwd and a patch in a disposable cwd.
const codexBinary = process.env.TT_CODEX_BIN;
const relayBinary = process.env.TT_RELAY_BIN;
if (codexBinary && !path.isAbsolute(codexBinary)) {
  throw new Error("TT_CODEX_BIN must be an absolute path");
}
const clientVersion = codexBinary
  ? execFileSync(codexBinary, ["--version"], { encoding: "utf8", timeout: 10_000 }).trim()
  : "";
const versionParts =
  clientVersion
    .match(/(\d+)\.(\d+)\.(\d+)/)
    ?.slice(1)
    .map(Number) ?? [];
const supportsModernAgents =
  versionParts[0] > 0 || versionParts[1] > 153 || (versionParts[1] === 153 && versionParts[2] >= 4);

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
type RpcResult = { thread: { id: string }; turn: { id: string; status?: string; error?: unknown } };
type RpcMessage = {
  id?: number;
  result?: RpcResult;
  error?: unknown;
  method?: string;
  params?: { item?: { type?: string }; turn?: RpcResult["turn"] };
};

async function checkModelPicker(
  binary: string,
  env: NodeJS.ProcessEnv,
  cwd: string,
  model: string,
  defaultEffort: string,
) {
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
        model,
        defaultReasoningEffort: defaultEffort,
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
  previous_response_id?: string;
  prompt_cache_key?: string;
  client_metadata?: Record<string, string>;
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

describe.skipIf(!codexBinary).each([
  { model: "gpt-6-astra", defaultEffort: "low", instructions: CODEX_ASTRA_INSTRUCTIONS },
  { model: "gpt-5.6-sol", defaultEffort: "low", instructions: CODEX_GPT56_INSTRUCTIONS },
  { model: "gpt-5.6-terra", defaultEffort: "medium", instructions: CODEX_GPT56_INSTRUCTIONS },
  { model: "gpt-5.6-luna", defaultEffort: "medium", instructions: CODEX_GPT56_INSTRUCTIONS },
])("$model client through the Responses relay", ({ model, defaultEffort, instructions }) => {
  it.for([
    { name: "default effort", effort: undefined, contextWindow: undefined, usableContext: 258_400 },
    ...["low", "medium", "high", "xhigh", "max"].map((effort) => ({
      name: `explicit ${effort} effort`,
      effort,
      contextWindow: undefined,
      usableContext: 258_400,
    })),
    {
      name: "web search and summary",
      effort: undefined,
      contextWindow: undefined,
      usableContext: 258_400,
    },
    {
      name: "explicit Responses Lite override",
      effort: undefined,
      contextWindow: undefined,
      usableContext: 258_400,
    },
    {
      name: "explicit code mode override",
      effort: "high",
      contextWindow: undefined,
      usableContext: 258_400,
    },
    ...["multi-agent lifecycle", "ordinary multi-agent lifecycle", "disabled delegation"].map(
      (name) => ({
        name,
        effort: model === "gpt-5.6-luna" ? "high" : "ultra",
        contextWindow: undefined,
        usableContext: 258_400,
      }),
    ),
    {
      name: "explicit WebSocket override",
      effort: "high",
      contextWindow: undefined,
      usableContext: 258_400,
    },
    ...[
      "ordinary code mode",
      "WebSocket Lite override",
      "code mode tool failure",
      "ordinary code mode tool failure",
    ].map((name) => ({
      name,
      effort: "high",
      contextWindow: undefined,
      usableContext: 258_400,
    })),
    {
      name: "WebSocket steering during tool execution",
      effort: "high",
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
    { timeout: 60_000 },
    async ({ name, effort, contextWindow, usableContext }, context) => {
      const withWebSearch = name === "web search and summary";
      const withAgents =
        name === "multi-agent lifecycle" || name === "ordinary multi-agent lifecycle";
      const agentsDisabled = name === "disabled delegation";
      const withToolFailure = name.endsWith("code mode tool failure");
      if ((withAgents || agentsDisabled) && !supportsModernAgents) context.skip();
      const withCodeMode =
        name === "explicit code mode override" ||
        name === "ordinary code mode" ||
        agentsDisabled ||
        withToolFailure;
      const withSteering = name === "WebSocket steering during tool execution";
      const withWebSocket =
        name === "explicit WebSocket override" ||
        name === "WebSocket Lite override" ||
        withSteering;
      const withLite =
        !name.startsWith("ordinary ") &&
        (name === "explicit Responses Lite override" ||
          name === "WebSocket Lite override" ||
          withCodeMode ||
          withAgents);
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
      const childCaptures: RequestBody[] = [];
      let rootThreadId: string | undefined;
      const liteHeaders: (string | string[] | undefined)[] = [];
      function mockResponse(body: RequestBody, lite: string | string[] | undefined) {
        rootThreadId ??= body.client_metadata?.thread_id;
        if (withAgents && body.client_metadata?.thread_id !== rootThreadId) {
          childCaptures.push(body);
          return responseStream(100, {
            type: "message",
            id: "child_done",
            role: "assistant",
            status: "completed",
            content: [{ type: "output_text", text: "synthetic-child-done", annotations: [] }],
          });
        }
        captures.push(body);
        liteHeaders.push(lite);
        if (withAgents && captures.length === 1) {
          return responseStream(80, {
            type: "function_call",
            id: "fc_spawn",
            call_id: "call_spawn",
            name: "spawn_agent",
            namespace: model === "gpt-5.6-luna" ? "multi_agent_v1" : "collaboration",
            arguments: JSON.stringify(
              model === "gpt-5.6-luna"
                ? { message: "synthetic-child-task" }
                : {
                    task_name: "bounded_check",
                    message: "synthetic-child-task",
                    fork_turns: "none",
                  },
            ),
          });
        }
        if (withAgents && captures.length === 2) {
          const spawnOutput = body.input.find(
            (item) => item.call_id === "call_spawn" && item.type === "function_call_output",
          );
          const rawSpawnOutput = spawnOutput?.output;
          const spawnText = Array.isArray(rawSpawnOutput)
            ? rawSpawnOutput.map((part) => part.text ?? "").join("")
            : String(rawSpawnOutput);
          const spawned = model === "gpt-5.6-luna" ? JSON.parse(spawnText) : {};
          return responseStream(81, {
            type: "function_call",
            id: "fc_wait",
            call_id: "call_wait",
            name: "wait_agent",
            namespace: model === "gpt-5.6-luna" ? "multi_agent_v1" : "collaboration",
            arguments: JSON.stringify(
              model === "gpt-5.6-luna"
                ? { ids: [spawned.agent_id], timeout_ms: 10000 }
                : { timeout_ms: 10000 },
            ),
          });
        }
        const index = captures.length - (withAgents ? 2 : 0);
        let item: Item;
        if (index === 1) {
          item = {
            type: "function_call",
            id: "fc_mock",
            call_id: "call_mock_exec",
            name: "exec_command",
            arguments: JSON.stringify({
              cmd: withToolFailure ? "pwd; exit 7" : withSteering ? "sleep 1; pwd" : "pwd",
              login: false,
              max_output_tokens: 100,
              ...(withSteering ? { yield_time_ms: 10000 } : {}),
            }),
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
          throw new Error("unexpected extra request");
        }
        if (withCodeMode && index <= 3) {
          const input =
            index === 1
              ? `text(await tools.exec_command(${String(item.arguments)}));`
              : index === 2
                ? `text(await tools.apply_patch(${JSON.stringify(item.input)}));`
                : `image(await tools.view_image(${String(item.arguments)}));`;
          item = {
            type: "custom_tool_call",
            id: item.id,
            call_id: item.call_id,
            name: "exec",
            input,
          };
        }
        return responseStream(index, item);
      }
      const upstream = createServer(async (request, response) => {
        try {
          if (request.method !== "POST" || request.url !== "/v1/responses" || withWebSocket) {
            response.writeHead(404).end();
            return;
          }
          const chunks: Buffer[] = [];
          for await (const chunk of request) chunks.push(Buffer.from(chunk));
          const body: RequestBody = JSON.parse(Buffer.concat(chunks).toString());
          const stream = mockResponse(
            body,
            request.headers["x-openai-internal-codex-responses-lite"],
          );
          response.writeHead(200, { "content-type": "text/event-stream" }).end(stream);
        } catch (error) {
          console.info("synthetic mock error", String(error));
          response.writeHead(500).end("invalid mock request");
        }
      });
      const websocketServer = new WebSocketServer({ server: upstream, path: "/v1/responses" });
      let websocketConnections = 0;
      const websocketRequests: RequestBody[] = [];
      websocketServer.on("connection", (socket) => {
        websocketConnections++;
        const history = new Map<string, Item[]>();
        socket.on("message", (data) => {
          try {
            const body = JSON.parse(
              Buffer.concat(
                Array.isArray(data) ? data : [Buffer.isBuffer(data) ? data : Buffer.from(data)],
              ).toString(),
            );
            websocketRequests.push(structuredClone(body));
            if (body.previous_response_id) {
              const previousInput = history.get(body.previous_response_id);
              if (!previousInput) throw new Error("unknown mock response continuation");
              body.input = [...previousInput, ...body.input];
            }
            if (body.generate === false) {
              history.set("resp_warmup", body.input);
              socket.send(
                JSON.stringify({
                  type: "response.completed",
                  response: { id: "resp_warmup", status: "completed", output: [] },
                }),
              );
              return;
            }
            const events = mockResponse(
              body,
              body.client_metadata?.ws_request_header_x_openai_internal_codex_responses_lite,
            );
            for (const line of events.split("\n")) {
              if (line.startsWith("data: ")) {
                const event = JSON.parse(line.slice(6));
                if (event.type === "response.completed")
                  history.set(event.response.id, [...body.input, ...event.response.output]);
                socket.send(line.slice(6));
              }
            }
          } catch {
            socket.close(1011, "invalid mock request");
          }
        });
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
        await admin("model-catalog", { id: model, family: "other" }, login.token);
        await admin(
          "provider-model-routes",
          {
            publicModelId: model,
            providerAccountId: account.data.id,
            upstreamModelId: "mock-coding-upstream",
            wireApi: "openai-responses",
          },
          login.token,
        );
        const snippets = buildClientSetupSnippets({
          apiKey: key.secret,
          serviceOrigin: relayUrl,
          codexModel: model,
          codexModels: [{ id: model, displayName: model }],
          codexAdvanced: {
            [model]: {
              responsesLite: withLite,
              codeMode: withCodeMode,
              multiAgent: withAgents || agentsDisabled,
            },
          },
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
        // Lite is an explicit per-model user override, never inferred from the
        // public model name or enabled in the default generated setup.
        await writeFile(catalogPath, catalog);
        // Only relocate the generated catalog path to the isolated test home.
        await writeFile(
          path.join(configDirectory, "config.toml"),
          snippets.codexConfig.replace(
            '"~/.codex/token-toxication-model-catalog.json"',
            JSON.stringify(catalogPath),
          ) + (withWebSocket ? "\nsupports_websockets = true\n" : ""),
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
        if (!effort) await checkModelPicker(codexBinary, childEnv, workspace, model, defaultEffort);
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
          `web_search="${withWebSearch ? "live" : "disabled"}"`,
          ...(withWebSearch ? ["-c", 'model_reasoning_summary="detailed"'] : []),
          // Prevent background marketplace clones unrelated to this contract.
          "-c",
          "features.plugins=false",
          ...(agentsDisabled
            ? ["-c", "agents.enabled=false", "-c", "features.multi_agent_v2=false"]
            : []),
          ...(withWebSocket ? ["-c", "features.responses_websockets_v2=true"] : []),
          ...(effort ? ["-c", `model_reasoning_effort="${effort}"`] : []),
          ...(contextWindow ? ["-c", `model_context_window=${contextWindow}`] : []),
          "Create compatibility.txt in this disposable workspace and verify the synthetic tool flow.",
        ];
        client = spawn(
          codexBinary,
          withSteering
            ? [
                "app-server",
                "-c",
                "features.plugins=false",
                "-c",
                "features.responses_websockets_v2=true",
                "-c",
                'web_search="disabled"',
                "-c",
                'model_reasoning_effort="high"',
              ]
            : args,
          {
            cwd: workspace,
            env: childEnv,
            stdio: [withSteering ? "pipe" : "ignore", "pipe", "pipe"],
          },
        );
        let output = "";
        client.stdout?.on("data", (data) => {
          output += data;
        });
        client.stderr?.on("data", (data) => {
          output += data;
        });
        const timer = setTimeout(() => client?.kill("SIGTERM"), 45_000);
        try {
          if (withSteering) {
            const lines = createInterface({ input: client.stdout! });
            const replies = new Map<number, RpcMessage>();
            const notifications: RpcMessage[] = [];
            lines.on("line", (line) => {
              const message: RpcMessage = JSON.parse(line);
              if (typeof message.id === "number") replies.set(message.id, message);
              else notifications.push(message);
            });
            let requestId = 0;
            const rpc = async (method: string, params: Item) => {
              const id = ++requestId;
              client!.stdin!.write(JSON.stringify({ id, method, params }) + "\n");
              await expect.poll(() => replies.has(id), { timeout: 10000 }).toBe(true);
              const reply = replies.get(id)!;
              expect(reply.error, output).toBeUndefined();
              expect(reply.result).toBeDefined();
              return reply.result!;
            };
            try {
              await rpc("initialize", {
                clientInfo: { name: "relay-steering-test", version: "1.0" },
              });
              client.stdin!.write(JSON.stringify({ method: "initialized" }) + "\n");
              const { thread } = await rpc("thread/start", {
                cwd: workspace,
                model,
                approvalPolicy: "never",
                sandbox: "workspace-write",
              });
              const { turn } = await rpc("turn/start", {
                threadId: thread.id,
                input: [
                  { type: "text", text: "Verify the synthetic tool flow.", text_elements: [] },
                  { type: "localImage", path: imagePath },
                ],
              });
              await expect
                .poll(
                  () =>
                    notifications.some(
                      (message) =>
                        message.method === "item/started" &&
                        message.params?.item?.type === "commandExecution",
                    ),
                  { timeout: 10000 },
                )
                .toBe(true);
              await rpc("turn/steer", {
                threadId: thread.id,
                expectedTurnId: turn.id,
                input: [
                  { type: "text", text: "synthetic-steering-during-tool", text_elements: [] },
                ],
              });
              await expect
                .poll(() => notifications.some((message) => message.method === "turn/completed"), {
                  timeout: 15000,
                })
                .toBe(true);
              const completed = notifications.find(
                (message) => message.method === "turn/completed",
              );
              expect(completed?.params?.turn?.status, output).toBe("completed");
              expect(completed?.params?.turn?.error, output).toBeFalsy();
            } finally {
              lines.close();
            }
          } else {
            const [code] = await once(client, "close");
            expect(code, output).toBe(0);
          }
        } finally {
          clearTimeout(timer);
        }
        expect(captures, output).toHaveLength(withAgents ? 6 : 4);
        expect(childCaptures.length).toBe(withAgents ? 1 : 0);
        if (withAgents) {
          for (const child of childCaptures) {
            expect(child.model).toBe("mock-coding-upstream");
            expect(JSON.stringify(child.input)).toContain("synthetic-child-task");
            if (withLite) {
              expect(child.input[1]).toMatchObject({
                role: "developer",
                content: [{ type: "input_text", text: instructions }],
              });
            } else {
              expect(child.instructions).toBe(instructions);
            }
          }
          captures.splice(0, 2);
          liteHeaders.splice(0, 2);
        }
        expect(websocketConnections).toBe(withWebSocket ? 1 : 0);
        if (withWebSocket)
          expect(websocketRequests.some((request) => request.previous_response_id)).toBe(true);
        const first = captures[0];
        if (withSteering)
          expect(JSON.stringify(captures[1].input)).toContain("synthetic-steering-during-tool");
        expect(first.model).toBe("mock-coding-upstream");
        for (const request of captures) {
          expect(request.instructions).toBe(withLite ? undefined : instructions);
          if (withLite) {
            expect(request).not.toHaveProperty("tools");
            expect(request.input[0]).toMatchObject({ type: "additional_tools", role: "developer" });
            expect(request.input[1]).toMatchObject({
              type: "message",
              role: "developer",
              content: [{ type: "input_text", text: instructions }],
            });
          }
        }
        expect(liteHeaders).toEqual(Array(4).fill(withLite ? "true" : undefined));
        expect(first.reasoning).toEqual({
          effort:
            effort === "ultra"
              ? model === "gpt-6-astra"
                ? "xhigh"
                : "max"
              : (effort ?? defaultEffort),
          ...(withWebSearch ? { summary: "detailed" } : {}),
          ...(withLite ? { context: "all_turns" } : {}),
        });
        const declaredTools = (withLite ? first.input[0].tools : first.tools) as Item[];
        const tools = declaredTools.flatMap((tool) =>
          tool.type === "namespace" ? (tool.tools as Item[]) : [tool],
        );
        if (agentsDisabled) expect(JSON.stringify(tools)).not.toContain("spawn_agent");
        const webTools = tools.filter((tool) => String(tool.type).startsWith("web_search"));
        if (withWebSearch) {
          expect(webTools).toEqual([
            expect.objectContaining({
              search_content_types: ["text", "image"],
              external_web_access: true,
            }),
          ]);
        } else {
          expect(webTools).toEqual([]);
        }
        expect(first).not.toHaveProperty("service_tier");
        expect(first.text.verbosity).toBe("low");
        expect(first.parallel_tool_calls).toBe(!withLite);
        for (const unsupported of [
          "temperature",
          "top_p",
          "top_logprobs",
          "prompt_cache_retention",
        ]) {
          expect(first).not.toHaveProperty(unsupported);
        }
        expect(tools).toEqual(
          expect.arrayContaining([
            ...(withCodeMode
              ? [expect.objectContaining({ type: "custom", name: "exec" })]
              : [
                  expect.objectContaining({ type: "function", name: "exec_command" }),
                  expect.objectContaining({ type: "custom", name: "apply_patch" }),
                  expect.objectContaining({ type: "function", name: "view_image" }),
                ]),
          ]),
        );
        const imageItems = first.input.flatMap((item) =>
          Array.isArray(item.content)
            ? item.content.filter((part) => part.type === "input_image")
            : [],
        );
        expect(imageItems).toHaveLength(1);
        expect(imageItems[0].image_url).toMatch(/^data:image\//);
        expect(first.input.some((item) => item.type === "additional_tools")).toBe(withLite);
        expect(captures[1].input).toEqual(
          expect.arrayContaining([
            expect.objectContaining({
              type: withCodeMode ? "custom_tool_call_output" : "function_call_output",
              call_id: "call_mock_exec",
            }),
          ]),
        );
        const shellOutput = captures[1].input.find(
          (item) =>
            item.call_id === "call_mock_exec" &&
            item.type === (withCodeMode ? "custom_tool_call_output" : "function_call_output"),
        );
        expect(JSON.stringify(shellOutput?.output)).toContain(await realpath(workspace));
        if (withToolFailure) {
          const output = shellOutput?.output;
          const text = Array.isArray(output)
            ? output.map((part) => part.text ?? "").join("\n")
            : String(output);
          expect(text).toMatch(/"exit_code"\s*:\s*7|Process exited with code 7/);
        }
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
          (item) =>
            item.type === (withCodeMode ? "custom_tool_call_output" : "function_call_output") &&
            item.call_id === "call_mock_image",
        );
        expect(originalImageOutput?.output).toEqual(
          expect.arrayContaining([
            expect.objectContaining({
              type: "input_image",
              ...(withLite ? {} : { detail: "original" }),
            }),
          ]),
        );
        if (withLite) {
          for (const item of (originalImageOutput?.output ?? []) as Item[])
            expect(item).not.toHaveProperty("detail");
        }
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
        for (const socket of websocketServer.clients) socket.terminate();
        websocketServer.close();
        if (upstream.listening) await close(upstream);
        await rm(root, { recursive: true, force: true, maxRetries: 3, retryDelay: 100 });
      }
    },
  );
});
