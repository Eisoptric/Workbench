import { useEffect, useRef, useState, type ReactNode } from "react";
import { desktop } from "../../application/desktop";
import { OlderFeatures } from "./OlderFeatures";
import { PermissionsPage } from "./Permissions";
import { HooksPage } from "./Hooks";
import { PluginsPage } from "./Plugins";
import type { Agent, LogLine, Mcp, Model, Profile, Project, Rag, RuntimeInfo } from "../../domain/model";
import { UsageBilling } from "./UsageBilling";
import { Telemetry } from "./Telemetry";
import { About } from "./About";
import { Button } from "../ui/base/Button.js";
import { classes } from "../ui/base/classes.js";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "../ui/base/Dialog.js";
import { Choice } from "../ui/base/Field.js";
import { Notice } from "../ui/base/Notice.js";
import { Textarea } from "../ui/base/Textarea.js";
import { RowActions, RowDescription, RowError, RowLabel, RowText, SectionTitle, SettingsCard, SettingsRow } from "./Parts";
import { shortcut, t } from "../../domain/i18n";
import { CloseIcon } from "./SettingsIcons";
import { General } from "./GeneralPage";
import { LlmPage } from "./LlmPage";
import { McpPage } from "./McpPage";
import { ProjectSettingsTab } from "./ProjectSettingsTab";

export { LlmPage } from "./LlmPage";
export { ClaudeCodeLogin, CodexLogin, ProgramGate } from "./ProgramAccess";

export type SettingsPage =
  | "settings"
  | "usage"
  | "llm"
  | "mcp"
  | "telemetry"
  | "logs"
  | "information"
  | "memory"
  | "permissions"
  | "hooks"
  | "plugins"
  | "shortcuts"
  | "older";

const GROUPS: { title: string; items: [SettingsPage, string, string][] }[] = [
  {
    title: t("Settings"),
    items: [
      ["settings", t("General"), t("Execution, appearance, and workspace")],
      ["llm", t("Models"), t("LLM providers & models")],
      ["mcp", t("MCP & Skills"), t("Tool servers & knowledge sources")],
      ["usage", t("Usage & Billing"), t("Token spend & costs")],
      ["memory", t("Memory"), t("What the app remembers across chats")],
      ["permissions", t("Permissions"), t("What agents may do without asking")],
      ["hooks", t("Hooks"), t("Commands around tool calls and turns")],
      ["plugins", t("Plugins"), t("Bundles of skills, hooks and MCP servers")],
    ],
  },
  {
    title: t("System"),
    items: [
      ["telemetry", t("Telemetry"), t("OpenTelemetry traces & logs")],
      ["logs", t("Logs"), t("Live runtime event stream")],
      ["information", t("About"), t("What the app is and how it fits together")],
    ],
  },
];

