// Shared HTTP client, polling, and fake OpenAI-compatible model server primitives for contract tests.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { createServer } from "node:http";
import path from "node:path";

/** Reads the runtime's per-session bearer token from its data folder. */
export async function tokenOf(dataDir) {
  return (await readFile(path.join(dataDir, "runtime-token"), "utf8")).trim();
}

/** An authenticated API call returning status, parsed body, and headers. */
export async function request(target, method, route, body, headers = {}) {
  const token = await tokenOf(target.dataDir);
  const response = await fetch(`${target.origin}${route}`, {
    method,
    headers: {
      authorization: `Bearer ${token}`,
      ...(body === undefined ? {} : { "content-type": "application/json" }),
      ...headers,
    },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });
  const text = await response.text();
  let parsed;
  try {
    parsed = text ? JSON.parse(text) : undefined;
  } catch {
    parsed = text;
  }
  return { status: response.status, body: parsed, headers: response.headers };
}

/** An authenticated API call that must succeed (< 300). */
export async function api(target, method, route, body) {
  const answer = await request(target, method, route, body);
  assert.ok(answer.status < 300, `${method} ${route}: ${answer.status} ${JSON.stringify(answer.body)}`);
  return answer.body;
}

/** Polls `read()` until `done(value)` holds, or fails after `ms` milliseconds. */
export async function until(what, read, done = Boolean, { ms = 30_000, interval = 20 } = {}) {
  const deadline = Date.now() + ms;
  let value;
  for (;;) {
    value = await read();
    if (done(value)) return value;
    if (Date.now() > deadline) throw new Error(`never happened: ${what}; last ${JSON.stringify(value)?.slice(0, 2000)}`);
    await new Promise((resolve) => setTimeout(resolve, interval));
  }
}

/** Whether a run has ended, whichever way. */
export const settled = (run) => !["queued", "running", "waiting_approval"].includes(run.status);

/** Starts a local HTTP server that collects JSON request bodies, closed by `stop()`. */
export async function fakeServer(answer) {
  const requests = [];
  const server = createServer((incoming, response) => {
    let raw = "";
    incoming.on("data", (chunk) => { raw += chunk; });
    incoming.on("end", async () => {
      let body = {};
      try {
        body = JSON.parse(raw || "{}");
      } catch {
        body = {};
      }
      requests.push({ ...body, __url: incoming.url });
      await answer(body, response, incoming);
    });
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  return {
    requests,
    url: `http://127.0.0.1:${server.address().port}`,
    stop: () => new Promise((resolve) => { server.closeAllConnections(); server.close(resolve); }),
  };
}

/**
 * A fake OpenAI-compatible chat completions server. `reply(body)` returns text, a tool_calls array,
 * or `{ status, body }` for an error; returning undefined answers `"done"`.
 */
export function fakeModel(reply = () => undefined) {
  return fakeServer(async (body, response) => {
    const answer = (await reply(body)) ?? "done";
    if (answer.status) {
      response.writeHead(answer.status, { "content-type": "application/json" }).end(JSON.stringify(answer.body ?? { error: { message: "scripted failure" } }));
      return;
    }
    const toolCalls = Array.isArray(answer);
    const message = toolCalls ? { role: "assistant", content: "", tool_calls: answer } : { role: "assistant", content: answer };
    response.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify({
      id: "reply",
      object: "chat.completion",
      model: "scripted",
      choices: [{ index: 0, finish_reason: toolCalls ? "tool_calls" : "stop", message }],
      usage: { prompt_tokens: 40, completion_tokens: 6, total_tokens: 46 },
    }));
  });
}
