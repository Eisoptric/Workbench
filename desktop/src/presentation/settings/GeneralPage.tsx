import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { desktop } from "../../application/desktop";
import type { AgentBrowser, Model, Profile, SandboxMode, SandboxSettings } from "../../domain/model";
import { ComputerPermissions } from "./ComputerPermissions";
import { SnapshotStorage } from "./SnapshotStorage";
import { Button } from "../ui/base/Button.js";
import { Choice } from "../ui/base/Field.js";
import { Input } from "../ui/base/Input.js";
import { Switch } from "../ui/base/Switch.js";
import { InlineForm, Pills, RowDescription, RowLabel, RowText, SectionTitle, SettingsCard, SettingsRow } from "./Parts";
import { chooseLanguage, languageChoice } from "../../application/language";
import { t } from "../../domain/i18n";
import { sandboxReason } from "../../domain/sandbox";
import { MoonIcon, SunIcon } from "./SettingsIcons";
import { ClaudeCodeLogin, CodexLogin, ProgramGate } from "./ProgramAccess";

/** Settings → General → Command line: install the `workbench` command, which runs this app's own CLI. */
function CommandLine(props: { onError: (message: string) => void }) {
  const queryClient = useQueryClient();
  const { data: status = null } = useQuery({
    queryKey: ["admin", "cliStatus"],
    queryFn: () => desktop.admin.cliStatus(),
  });
  const install = useMutation({
    mutationFn: () => desktop.admin.installCli(),
    onSuccess: (next) => {
      queryClient.setQueryData(["admin", "cliStatus"], next);
      props.onError("");
    },
    onError: (reason) => {
      props.onError(reason instanceof Error ? reason.message : t("Could not install the command"));
    },
  });
  return (
    <div>
      <SectionTitle>{t("Command Line")}</SectionTitle>
      <SettingsCard>
        <SettingsRow>
          <RowText>
            <RowLabel>{t("The workbench command")}</RowLabel>
            <RowDescription>
              {t("Run agents from a terminal or a script while the app is open:")} <code>workbench run "fix the failing test" --agent "Workbench" --project .</code>, <code>workbench runs</code>, <code>workbench approve</code>{t(". It uses this app's own runtime, so it never goes out of date.")}
              {status?.installed ? " " + (status.current ? t("Installed at {path}.", { path: status.path ?? "" }) : t("Installed at {path}, for another copy of Reinstall to point it here.", { path: status.path ?? "" })) : ""}
              {status?.installed && !status.onPath ? " " + t('Add {folder} to your PATH to type just "workbench".', { folder: status.folder ?? "" }) : ""}
            </RowDescription>
          </RowText>
          <Button type="button" variant="default" size="small" disabled={install.isPending || status?.current === true} onClick={() => install.mutate()}>
            {status?.current ? t("Installed") : status?.installed ? t("Reinstall") : install.isPending ? t("Installing…") : t("Install")}
          </Button>
        </SettingsRow>
      </SettingsCard>
    </div>
  );
}

/** Says when this computer cannot sandbox commands, and on Windows offers the one-time setup that makes it able to. */
function SandboxSetup(props: { onError: (message: string) => void }) {
  const queryClient = useQueryClient();
  const [done, setDone] = useState(false);
  const { data: status = null } = useQuery({
    queryKey: ["admin", "sandboxStatus"],
    queryFn: () => desktop.admin.sandboxStatus(),
  });
  const setUp = useMutation({
    mutationFn: () => desktop.admin.setUpSandbox(),
    onSuccess: (next) => {
      queryClient.setQueryData(["admin", "sandboxStatus"], next);
      setDone(next.available);
      props.onError(next.available ? "" : t("The sandbox is still off: {reason}.", { reason: sandboxReason(next.reason, t) }));
    },
    onError: (reason) => {
      props.onError(reason instanceof Error ? sandboxReason(reason.message, t) : t("Could not set up the sandbox"));
    },
  });
  if (!status || (status.available && !done)) return null;
  return (
    <SettingsRow>
      <RowText>
        <RowLabel>{status.available ? t("The sandbox is set up") : t("The sandbox is off on this computer")}</RowLabel>
        <RowDescription>
          {status.available
            ? t("Commands now run in the sandbox chosen above.")
            : status.setup
              ? t("Windows needs a one-time setup by an administrator before it can sandbox commands; until then they run as you, whatever is chosen above. The setup lets sandboxed programs see that the folders above your home folder and your projects exist, and read the programs on PATH. Windows asks for an administrator first.")
              : t("Commands run as you, whatever is chosen above: {reason}.", { reason: sandboxReason(status.reason, t) })}
        </RowDescription>
      </RowText>
      {status.setup ? (
        <Button type="button" variant="default" size="small" disabled={setUp.isPending} onClick={() => setUp.mutate()}>
          {setUp.isPending ? t("Setting up…") : t("Set up")}
        </Button>
      ) : null}
    </SettingsRow>
  );
}