export function Settings(props: {
  initialPage?: SettingsPage;
  /** Opens on one project's page (the chat's rules chip). */
  initialProjectId?: string | null;
  agents: { id: string; name: string; status: string }[];
  agent: Agent | null;
  models: Model[];
  mcp: Mcp[];
  rag: Rag[];
  projects?: Project[];
  onClose: () => void;
  onChanged: () => Promise<void>;
  onPatchAgent: (patch: Partial<Agent>) => Promise<void>;
  onProfile: (profile: Profile) => void;
  onProjectRemoved?: () => void;
  theme?: "light" | "dark";
  onSetTheme?: (theme: "light" | "dark") => void;
  onToggleTheme?: () => void;
}) {
  const [page, setPage] = useState<SettingsPage>(props.initialPage ?? "settings");
  const paneRef = useRef<HTMLDivElement | null>(null);
  const [selectedProjectId, setSelectedProjectId] = useState<string | null>(null);
  const [profile, setProfile] = useState<Profile | null>(null);
  const [info, setInfo] = useState<RuntimeInfo | null>(null);
  const [logs, setLogs] = useState<LogLine[]>([]);
  const [error, setError] = useState("");

  useEffect(() => {
    if (props.initialPage) {
      setPage(props.initialPage);
      setSelectedProjectId(props.initialProjectId ?? null);
    }
  }, [props.initialPage, props.initialProjectId]);

  // Each page starts at its top.
  useEffect(() => {
    paneRef.current?.scrollTo({ top: 0 });
  }, [page, selectedProjectId]);

  async function load() {
    const next = await desktop.admin.console();
    setProfile(next.profile);
    props.onProfile(next.profile);
    setInfo(next.information);
    setLogs(next.logs);
  }

  useEffect(() => {
    void load().catch((reason: unknown) => setError(reason instanceof Error ? reason.message : t("Could not load settings")));
  }, [props.models, props.agents]);

  const selectedProject = selectedProjectId ? props.projects?.find((p) => p.id === selectedProjectId) : null;
  const currentMeta = selectedProject
    ? ["project", selectedProject.name, t("Project configuration & custom rules ({path})", { path: selectedProject.path })]
    : page === "shortcuts"
      ? ["shortcuts", t("Shortcuts"), t("Quick keyboard shortcuts for the app")]
      : page === "older"
        ? ["older", t("Still running"), t("What the earlier interface set going")]
      : GROUPS.flatMap((group) => group.items).find(([id]) => id === page);

  return (
    <Dialog open onOpenChange={(open) => { if (!open) props.onClose(); }}>
      <DialogContent className="h-[min(690px,88vh)] w-[min(980px,94vw)] max-w-[min(980px,94vw)] grid-cols-[210px_minmax(0,1fr)] gap-0 overflow-hidden p-0 sm:max-w-[min(980px,94vw)] max-[760px]:grid-cols-[168px_minmax(0,1fr)]">
        <Button
          variant="ghost"
          className="absolute top-3.5 right-4 z-20 size-7 px-0 text-muted-foreground"
          type="button"
          onClick={props.onClose}
          title={t("Close (Esc)")}
          aria-label={t("Close settings")}
        >
          <CloseIcon />
        </Button>

        <aside className="flex h-full flex-col justify-between overflow-hidden border-r border-border bg-muted/40 px-2.5 pt-4 pb-3">
          <div className="flex min-h-0 flex-1 flex-col gap-3.5 overflow-y-auto pr-0.5">
            {GROUPS.map((group) => (
              <div className="flex flex-col gap-0.5" key={group.title}>
                <div className={RAIL_GROUP}>{group.title}</div>
                {group.items.map(([id, label]) => (
                  <RailLink
                    key={id}
                    active={page === id && !selectedProjectId}
                    onClick={() => { setPage(id); setSelectedProjectId(null); setError(""); }}
                  >
                    <span>{label}</span>
                    {id === "llm" ? <small className="text-[11px] text-muted-foreground">{props.models.length}</small> : null}
                    {id === "mcp" ? <small className="text-[11px] text-muted-foreground">{props.mcp.length}</small> : null}
                  </RailLink>
                ))}
              </div>
            ))}

            {props.projects && props.projects.length > 0 ? (
              <div className="flex flex-col gap-0.5">
                <div className={RAIL_GROUP}>{t("Projects")}</div>
                {props.projects.map((proj) => (
                  <RailLink
                    key={proj.id}
                    active={selectedProjectId === proj.id}
                    onClick={() => { setSelectedProjectId(proj.id); setError(""); }}
                  >
                    <span>{proj.name}</span>
                  </RailLink>
                ))}
              </div>
            ) : null}
          </div>

          <div className="mt-1.5 flex shrink-0 flex-col gap-1.5 border-t border-border pt-2.5">
            <RailLink
              active={page === "shortcuts" && !selectedProjectId}
              onClick={() => { setPage("shortcuts"); setSelectedProjectId(null); setError(""); }}
            >
              <span>{t("Shortcuts")}</span>
            </RailLink>
            <RailLink
              active={page === "older" && !selectedProjectId}
              onClick={() => { setPage("older"); setSelectedProjectId(null); setError(""); }}
            >
              <span>{t("Still running")}</span>
            </RailLink>
          </div>
        </aside>

        <div className="min-h-0 overflow-y-auto px-8.5 pb-12 max-[760px]:px-5 max-[760px]:pb-9" ref={paneRef}>
          <header className="sticky top-0 z-5 -mx-8.5 mb-5 flex items-start justify-between gap-4 border-b border-border bg-popover py-5.5 pr-16 pb-3.5 pl-8.5 max-[760px]:-mx-5 max-[760px]:mb-4 max-[760px]:pl-5">
            <div>
              <DialogTitle id="settings-dialog-title" className="text-xl font-semibold tracking-tight">{currentMeta?.[1] ?? t("Settings")}</DialogTitle>
              {currentMeta?.[2] ? <DialogDescription className="mt-0.5 text-[12.5px]">{currentMeta[2]}</DialogDescription> : null}
            </div>
          </header>

          {error ? <Notice tone="bad" className="mb-3">{error}</Notice> : null}

          <div className={classes("flex flex-col gap-3", page === "telemetry" ? "max-w-[1180px]" : "max-w-[820px]")}>
            {selectedProject ? (
              <ProjectSettingsTab
                onRemoved={props.onProjectRemoved}
                project={selectedProject}
                onError={setError}
              />
            ) : page === "shortcuts" ? (
              <ShortcutsTab />
            ) : (
              <>
                {page === "usage" ? <UsageBilling refreshOn={[props.models, props.agents]} /> : null}
                {page === "memory" ? <Memory projects={props.projects ?? []} /> : null}
                {page === "permissions" ? <PermissionsPage projects={props.projects ?? []} agents={props.agents} onError={setError} /> : null}
                {page === "hooks" ? <HooksPage projects={props.projects ?? []} onError={setError} /> : null}
                {page === "plugins" ? <PluginsPage onError={setError} /> : null}
                {page === "mcp" ? (
                  <McpPage
                    agent={props.agent}
                    mcp={props.mcp}
                    rag={props.rag}
                    onToggle={props.onPatchAgent}
                    onChanged={async () => { await props.onChanged(); await load(); }}
                    onError={setError}
                  />
                ) : null}
                {page === "llm" ? (
                  <LlmPage
                    models={props.models}
                    onChanged={async () => { await props.onChanged(); await load(); }}
                    onError={setError}
                  />
                ) : null}
                {page === "information" && info ? (
                  <About
                    info={info}
                    onRerunOnboarding={() => void desktop.admin.updateSettings({ onboardedAt: null }).then((next) => { props.onProfile(next); props.onClose(); })}
                  />
                ) : null}
                {page === "logs" ? <Logs lines={logs} onReload={() => void load()} /> : null}
                {page === "telemetry" ? <Telemetry /> : null}
                {page === "older" ? <OlderFeatures onError={setError} /> : null}
                {page === "settings" && profile ? (
                  <General
                    profile={profile}
                    models={props.models}
                    theme={props.theme}
                    onSetTheme={props.onSetTheme}
                    onToggleTheme={props.onToggleTheme}
                    onSaved={async (next) => { setProfile(next); props.onProfile(next); await load(); }}
                    onError={setError}
                  />
                ) : null}
              </>
            )}
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}

/** A group's name in the rail. */
const RAIL_GROUP = "px-2 pt-1 pb-0.5 text-[11px] font-semibold text-muted-foreground";

/** A page of settings in the rail. */
function RailLink(props: { active: boolean; onClick: () => void; children: ReactNode }) {
  return (
    <Button
      type="button"
      variant="ghost"
      className={classes("w-full justify-between px-2.25 text-[13px] font-normal text-muted-foreground", props.active && "bg-card font-medium text-foreground")}
      onClick={props.onClick}
    >
      {props.children}
    </Button>
  );
}

function Memory(props: { projects: Project[] }) {
  const [scope, setScope] = useState("global");
  const [content, setContent] = useState("");
  const [saved, setSaved] = useState("");
  const [error, setError] = useState("");

  useEffect(() => {
    let current = true;
    desktop.admin.memory(scope).then(
      (result) => {
        if (!current) return;
        setContent(result.content);
        setSaved(result.content);
        setError("");
      },
      (reason: unknown) => current && setError(reason instanceof Error ? reason.message : t("Could not load memory")),
    );
    return () => {
      current = false;
    };
  }, [scope]);

  async function save() {
    try {
      const result = await desktop.admin.saveMemory(scope, content);
      setContent(result.content);
      setSaved(result.content);
      setError("");
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : t("Could not save"));
    }
  }

  return (
    <div>
      <SectionTitle>{t("Memory")}</SectionTitle>
      <SettingsCard>
        <SettingsRow>
          <RowText>
            <RowLabel>{t("Notes")}</RowLabel>
            <RowDescription>{t("One line per fact. Agents read these at the start of every chat (project notes in that project's chats, user notes everywhere). Delete a line to make the app forget it.")}</RowDescription>
          </RowText>
          <Choice className="w-45" ariaLabel={t("Memory scope")} value={scope} options={[{ value: "global", label: t("About you (every chat)") }, ...props.projects.map((project) => ({ value: project.id, label: project.name }))]} onChange={setScope} />
        </SettingsRow>
        <SettingsRow stacked>
          <Textarea className="min-h-55 w-full resize-y font-mono text-[12.5px] leading-normal field-sizing-fixed" aria-label={t("Memory notes")} value={content} rows={14} placeholder={scope === "global" ? t("Nothing remembered yet. Notes appear after a few turns of chat, or write your own: \"- Prefers short answers\".") : t("Nothing remembered about this project yet.")} onChange={(event) => setContent(event.target.value)} />
          <RowActions>
            <Button type="button" variant="default" size="small" disabled={content === saved} onClick={() => void save()}>{t("Save")}</Button>
            {content !== saved ? <Button type="button" variant="outline" size="small" onClick={() => setContent(saved)}>{t("Discard")}</Button> : null}
          </RowActions>
        </SettingsRow>
        {error ? <RowError>{error}</RowError> : null}
      </SettingsCard>
    </div>
  );
}

