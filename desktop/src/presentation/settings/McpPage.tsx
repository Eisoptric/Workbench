import { useState } from "react";
import { desktop } from "../../application/desktop";
import { removalQuestion, splitCommandLine, type Agent, type Mcp, type Rag } from "../../domain/model";
import { ConfigureMcpModal } from "./ConfigureMcpModal";
import { askToConfirm } from "../../infrastructure/confirm";
import { Button } from "../ui/base/Button.js";
import { Card } from "../ui/base/Card.js";
import { Input } from "../ui/base/Input.js";
import { Notice } from "../ui/base/Notice.js";
import { Tabs, TabsList, TabsTrigger } from "../ui/base/Tabs.js";
import { Textarea } from "../ui/base/Textarea.js";
import { CardRow, RowActions, SubTitle } from "./Parts";
import { Chevron } from "./SettingsIcons";
import { t } from "../../domain/i18n";

/** What each of the app's own tool servers is for. */
const BUILT_IN_SUMMARIES: Record<string, string> = {
  "general-skills": t("Files, commands, web and plans"),
  "computer-use": t("Mouse, keyboard and screen"),
  origin: t("GitHub repositories and pull requests"),
  customizations: t("Rules, skills and plugins"),
  browser: t("A browser agents can drive"),
  research: t("Research records, runs and looking inside models"),
};

