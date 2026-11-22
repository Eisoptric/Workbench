import { RuntimeClient } from "./runtime-client.js";

/** Where the runtime stands, as the desktop app reports it; a plain browser only knows whether it answers. */
export type RuntimeStatus =
  | { state: "starting"; attempt: number; log: string[] }
  | { state: "ready"; version: string; own: boolean }
  | { state: "blocked"; reason: string }
  | { state: "failed"; reason: string; log: string[] };

/** Asks the app, or in a plain browser the runtime itself. */
export async function readRuntimeStatus(client: RuntimeClient): Promise<RuntimeStatus> {
  const fromApp = window.workbenchDesktop?.runtimeStatus;
  if (fromApp) return fromApp();
  try {
    const answer = await client.request<{ version?: string }>("/health");
    return { state: "ready", version: answer.version ?? "unknown", own: false };
  } catch {
    return { state: "starting", attempt: 0, log: [] };
  }
}

/** Returns a runtime restart trigger when running inside the desktop shell. */
export function getRestartRuntime(): (() => Promise<void>) | null {
  const restart = window.workbenchDesktop?.restartRuntime ?? null;
  return restart ? () => restart() : null;
}
