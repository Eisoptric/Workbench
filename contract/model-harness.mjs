// What the contract/model-*.test.mjs files share: API calls with the token, waiting with a deadline, and fake model
// servers on this computer. Nothing here reaches a real provider, the developer's own Workbench, or GitHub.
import { api, fakeModel, fakeServer, request, settled, until } from "./client.mjs";

export { api, fakeModel, fakeServer, request, settled, until };

/** One tool call in a model's turn. */
export const call = (name, args, id = `call_${name}`) => ({ id, type: "function", function: { name, arguments: JSON.stringify(args) } });

/** A run, once it has ended. */
export const finished = (target, id) => until(`run ${id} ends`, () => api(target, "GET", `/runs/${id}`), settled);

/** Whether the model already acted since the user's last message. */
export function actedSinceAsking(messages = []) {
  const asked = messages.findLastIndex((item) => item.role === "user");
  return messages.slice(asked + 1).some((item) => item.role === "tool" || item.tool_calls?.length);
}

/** The system prompt a request carried. */
export const systemOf = (body) => body.messages?.find((item) => item.role === "system")?.content ?? "";

/** The names of the tools a request offered. */
export const offeredBy = (body) => (body.tools ?? []).map((tool) => tool.function?.name);

/** A connection to `model`, an agent on it, and a chat for that agent, optionally in a project folder. */
export async function agentOn(target, model, { autonomy = "bypass", folder, mcpConnectionIds, connection = {}, title = "Contract" } = {}) {
  const made = await api(target, "POST", "/connections/models", { name: "Scripted", provider: "openai", model: "scripted", baseUrl: model.url, apiKey: "scripted-test-value", ...connection });
  const agent = await api(target, "POST", "/agents", { name: "Builder", modelConnectionId: made.id, autonomy, ...(mcpConnectionIds ? { mcpConnectionIds } : {}) });
  const project = folder ? await api(target, "POST", "/projects", { path: folder, grant: true }) : undefined;
  const thread = await api(target, "POST", `/agents/${agent.id}/threads`, { title, ...(project ? { projectId: project.id } : {}) });
  return { connection: made, agent, project, thread };
}

/** One message in a chat, up to the end of its run. */
export async function turn(target, setup, input = "go") {
  const started = await api(target, "POST", `/agents/${setup.agent.id}/runs`, { input, threadId: setup.thread.id });
  return finished(target, started.id);
}