export function McpPage(props: {
  agent: Agent | null;
  mcp: Mcp[];
  rag: Rag[];
  onToggle: (patch: Partial<Agent>) => Promise<void>;
  onChanged: () => Promise<void>;
  onError: (message: string) => void;
}) {
  const [url, setUrl] = useState("");
  const [kind, setKind] = useState<"url" | "command">("url");
  const [commandText, setCommandText] = useState("");
  const [commandName, setCommandName] = useState("");
  const [envText, setEnvText] = useState("");
  const [ragUrl, setRagUrl] = useState("");
  const [confirming, setConfirming] = useState<string | null>(null);
  const [open, setOpen] = useState<Record<string, boolean>>({});
  const [configuringServer, setConfiguringServer] = useState<Mcp | null>(null);

  async function removeServer(id: string) {
    try {
      const users = (await desktop.catalog.listAgents()).filter((agent) => agent.mcpConnectionIds?.includes(id)).map((agent) => agent.name);
      const name = props.mcp.find((item) => item.id === id)?.name ?? "this server";
      if (users.length && !await askToConfirm(removalQuestion(name, users, "server", null))) return;
      await desktop.admin.deleteMcp(id, users.length > 0);
      setConfirming(null);
      props.onError("");
      await props.onChanged();
    } catch (reason) {
      props.onError(reason instanceof Error ? reason.message : t("Could not remove the MCP server"));
    }
  }

  async function addMcp() {
    let created: Mcp | null = null;
    try {
      created = await desktop.admin.createMcp(url);
      setUrl("");
      await desktop.admin.discoverMcp(created.id);
      props.onError("");
    } catch (reason) {
      props.onError(reason instanceof Error ? reason.message : t("Could not connect MCP"));
    } finally {
      // Added even when connecting it needs a sign-in first: its card then offers Connect, instead of a second server.
      if (created) await props.onChanged();
    }
  }

  /** Lists a remote server's tools again, as after signing in to it in the browser. */
  async function connect(item: Mcp) {
    try {
      await desktop.admin.discoverMcp(item.id);
      props.onError("");
    } catch (reason) {
      props.onError(reason instanceof Error ? reason.message : t("Could not connect MCP"));
    } finally {
      await props.onChanged();
    }
  }

  /** Adds a local server. Nothing runs yet: its card then shows the exact command for the user to allow. */
  async function addLocal() {
    try {
      const [program, ...args] = splitCommandLine(commandText);
      if (!program) throw new Error(t("Type the command that starts the server, for example npx -y @modelcontextprotocol/server-filesystem /path/to/folder"));
      const env: Record<string, string> = {};
      for (const line of envText.split("\n").map((item) => item.trim()).filter(Boolean)) {
        const at = line.indexOf("=");
        if (at < 1) throw new Error(t('"{line}" is not NAME=value', { line }));
        env[line.slice(0, at).trim()] = line.slice(at + 1);
      }
      const created = await desktop.admin.createLocalMcp({ name: commandName.trim(), program, args, env });
      setCommandText("");
      setCommandName("");
      setEnvText("");
      setOpen((current) => ({ ...current, [created.id]: true }));
      props.onError("");
      await props.onChanged();
    } catch (reason) {
      props.onError(reason instanceof Error ? reason.message : t("Could not add the local MCP server"));
    }
  }

  async function allow(item: Mcp) {
    try {
      await desktop.admin.approveMcp(item.id, item.commandFingerprint ?? "");
      await desktop.admin.discoverMcp(item.id);
      props.onError("");
      await props.onChanged();
    } catch (reason) {
      props.onError(reason instanceof Error ? reason.message : t("Could not start the MCP server"));
      await props.onChanged();
    }
  }

  async function addRag() {
    try {
      await desktop.admin.createRag(ragUrl);
      setRagUrl("");
      props.onError("");
      await props.onChanged();
    } catch (reason) {
      props.onError(reason instanceof Error ? reason.message : t("Could not connect knowledge"));
    }
  }

  const isBuiltin = (item: Mcp) =>
    item.url.startsWith("builtin:") ||
    ["general-skills", "computer-use", "origin", "customizations", "cursor-computer-use", "cursor-origin", "antigravity-skills"].includes(item.id);

  const builtinServers = props.mcp.filter(isBuiltin);
  const installedServers = props.mcp.filter((item) => !isBuiltin(item));

  function renderMcpCard(item: Mcp) {
    const on = props.agent?.mcpConnectionIds.includes(item.id) ?? false;
    const builtin = isBuiltin(item);
    const removable = !builtin;
    const expanded = open[item.id] ?? false;
    return (
      <Card key={item.id}>
        <CardRow>
          <button
            className="flex min-w-0 flex-1 cursor-pointer items-center gap-2 border-0 bg-transparent p-0 text-left text-foreground"
            type="button"
            aria-expanded={expanded}
            onClick={() => setOpen((current) => ({ ...current, [item.id]: !current[item.id] }))}
          >
            <Chevron open={expanded} />
            <span className="min-w-0">
              <strong className="block truncate">{item.name}</strong>
              <span className="block truncate text-[12.5px] text-muted-foreground">
                {item.command
                  ? item.approval === "approved"
                    ? t("{count} tools · Local program · {command}", { count: item.tools.length, command: item.commandLine ?? item.command.program })
                    : t("Local program · {command}", { command: item.commandLine ?? item.command.program })
                  : t("{count} tools · {source}", { count: item.tools.length, source: builtin ? (BUILT_IN_SUMMARIES[item.id] ?? t("Built in")) : item.url })}
              </span>
            </span>
          </button>
          <RowActions>
            {!builtin && !item.command && item.tools.length === 0 ? (
              <Button variant="default" size="small" type="button" onClick={() => void connect(item)} title={t("List this server's tools, after signing in to it if it asked")}>
                {t("Connect")}
              </Button>
            ) : null}
            <Button
              variant="outline" size="small"
              type="button"
              onClick={() => setConfiguringServer(item)}
              title={t("Configure {name}", { name: item.name })}
            >
              {t("Configure")}
            </Button>
            {removable ? (
              confirming === item.id ? (
                <>
                  <Button variant="outline" size="small" type="button" onClick={() => setConfirming(null)}>{t("Cancel")}</Button>
                  <Button variant="danger" size="small" type="button" onClick={() => void removeServer(item.id)}>{t("Remove")}</Button>
                </>
              ) : (
                <Button variant="outline" size="small" type="button" onClick={() => setConfirming(item.id)}>{t("Remove")}</Button>
              )
            ) : null}
            {!builtin ? (
              <Button
                variant={on ? "default" : "outline"} size="small"
                type="button"
                disabled={!props.agent}
                onClick={() =>
                  void props.onToggle({
                    mcpConnectionIds: on
                      ? props.agent!.mcpConnectionIds.filter((id) => id !== item.id)
                      : [...(props.agent?.mcpConnectionIds ?? []), item.id],
                  })
                }
              >
                {on ? t("Enabled") : t("Disabled")}
              </Button>
            ) : null}
          </RowActions>
        </CardRow>
        {item.command && item.approval !== "approved" ? (
          <Notice tone="wait" className="mt-2.5 grid gap-2 whitespace-normal">
            <p className="m-0 text-[12.5px] leading-snug">
              <strong>{t("This server runs a program on your computer")}</strong>
              {t(", with your user account's permissions. Allow it only if you trust it. It starts with only a minimal environment, not the app's keys.")}
            </p>
            <code className="block rounded-md bg-card px-2 py-1.5 text-xs whitespace-pre-wrap [overflow-wrap:anywhere]">{item.commandLine}</code>
            {item.command.env?.length ? <p className="m-0 text-[12.5px] text-muted-foreground">{t("Environment variables: {names} (values hidden)", { names: item.command.env.join(", ") })}</p> : null}
            <RowActions className="gap-1.5">
              <Button variant="default" size="small" type="button" onClick={() => void allow(item)}>{t("Allow and Start")}</Button>
              <Button variant="outline" size="small" type="button" onClick={() => void removeServer(item.id)}>{t("Remove")}</Button>
            </RowActions>
          </Notice>
        ) : null}
        {expanded ? (
          <div className="flex flex-wrap gap-1.5 pt-1">
            {item.tools.map((tool) => (
              <code className="rounded-md border border-border bg-muted px-2 py-0.5 text-[11.5px]" key={tool.toolId}>{tool.toolId}</code>
            ))}
          </div>
        ) : null}
      </Card>
    );
  }

  return (
    <>
      <SubTitle>{t("Connect an MCP server")}</SubTitle>
      <Card>
        <Tabs value={kind} onValueChange={(next) => setKind(next as "url" | "command")} className="mt-2 mb-1">
          <TabsList aria-label={t("Kind of MCP server")}>
            <TabsTrigger value="url">URL</TabsTrigger>
            <TabsTrigger value="command">{t("Local Program")}</TabsTrigger>
          </TabsList>
        </Tabs>
        {kind === "url" ? (
          <>
            <p className="text-[12.5px] text-muted-foreground">{t("An MCP server reached over HTTP(S). Tools are discovered automatically.")}</p>
            <div className="flex gap-2">
              <Input aria-label={t("MCP server address")} value={url} placeholder="https://mcp.example.com/mcp" onChange={(event) => setUrl(event.target.value)} />
              <Button variant="default" type="button" disabled={!url.trim()} onClick={() => void addMcp()}>
                {t("Connect MCP")}
              </Button>
            </div>
          </>
        ) : (
          <>
            <p className="text-[12.5px] text-muted-foreground">
              {t("A server the app starts on this computer and talks to over stdio. It runs only after you allow its exact command. No shell is used, so ~, $VARIABLES and pipes are not expanded: write full paths.")}
            </p>
            <div className="mt-2 grid gap-2">
              <Input aria-label={t("Server name")} value={commandName} placeholder={t("Name (optional)")} onChange={(event) => setCommandName(event.target.value)} />
              <Input aria-label={t("Command")} value={commandText} placeholder="npx -y @modelcontextprotocol/server-filesystem /Users/you/Documents" onChange={(event) => setCommandText(event.target.value)} />
              <Textarea aria-label={t("Environment variables")} className="resize-y font-mono text-xs field-sizing-fixed" value={envText} rows={2} placeholder={t("Environment variables, one per line (optional)\nGITHUB_TOKEN=…")} onChange={(event) => setEnvText(event.target.value)} />
              <Button variant="default" className="justify-self-start" type="button" disabled={!commandText.trim()} onClick={() => void addLocal()}>
                {t("Add Local Server")}
              </Button>
            </div>
          </>
        )}
      </Card>

      <SubTitle>{t("Built in")}</SubTitle>
      {builtinServers.length === 0 ? (
        <p className="text-[12.5px] text-muted-foreground">{t("No built-in skills available.")}</p>
      ) : (
        builtinServers.map(renderMcpCard)
      )}

      <SubTitle className="mt-6">{t("Installed MCP Servers")}</SubTitle>
      {installedServers.length === 0 ? (
        <p className="text-[12.5px] text-muted-foreground">{t("No external MCP servers installed yet.")}</p>
      ) : (
        installedServers.map(renderMcpCard)
      )}

      {configuringServer && (
        <ConfigureMcpModal
          server={configuringServer}
          onClose={() => setConfiguringServer(null)}
          onSave={async (disabledTools) => {
            try {
              await desktop.admin.updateMcp(configuringServer.id, { disabledTools });
              props.onError("");
              await props.onChanged();
            } catch (reason) {
              props.onError(reason instanceof Error ? reason.message : t("Could not save tool configuration"));
            }
          }}
          onReload={async () => {
            try {
              await desktop.admin.discoverMcp(configuringServer.id);
              props.onError("");
              await props.onChanged();
            } catch (reason) {
              props.onError(reason instanceof Error ? reason.message : t("Could not reload tools"));
            }
          }}
        />
      )}

      <SubTitle className="mt-6">{t("Knowledge & RAG Endpoints")}</SubTitle>
      <Card>
        <div className="text-xs font-medium text-muted-foreground">{t("Connect Knowledge URL")}</div>
        <div className="flex gap-2">
          <Input aria-label={t("Knowledge URL")} value={ragUrl} placeholder="http://127.0.0.1:4200/search" onChange={(event) => setRagUrl(event.target.value)} />
          <Button variant="default" type="button" disabled={!ragUrl.trim()} onClick={() => void addRag()}>
            {t("Connect Knowledge")}
          </Button>
        </div>
      </Card>
      {props.rag.map((item) => {
        const on = props.agent?.ragConnectionIds.includes(item.id) ?? false;
        return (
          <Card key={item.id}>
            <CardRow>
              <div>
                <strong>{item.name}</strong>
                <div className="text-[12.5px] text-muted-foreground">{item.url}</div>
              </div>
              <Button
                variant={on ? "default" : "outline"} size="small"
                type="button"
                disabled={!props.agent}
                onClick={() =>
                  void props.onToggle({
                    ragConnectionIds: on
                      ? props.agent!.ragConnectionIds.filter((id) => id !== item.id)
                      : [...(props.agent?.ragConnectionIds ?? []), item.id],
                  })
                }
              >
                {on ? t("Enabled") : t("Disabled")}
              </Button>
            </CardRow>
          </Card>
        );
      })}
    </>
  );
}