/** The log filters' choice for everything, since a choice cannot be empty. */
const ALL_LOGS = "all";

function Logs(props: { lines: LogLine[]; onReload: () => void }) {
  const [service, setService] = useState("");
  const [level, setLevel] = useState("");
  const services = [...new Set(props.lines.map((item) => item.service))].sort();
  const rank = { debug: 0, info: 1, warning: 2, error: 3 } as const;
  const shown = props.lines.filter((item) => {
    if (service && item.service !== service) return false;
    if (level && rank[item.level] < rank[level as keyof typeof rank]) return false;
    return true;
  });
  return (
    <>
      <div className="flex items-center gap-2">
        <Choice className="min-w-35" ariaLabel={t("Service")} value={service || ALL_LOGS} options={[{ value: ALL_LOGS, label: t("All services") }, ...services.map((name) => ({ value: name, label: name }))]} onChange={(next) => setService(next === ALL_LOGS ? "" : next)} />
        <Choice
          className="min-w-35"
          ariaLabel={t("Level")}
          value={level || ALL_LOGS}
          options={[{ value: ALL_LOGS, label: t("All levels") }, { value: "info", label: "info+" }, { value: "warning", label: "warning+" }, { value: "error", label: "error" }]}
          onChange={(next) => setLevel(next === ALL_LOGS ? "" : next)}
        />
        <Button variant="outline" type="button" onClick={props.onReload}>{t("Refresh")}</Button>
      </div>
      <div className="h-[54vh] overflow-auto rounded-lg border border-border bg-muted px-3 py-2.5 font-mono text-xs leading-normal">
        {shown.length === 0 ? <div className="text-[12.5px] text-muted-foreground">{t("No log lines matching filter.")}</div> : shown.map((item) => (
          <div className={classes("grid grid-cols-[88px_72px_64px_1fr] gap-2.5 py-0.5", item.level === "debug" && "opacity-60", item.level === "warning" && "text-wait", item.level === "error" && "text-destructive")} key={item.seq}>
            <span className="text-muted-foreground">{item.ts.slice(11, 23)}</span>
            <span className="text-muted-foreground">{item.service}</span>
            <span>{item.level}</span>
            <span>{item.message}</span>
          </div>
        ))}
      </div>
    </>
  );
}

function ShortcutsTab() {
  const shortcuts = [
    [shortcut("⌘ K"), t("Commands")],
    [t("⌘ 1 to 7"), t("Go to a screen")],
    [shortcut("⌘ J"), t("Show or hide the research assistant")],
    [shortcut("⌘ S"), t("Save the open file")],
    [shortcut("⌘ ,"), t("Open Settings")],
    ["Enter", t("Send a message")],
    ["Shift + Enter", t("New line in a message")],
    ["Esc", t("Close popups")],
  ];
  return (
    <div className="flex max-w-[760px] flex-col gap-5">
      <div>
        <SectionTitle>{t("Keyboard Shortcuts")}</SectionTitle>
        <SettingsCard>
          {shortcuts.map(([keys, label]) => (
            <SettingsRow key={keys}>
              <RowText>
                <RowLabel>{label}</RowLabel>
              </RowText>
              <kbd className="rounded-md border border-border bg-muted px-2 py-0.5 font-mono text-xs">
                {keys}
              </kbd>
            </SettingsRow>
          ))}
        </SettingsCard>
      </div>
    </div>
  );
}