/** The background model choice for letting the agent's own model do the work, since a choice cannot be empty. */
const AUTOMATIC_MODEL = "automatic";

export function General(props: {
  profile: Profile;
  models: Model[];
  onSaved: (profile: Profile) => Promise<void>;
  onError: (message: string) => void;
  theme?: "light" | "dark";
  onSetTheme?: (theme: "light" | "dark") => void;
  onToggleTheme?: () => void;
}) {
  const [name, setName] = useState(props.profile.name);
  const [logging, setLogging] = useState(props.profile.logging);
  const [memory, setMemory] = useState(props.profile.memory !== false);
  const [notify, setNotify] = useState(localStorage.getItem("workbench-notify") !== "off");
  const [planApproval, setPlanApproval] = useState(props.profile.planApproval === true);
  const [claudeProjectSettings, setClaudeProjectSettings] = useState(props.profile.claudeProjectSettings === true);
  const [agentBrowser, setAgentBrowser] = useState<AgentBrowser>(props.profile.browser ?? "workbench");
  // Safari's automation (safaridriver) exists only on macOS.
  const isMac = /Mac/i.test(navigator.userAgent);

  async function chooseAgentBrowser(next: AgentBrowser) {
    if (next === agentBrowser) return;
    const previous = agentBrowser;
    setAgentBrowser(next);
    try {
      await props.onSaved(await desktop.admin.updateSettings({ browser: next }));
      props.onError("");
    } catch (reason) {
      setAgentBrowser(previous);
      props.onError(reason instanceof Error ? reason.message : t("Could not save"));
    }
  }

  async function chooseClaudeProjectSettings(next: boolean) {
    setClaudeProjectSettings(next);
    try {
      await props.onSaved(await desktop.admin.updateSettings({ claudeProjectSettings: next }));
      props.onError("");
    } catch (reason) {
      setClaudeProjectSettings(!next);
      props.onError(reason instanceof Error ? reason.message : t("Could not save"));
    }
  }
  const [sandbox, setSandbox] = useState<SandboxSettings>(props.profile.sandbox ?? { mode: "workspace-write", network: true });

  async function chooseSandbox(next: SandboxSettings) {
    const previous = sandbox;
    setSandbox(next);
    try {
      await props.onSaved(await desktop.admin.updateSettings({ sandbox: next }));
      props.onError("");
    } catch (reason) {
      setSandbox(previous);
      props.onError(reason instanceof Error ? reason.message : t("Could not save"));
    }
  }
  async function chooseFlag(flag: "memory" | "planApproval", next: boolean) {
    const set = flag === "memory" ? setMemory : setPlanApproval;
    set(next);
    try {
      await props.onSaved(await desktop.admin.updateSettings({ [flag]: next }));
      props.onError("");
    } catch (reason) {
      set(!next);
      props.onError(reason instanceof Error ? reason.message : t("Could not save"));
    }
  }

  const [saved, setSaved] = useState(false);
  const [backgroundModelId, setBackgroundModelId] = useState(props.profile.completionModelId ?? "");
  // Claude Code and Codex models run an agent loop on the user's plan; background work needs a plain API call.
  const backgroundChoices = props.models.filter((item) => item.provider !== "claude-code" && item.provider !== "codex");

  async function chooseBackgroundModel(next: string) {
    const previous = backgroundModelId;
    setBackgroundModelId(next);
    try {
      await props.onSaved(await desktop.admin.updateSettings({ completionModelId: next || null }));
      props.onError("");
    } catch (reason) {
      setBackgroundModelId(previous);
      props.onError(reason instanceof Error ? reason.message : t("Could not save"));
    }
  }

  async function save() {
    try {
      const next = await desktop.admin.updateSettings({ name, logging });
      setSaved(true);
      props.onError("");
      await props.onSaved(next);
      setTimeout(() => setSaved(false), 2000);
    } catch (reason) {
      props.onError(reason instanceof Error ? reason.message : t("Could not save"));
    }
  }

  const themeButtons = (
    <Pills
      label={t("Theme")}
      value={props.theme}
      options={[["light", t("Light"), <SunIcon key="sun" />], ["dark", t("Dark"), <MoonIcon key="moon" />]] as const}
      onChoose={(value) => {
        if (props.onSetTheme) props.onSetTheme(value);
        else if (props.theme !== value) props.onToggleTheme?.();
      }}
    />
  );

  return (
    <div className="flex max-w-[760px] flex-col gap-5">
      <div>
        <SectionTitle>{t("General")}</SectionTitle>
        <SettingsCard>
          <SettingsRow>
            <RowText>
              <RowLabel>{t("Agent browser")}</RowLabel>
              <RowDescription>
                {agentBrowser === "system"
                  ? t("Agents browse in a visible window of your Chrome (or Edge, Brave), with the app's own profile: your logins and tabs stay out of it. You watch the cursor there.")
                  : agentBrowser === "safari"
                    ? t("Agents browse in a Safari automation window (striped address bar), clean of your logins; you cannot click into it while the agent works. Turn on Develop → Allow Remote Automation in Safari once. The Browser tab shows it about once a second.")
                    : t("Agents browse in the app's own browser, without a window, shown live with the agent's cursor in a Browser tab of the Workspace.")}
              </RowDescription>
            </RowText>
            <Pills
              label={t("Agent browser")}
              value={agentBrowser}
              options={isMac ? ([["workbench", "Workbench"], ["system", "Chrome"], ["safari", "Safari"]] as const) : ([["workbench", "Workbench"], ["system", "Chrome"]] as const)}
              onChoose={(value) => void chooseAgentBrowser(value)}
            />
          </SettingsRow>
          <SettingsRow>
            <RowText>
              <RowLabel>{t("Theme")}</RowLabel>
              <RowDescription>{t("Light or dark appearance.")}</RowDescription>
            </RowText>
            {themeButtons}
          </SettingsRow>
          <SettingsRow>
            <RowText>
              <RowLabel>{t("Language")}</RowLabel>
              <RowDescription>{t("The interface's language. The app reloads to show it.")}</RowDescription>
            </RowText>
            <Pills
              label={t("Language")}
              value={languageChoice()}
              options={[["auto", t("System")], ["en", "English"], ["zh-Hant", "繁體中文"]] as const}
              onChoose={(value) => {
                if (value !== languageChoice()) chooseLanguage(value);
              }}
            />
          </SettingsRow>
        </SettingsCard>
      </div>

      <div>
        <SectionTitle>{t("Computer use")}</SectionTitle>
        <SettingsCard>
          <ComputerPermissions />
        </SettingsCard>
      </div>

      <div>
        <SectionTitle>{t("Storage")}</SectionTitle>
        <SettingsCard>
          <SnapshotStorage onError={props.onError} />
        </SettingsCard>
      </div>

      <div>
        <SectionTitle>Claude Code</SectionTitle>
        <SettingsCard>
          <ProgramGate id="claude-code">
            <ClaudeCodeLogin />
          </ProgramGate>
          <SettingsRow>
            <RowText>
              <RowLabel>{t("Use the project's Claude Code settings")}</RowLabel>
              <RowDescription>{t("Claude Code loads the chat's project")} <code>.claude/settings.json</code>{t(": its hooks, permission rules and settings, as it would in a terminal there. Your own Claude settings are never loaded. Off by default, since a project's hooks run commands.")}</RowDescription>
            </RowText>
            <Switch checked={claudeProjectSettings} aria-label={t("Use the project's Claude Code settings")} onCheckedChange={(next) => void chooseClaudeProjectSettings(next)} />
          </SettingsRow>
        </SettingsCard>
      </div>

      <div>
        <SectionTitle>Codex</SectionTitle>
        <SettingsCard>
          <ProgramGate id="codex">
            <CodexLogin />
          </ProgramGate>
        </SettingsCard>
      </div>

      <div>
        <SectionTitle>{t("Models")}</SectionTitle>
        <SettingsCard>
          <SettingsRow>
            <RowText>
              <RowLabel>{t("Background Model")}</RowLabel>
              <RowDescription>
                {t("Writes memory notes from your chats. Chat content goes to this model. On Automatic, memory notes use the agent's model.")}
              </RowDescription>
            </RowText>
            <Choice
              className="w-45"
              ariaLabel={t("Background model")}
              value={backgroundModelId || AUTOMATIC_MODEL}
              disabled={!backgroundChoices.length && !backgroundModelId}
              options={[{ value: AUTOMATIC_MODEL, label: t("Automatic") }, ...backgroundChoices.map((item) => ({ value: item.id, label: item.name }))]}
              onChange={(next) => void chooseBackgroundModel(next === AUTOMATIC_MODEL ? "" : next)}
            />
          </SettingsRow>
        </SettingsCard>
      </div>

      <div>
        <SectionTitle>{t("Agents")}</SectionTitle>
        <SettingsCard>
          <SettingsRow>
            <RowText>
              <RowLabel>{t("Memory")}</RowLabel>
              <RowDescription>{t("Every few turns, the app notes durable facts from the chat (how you like to work, a project's conventions) and gives them to agents in later chats. Read and edit the notes under Memory.")}</RowDescription>
            </RowText>
            <Switch checked={memory} aria-label={t("Memory")} onCheckedChange={(next) => void chooseFlag("memory", next)} />
          </SettingsRow>
          <SettingsRow>
            <RowText>
              <RowLabel>{t("Notifications")}</RowLabel>
              <RowDescription>{t("A system notification when an agent finishes, fails or needs your approval while you are not looking at its chat. Click it to open the chat.")}</RowDescription>
            </RowText>
            <Switch checked={notify} aria-label={t("Notifications")} onCheckedChange={(next) => { setNotify(next); localStorage.setItem("workbench-notify", next ? "on" : "off"); }} />
          </SettingsRow>
          <SettingsRow>
            <RowText>
              <RowLabel>{t("Plan approval")}</RowLabel>
              <RowDescription>{t('Agents write a plan and wait for your go-ahead before changing files or running commands. Reply "go" to proceed, or say what to change.')}</RowDescription>
            </RowText>
            <Switch checked={planApproval} aria-label={t("Plan approval")} onCheckedChange={(next) => void chooseFlag("planApproval", next)} />
          </SettingsRow>
        </SettingsCard>
      </div>

      <CommandLine onError={props.onError} />

      <div>
        <SectionTitle>{t("Sandbox")}</SectionTitle>
        <SettingsCard>
          <SettingsRow>
            <RowText>
              <RowLabel>{t("Commands")}</RowLabel>
              <RowDescription>
                {t("Where a shell command run by an agent may write. ")}
                <strong>{t("Workspace")}</strong>
                {t(" confines it to the project folder (plus temp and cache folders) with the operating system's sandbox, so a stray ")}
                <code>rm</code>
                {t(" or a script cannot touch anything else; ")}
                <strong>{t("Read-only")}</strong>
                {t(" lets it write nowhere; ")}
                <strong>{t("Off")}</strong>
                {t(" runs it as you. An agent can ask to run one command outside the sandbox, which shows an approval card. macOS and Linux (with bubblewrap) sandbox; Windows does after a one-time setup. A project can override this.")}
              </RowDescription>
            </RowText>
            <Choice ariaLabel={t("Sandbox mode")} value={sandbox.mode} options={[{ value: "workspace-write", label: t("Workspace") }, { value: "read-only", label: t("Read-only") }, { value: "off", label: t("Off") }]} onChange={(next) => void chooseSandbox({ ...sandbox, mode: next as SandboxMode })} />
          </SettingsRow>
          <SettingsRow>
            <RowText>
              <RowLabel>{t("Network")}</RowLabel>
              <RowDescription>{t("Whether sandboxed commands may use the network (package installs, git push, curl). Off blocks every connection except local sockets; the agent can still ask to run a command outside the sandbox.")}</RowDescription>
            </RowText>
            <Switch checked={sandbox.network} aria-label={t("Network for commands")} onCheckedChange={(next) => void chooseSandbox({ ...sandbox, network: next })} />
          </SettingsRow>
          <SandboxSetup onError={props.onError} />
        </SettingsCard>
      </div>

      <div>
        <SectionTitle>{t("Workspace")}</SectionTitle>
        <SettingsCard>
          <SettingsRow>
            <RowText>
              <RowLabel>{t("Workspace Name")}</RowLabel>
              <RowDescription>{t("Shown in the title bar and settings.")}</RowDescription>
            </RowText>
            <InlineForm
              onSubmit={(event) => {
                event.preventDefault();
                void save();
              }}
            >
              <Input
                className="w-45"
                aria-label={t("Workspace name")}
                value={name}
                onChange={(event) => { setName(event.target.value); setSaved(false); }}
              />
              <Button type="submit" variant="default" size="small" disabled={!name.trim() || name === props.profile.name}>
                {saved ? t("Saved ✓") : t("Save")}
              </Button>
            </InlineForm>
          </SettingsRow>

          <SettingsRow>
            <RowText>
              <RowLabel>{t("Log Level")}</RowLabel>
              <RowDescription>{t("How much the runtime writes to Logs.")}</RowDescription>
            </RowText>
            <Pills
              label={t("Log level")}
              value={logging}
              options={[["debug", t("Debug")], ["info", t("Info")], ["warning", t("Warning")], ["error", t("Error")]] as const}
              onChoose={(level) => {
                const previous = logging;
                setLogging(level);
                desktop.admin.updateSettings({ logging: level }).then(props.onSaved, (reason: unknown) => {
                  setLogging(previous);
                  props.onError(reason instanceof Error ? reason.message : t("Could not save"));
                });
              }}
            />
          </SettingsRow>

          <SettingsRow>
            <RowText>
              <RowLabel>{t("API Keys")}</RowLabel>
              <RowDescription>{t("Kept on this Mac in a file only your user account can read.")}</RowDescription>
            </RowText>
            <code className="rounded-md bg-muted px-2 py-0.5 text-[11.5px] whitespace-nowrap text-muted-foreground">~/.workbench-app/secrets.json</code>
          </SettingsRow>
        </SettingsCard>
      </div>
    </div>
  );
}
