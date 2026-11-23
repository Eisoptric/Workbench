import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { desktop } from "../../application/desktop";
import { t } from "../../domain/i18n";
import type { Hook, HookEvent, HookSet, Project } from "../../domain/model";
import { Badge } from "../ui/base/Badge.js";
import { Button } from "../ui/base/Button.js";
import { Input } from "../ui/base/Input.js";
import { Choice } from "../ui/base/Field.js";
import { Explained } from "./Explained";
import { FormHint, RowDescription, RowLabel, RuleCode, RuleEffect, RuleForm, RuleItem, RuleList, SectionTitle, SettingsCard, SettingsRow } from "./Parts";

const EVENTS: { id: HookEvent; label: string; hint: string }[] = [
  { id: "pre_tool", label: t("Before a tool"), hint: t("Exit 2 blocks the call (stderr goes to the agent). Print {\"decision\": \"allow\" | \"ask\" | \"deny\"} to skip the card, ask, or refuse.") },
  { id: "post_tool", label: t("After a tool"), hint: t("Exit 2 adds stderr to the result as feedback for the agent (a linter, a formatter's complaint).") },
  { id: "stop", label: t("When a turn ends"), hint: t("Exit 2 sends stderr back to the agent as a follow-up, at most twice per message (insist on green tests).") },
];

/** Each event as its word on a hook's badge. */
const EVENT_WORDS: Record<HookEvent, string> = { pre_tool: t("pre tool"), post_tool: t("post tool"), stop: t("stop") };

function HookList(props: { hooks: Hook[]; onRemove?: (index: number) => void }) {
  if (!props.hooks.length) return <p className="text-xs text-muted-foreground">{t("No hooks.")}</p>;
  return (
    <RuleList>
      {props.hooks.map((hook, index) => (
        <RuleItem key={`${hook.event}:${hook.command}:${index}`}>
          <RuleEffect kind="hook">{EVENT_WORDS[hook.event] ?? hook.event}</RuleEffect>
          {hook.event !== "stop" ? <RuleCode>{hook.matcher || t("any tool")}</RuleCode> : null}
          <RuleCode pattern title={hook.command}>{hook.command}</RuleCode>
          {props.onRemove ? (
            <Button type="button" variant="ghost" size="small" className="text-muted-foreground hover:bg-destructive/12 hover:text-destructive" onClick={() => props.onRemove!(index)} aria-label={t("Remove {command}", { command: hook.command })}>{t("Remove")}</Button>
          ) : null}
        </RuleItem>
      ))}
    </RuleList>
  );
}

