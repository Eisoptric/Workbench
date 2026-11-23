import { useEffect, useState } from "react";
import { desktop } from "../../application/desktop";
import { ProjectPermissions } from "./Permissions";
import { InstructionsReview, useInstructions } from "../InstructionFiles";
import type { Project, SandboxMode, SandboxSettings } from "../../domain/model";
import { askToConfirm } from "../../infrastructure/confirm";
import { Button } from "../ui/base/Button.js";
import { Choice } from "../ui/base/Field.js";
import { Input } from "../ui/base/Input.js";
import { Switch } from "../ui/base/Switch.js";
import { Textarea } from "../ui/base/Textarea.js";
import { InlineForm, RowDescription, RowLabel, RowText, SectionTitle, SettingsCard, SettingsRow } from "./Parts";
import { t } from "../../domain/i18n";

export function ProjectSettingsTab(props: {
  project: Project;
  onError: (message: string) => void;
  onRemoved?: () => void;
}) {
  const [rules, setRules] = useState("");
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [verifyCommand, setVerifyCommand] = useState(props.project.verifyCommand ?? "");
  const [verifySaved, setVerifySaved] = useState(false);
  const [sandbox, setSandbox] = useState<SandboxSettings | null>(props.project.sandbox ?? null);
  const { instructions, set: setInstructions, refresh: refreshInstructions } = useInstructions(props.project.id);

  async function chooseSandbox(next: SandboxSettings | null) {
    const previous = sandbox;
    setSandbox(next);
    try {
      await desktop.projects.update(props.project.id, { sandbox: next });
    } catch (reason) {
      setSandbox(previous);
      props.onError(reason instanceof Error ? reason.message : t("Could not save"));
    }
  }

  /** Takes the project and its chats out of the app. Its folder and everything in it stay. */
  async function removeProject() {
    if (!await askToConfirm(t("Remove {name} from the app? Its chats go with it. The folder and its files stay where they are.", { name: props.project.name }))) return;
    try {
      await desktop.projects.delete(props.project.id);
      props.onRemoved?.();
    } catch (reason) {
      props.onError(reason instanceof Error ? reason.message : t("Could not remove the project"));
    }
  }

  async function saveVerify() {
    try {
      await desktop.projects.update(props.project.id, { verifyCommand: verifyCommand.trim() });
      setVerifySaved(true);
      setTimeout(() => setVerifySaved(false), 2000);
    } catch (reason) {
      props.onError(reason instanceof Error ? reason.message : t("Could not save"));
    }
  }

  useEffect(() => {
    let active = true;
    setLoading(true);
    desktop.projects
      .getRules(props.project.id)
      .then((result) => {
        if (!active) return;
        setRules(result.content);
        setLoading(false);
      })
      .catch(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, [props.project.id]);

  async function saveRules() {
    setSaving(true);
    try {
      await desktop.projects.saveRules(props.project.id, rules);
      await refreshInstructions();
      setSaved(true);
      setTimeout(() => setSaved(false), 2000);
    } catch (reason) {
      props.onError(reason instanceof Error ? reason.message : t("Failed to save project rules"));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="flex max-w-[760px] flex-col gap-5">
      <div>
        <SectionTitle>{t("Project Details")}</SectionTitle>
        <SettingsCard>
          <SettingsRow>
            <RowText>
              <RowLabel>{t("Project Name")}</RowLabel>
              <RowDescription>{props.project.name}</RowDescription>
            </RowText>
          </SettingsRow>
          <SettingsRow>
            <RowText>
              <RowLabel>{t("Workspace Path")}</RowLabel>
              <RowDescription className="font-mono">{props.project.path}</RowDescription>
            </RowText>
          </SettingsRow>
          {props.project.gitBranch ? (
            <SettingsRow>
              <RowText>
                <RowLabel>{t("Git Branch")}</RowLabel>
                <RowDescription className="font-mono">{props.project.gitBranch}</RowDescription>
              </RowText>
            </SettingsRow>
          ) : null}
        </SettingsCard>
      </div>

      <div>
        <SectionTitle>{t("Check After Changes")}</SectionTitle>
        <SettingsCard>
          <SettingsRow>
            <RowText>
              <RowLabel>{t("Verify command")}</RowLabel>
              <RowDescription>{t('Run after an agent changes files here; a failure goes straight back to the agent to fix (at most twice per message). Use "auto" for package.json\'s typecheck or lint script, or leave empty to skip.')}</RowDescription>
            </RowText>
            <InlineForm onSubmit={(event) => { event.preventDefault(); void saveVerify(); }}>
              <Input className="w-45" aria-label={t("Verify command")} value={verifyCommand} placeholder={t("auto, or e.g. npm test")} onChange={(event) => setVerifyCommand(event.target.value)} />
              <Button type="submit" variant="default" size="small" disabled={(props.project.verifyCommand ?? "") === verifyCommand.trim()}>{verifySaved ? t("Saved") : t("Save")}</Button>
            </InlineForm>
          </SettingsRow>
        </SettingsCard>
      </div>

      <div>
        <SectionTitle>{t("Sandbox")}</SectionTitle>
        <SettingsCard>
          <SettingsRow>
            <RowText>
              <RowLabel>{t("Commands in this project")}</RowLabel>
              <RowDescription>{t("Overrides the sandbox under Settings → General for commands run here. Workspace confines writes to this folder; Read-only allows none; Off runs commands as you.")}</RowDescription>
            </RowText>
            <div className="flex items-center gap-2">
              <Choice
                ariaLabel={t("Sandbox for this project")}
                value={sandbox?.mode ?? "inherit"}
                options={[{ value: "inherit", label: t("Same as General") }, { value: "workspace-write", label: t("Workspace") }, { value: "read-only", label: t("Read-only") }, { value: "off", label: t("Off") }]}
                onChange={(next) => void chooseSandbox(next === "inherit" ? null : { mode: next as SandboxMode, network: sandbox?.network ?? true })}
              />
              {sandbox ? <Switch checked={sandbox.network} aria-label={t("Network for commands")} onCheckedChange={(next) => void chooseSandbox({ ...sandbox, network: next })} /> : null}
            </div>
          </SettingsRow>
        </SettingsCard>
      </div>

      <ProjectPermissions project={props.project} onError={props.onError} />

      {instructions ? (
        <div>
          <SectionTitle>{t("Instruction files")}</SectionTitle>
          <SettingsCard>
            <InstructionsReview projectId={props.project.id} instructions={instructions} onChanged={setInstructions} onError={props.onError} />
          </SettingsCard>
        </div>
      ) : null}

      <div>
        <SectionTitle>{t("Project Rules & Custom Instructions")}</SectionTitle>
        <SettingsCard>
          <div className="flex flex-col gap-2.5">
            <RowDescription>{t("Custom instructions for agents working in this project (.cursorrules / AGENTS.md).")}</RowDescription>
            <Textarea
              aria-label={t("Project Rules & Custom Instructions")}
              className="resize-y font-mono text-xs field-sizing-fixed"
              rows={7}
              value={rules}
              disabled={loading}
              placeholder={loading ? t("Loading project rules…") : t("Add custom instructions for this project…")}
              onChange={(event) => {
                setRules(event.target.value);
                setSaved(false);
              }}
            />
            <div className="flex justify-end">
              <Button type="button" variant="default" size="small" disabled={saving || loading} onClick={() => void saveRules()}>
                {saving ? t("Saving…") : saved ? t("Saved ✓") : t("Save Rules")}
              </Button>
            </div>
          </div>
        </SettingsCard>
      </div>

      <div>
        <SectionTitle>{t("Actions")}</SectionTitle>
        <SettingsCard className="flex flex-wrap gap-2.5">
          <Button type="button" variant="outline" size="small" onClick={() => void desktop.projects.openInTerminal(props.project.id)}>
            {t("Open in Terminal")}
          </Button>
          <Button type="button" variant="outline" size="small" onClick={() => void desktop.projects.openInFolder(props.project.id)}>
            {t("Reveal in Finder")}
          </Button>
          <Button type="button" variant="danger" size="small" onClick={() => void removeProject()}>
            {t("Remove from the app")}
          </Button>
        </SettingsCard>
      </div>
    </div>
  );
}
