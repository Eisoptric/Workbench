import { useEffect, useState } from "react";
import { RuntimeClient } from "../../infrastructure/runtime-client.js";
import { getRestartRuntime, readRuntimeStatus, type RuntimeStatus } from "../../infrastructure/runtime-status.js";

export type { RuntimeStatus };

/** The runtime's state, asked for often while it is not ready and now and then once it is. */
export function useRuntimeStatus(): { status: RuntimeStatus; restart: (() => Promise<void>) | null } {
  const [status, setStatus] = useState<RuntimeStatus>({ state: "starting", attempt: 0, log: [] });
  useEffect(() => {
    const client = new RuntimeClient();
    let live = true;
    let timer = 0;
    const ask = async () => {
      const next = await readRuntimeStatus(client);
      if (!live) return;
      setStatus((current) => (JSON.stringify(current) === JSON.stringify(next) ? current : next));
      timer = window.setTimeout(() => void ask(), next.state === "ready" ? 3000 : 1000);
    };
    void ask();
    return () => {
      live = false;
      window.clearTimeout(timer);
    };
  }, []);
  return { status, restart: getRestartRuntime() };
}