/** Settings → Hooks: the user's own hooks, then each project's file hooks, which run only once allowed. */
export function HooksPage(props: { projects: Project[]; onError: (message: string) => void }) {
  const queryClient = useQueryClient();
  const [event, setEvent] = useState<HookEvent>("pre_tool");
  const [matcher, setMatcher] = useState("");
  const [command, setCommand] = useState("");
  const projectIds = props.projects.map((project) => project.id).join(",");

  const userQuery = useQuery({
    queryKey: ["settings", "hooks", "user"],
    queryFn: async () => {
      try {
        const sets = await desktop.admin.hooks();
        return sets.find((set) => set.source === "user")?.hooks ?? [];
      } catch (reason) {
        props.onError(reason instanceof Error ? reason.message : t("Could not load hooks"));
        throw reason;
      }
    },
  });

  const projectsQuery = useQuery({
    queryKey: ["settings", "hooks", "projects", projectIds],
    queryFn: async () => {
      const entries = await Promise.all(
        props.projects.map(async (project) => {
          try {
            const sets = await desktop.admin.hooks(project.id);
            return [project.id, sets.filter((set) => set.source === "project-file" || set.source === "claude-file")] as const;
          } catch {
            return [project.id, [] as HookSet[]] as const;
          }
        }),
      );
      return Object.fromEntries(entries) as Record<string, HookSet[]>;
    },
  });

  const saveMutation = useMutation({
    mutationFn: (next: Hook[]) => desktop.admin.setUserHooks(next),
    onSuccess: (sets) => {
      queryClient.setQueryData(["settings", "hooks", "user"], sets.find((set) => set.source === "user")?.hooks ?? []);
      props.onError("");
    },
    onError: (reason) => props.onError(reason instanceof Error ? reason.message : t("Could not save hooks")),
  });

  const allowMutation = useMutation({
    mutationFn: ({ projectId, set, allowed }: { projectId: string; set: HookSet; allowed: boolean }) =>
      allowed ? desktop.admin.approveHooks(projectId, set.path!, set.fingerprint!) : desktop.admin.revokeHooks(projectId, set.path!),
    onSuccess: (sets, { projectId }) => {
      queryClient.setQueryData<Record<string, HookSet[]>>(["settings", "hooks", "projects", projectIds], (prev) => ({
        ...(prev ?? {}),
        [projectId]: sets.filter((item) => item.source === "project-file" || item.source === "claude-file"),
      }));
    },
    onError: (reason) => props.onError(reason instanceof Error ? reason.message : t("Could not change the approval")),
  });

  const userHooks = userQuery.data ?? [];
  const projectSets = projectsQuery.data ?? {};
  const busy = saveMutation.isPending;
  const save = (next: Hook[]) => saveMutation.mutateAsync(next);
  const allow = (projectId: string, set: HookSet, allowed: boolean) => allowMutation.mutate({ projectId, set, allowed });

  const withFiles = props.projects.filter((project) => (projectSets[project.id] ?? []).length);
  return (
    <div className="flex max-w-[760px] flex-col gap-5">
      <Explained summary={t("A hook is a shell command that runs around an agent's tool calls, or when its turn ends.")}>
        <li>{t("It runs in the chat's project folder, and gets the event as JSON on stdin: the tool, its arguments and result, and the turn's answer.")}</li>
        <li>{t("Your hooks below run in every project.")}</li>
        <li>{t("A project can bring its own hooks file:")} <code>.workbench/hooks.json</code>. {t("The hooks in its Claude Code settings are read too (PreToolUse, PostToolUse and Stop):")} <code>.claude/settings.json</code>. {t("Those run only after you allow them here, and any change to the file asks again.")}</li>
      </Explained>

      <div>
        <SectionTitle>{t("Your hooks")}</SectionTitle>
        <SettingsCard>
          <SettingsRow stacked>
            <HookList hooks={userHooks} onRemove={(index) => void save(userHooks.filter((_, at) => at !== index))} />
            <RuleForm
              onSubmit={(formEvent) => {
                formEvent.preventDefault();
                if (!command.trim()) return;
                void save([...userHooks, { event, ...(event !== "stop" && matcher.trim() ? { matcher: matcher.trim() } : {}), command: command.trim() }]).then(() => { setCommand(""); setMatcher(""); });
              }}
            >
              <Choice ariaLabel={t("Hook event")} value={event} options={EVENTS.map((item) => ({ value: item.id, label: item.label }))} onChange={(next) => setEvent(next as HookEvent)} />
              {event !== "stop" ? <Input className="w-55 flex-none" aria-label={t("Tools")} placeholder={t("tools: code, files, github_* (empty: all)")} value={matcher} onChange={(changeEvent) => setMatcher(changeEvent.target.value)} /> : null}
              <Input className="w-55 flex-none font-mono" aria-label={t("Command")} placeholder="./scripts/check.sh" value={command} onChange={(changeEvent) => setCommand(changeEvent.target.value)} />
              <Button type="submit" disabled={busy || !command.trim()}>{t("Add hook")}</Button>
              <FormHint>{EVENTS.find((item) => item.id === event)?.hint}</FormHint>
            </RuleForm>
          </SettingsRow>
        </SettingsCard>
      </div>

      <div>
        <SectionTitle>{t("Project hooks")}</SectionTitle>
        <SettingsCard>
          {withFiles.length ? withFiles.map((project) => (projectSets[project.id] ?? []).map((set) => (
            <SettingsRow stacked key={`${project.id}:${set.path}`}>
              <div className="flex flex-wrap items-center gap-2">
                <RowLabel className="min-w-0 flex-1">{project.name} · <code>{set.path}</code></RowLabel>
                <Badge tone={set.approved ? "good" : "wait"}>{set.approved ? t("Allowed") : t("Not running: needs your approval")}</Badge>
                <Button type="button" variant={set.approved ? "outline" : "default"} size="small" onClick={() => void allow(project.id, set, !set.approved)}>
                  {set.approved ? t("Stop allowing") : t("Allow these hooks")}
                </Button>
              </div>
              <HookList hooks={set.hooks} />
              {!set.approved ? <RowDescription>{t("These commands come with the folder and run with your permissions. Read them before allowing.")}</RowDescription> : null}
            </SettingsRow>
          ))) : <RowDescription>{t("No project has hooks in either file:")} <code>.workbench/hooks.json</code>, <code>.claude/settings.json</code></RowDescription>}
        </SettingsCard>
      </div>
    </div>
  );
}
