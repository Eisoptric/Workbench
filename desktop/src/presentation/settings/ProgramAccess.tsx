import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState, type ReactNode } from "react";
import { desktop } from "../../application/desktop";
import type { CodexStatus, ProgramStatus } from "../../application/admin";
import { askToConfirm } from "../../infrastructure/confirm";
import { Button } from "../ui/base/Button.js";
import { LinkButton, RowActions, RowDescription, RowError, RowLabel, RowText, SettingsRow, Status } from "./Parts";
import { t } from "../../domain/i18n";

const COMMAND_BOX = "my-1.5 mb-1 flex items-center gap-2 rounded-lg border border-border bg-muted px-2.5 py-2";

/**
 * Claude Code and Codex are separate official programs that the app no longer ships in its installer. Until one is
 * found (The app's download, or the user's own install on the PATH), this offers the download; once it is, it shows
 * where it comes from and then the sign-in (children).
 */
export function ProgramGate(props: { id: ProgramStatus["id"]; children: ReactNode }) {
  const queryClient = useQueryClient();
  const [actionError, setActionError] = useState("");
  const query = useQuery({
    queryKey: ["admin", "program", props.id],
    queryFn: () => desktop.admin.program(props.id),
    refetchInterval: (current) => {
      const state = current.state.data?.install?.state;
      return state && ["downloading", "verifying", "unpacking"].includes(state) ? 700 : false;
    },
  });
  const status = query.data ?? null;
  const error = actionError || (query.error instanceof Error ? query.error.message : query.error ? t("Could not reach the runtime") : "");
  const busy = Boolean(status?.install && ["downloading", "verifying", "unpacking"].includes(status.install.state));

  async function install() {
    setActionError("");
    try {
      queryClient.setQueryData(["admin", "program", props.id], await desktop.admin.installProgram(props.id));
    } catch (reason) {
      setActionError(reason instanceof Error ? reason.message : t("Could not start the download"));
    }
  }

  async function remove() {
    if (!await askToConfirm(t("Remove the app's copy of {program}? {program} models stop working until it is downloaded again or found on your system.", { program: status?.label ?? "" }))) return;
    try {
      queryClient.setQueryData(["admin", "program", props.id], await desktop.admin.removeProgram(props.id));
    } catch (reason) {
      setActionError(reason instanceof Error ? reason.message : t("Could not remove it"));
    }
  }

  const label = status?.label ?? (props.id === "codex" ? "Codex" : "Claude Code");
  const megabytes = status?.downloadSize ? `${Math.round(status.downloadSize / 1048576)} MB` : "";
  if (status === null) {
    return (
      <SettingsRow>
        <RowText>
          <RowLabel>{t("Program")}</RowLabel>
          <RowDescription>{error || t("Checking…")}</RowDescription>
        </RowText>
      </SettingsRow>
    );
  }
  if (!status.installed) {
    const progress = status.install;
    const percent = progress && progress.total ? Math.min(100, Math.round((progress.received / progress.total) * 100)) : 0;
    return (
      <>
        <SettingsRow>
          <RowText>
            <RowLabel>{t("{program} is not installed", { program: label })}</RowLabel>
            <RowDescription>
              {busy
                ? progress!.state === "downloading"
                  ? t("Downloading… {received} of {total} MB", { received: Math.round(progress!.received / 1048576), total: Math.round(progress!.total / 1048576) })
                  : progress!.state === "verifying" ? t("Checking the download against its pinned checksum…") : t("Unpacking…")
                : status.downloadVersion
                  ? t("The app downloads the official {program} program ({version}) from the npm registry and checks it against a checksum pinned in the app, or uses one you have installed yourself.", { program: label, version: status.downloadVersion })
                  : t("The app downloads the official {program} program from the npm registry and checks it against a checksum pinned in the app, or uses one you have installed yourself.", { program: label })}
            </RowDescription>
            {busy ? (
              <div className="mt-2 h-1 overflow-hidden rounded-full bg-border" role="progressbar" aria-valuenow={percent} aria-valuemin={0} aria-valuemax={100}>
                <div className="h-full rounded-[inherit] bg-primary transition-[width] duration-400" style={{ width: `${progress!.state === "downloading" ? percent : 100}%` }} />
              </div>
            ) : null}
          </RowText>
          <RowActions>
            <Status ok={false} label={t("Not installed")} />
            <Button type="button" variant="default" size="small" disabled={Boolean(busy) || !status.downloadVersion} onClick={() => void install()}>
              {busy ? t("Downloading…") : progress?.state === "failed" ? t("Try Again") : megabytes ? t("Download ({size})", { size: megabytes }) : t("Download")}
            </Button>
          </RowActions>
        </SettingsRow>
        {error || progress?.state === "failed" ? <RowError>{error || progress?.error}</RowError> : null}
      </>
    );
  }
  const where =
    status.source === "workbench" ? `${t("The app's copy")}${status.version ? ` · ${status.version}` : ""}`
    : status.source === "system" ? `${t("Your installed {program}", { program: label })}${status.version ? ` · ${status.version}` : ""} · ${status.path}`
    : `${t("Development copy")}${status.version ? ` · ${status.version}` : ""}`;
  return (
    <>
      <SettingsRow>
        <RowText>
          <RowLabel>{t("Program")}</RowLabel>
          <RowDescription title={status.path ?? ""}>{where}</RowDescription>
        </RowText>
        <RowActions>
          <Status ok label={t("Installed")} />
          {status.source === "workbench" ? (
            <Button type="button" variant="outline" size="small" onClick={() => void remove()}>{t("Remove")}</Button>
          ) : null}
        </RowActions>
      </SettingsRow>
      {error ? <RowError>{error}</RowError> : null}
      {props.children}
    </>
  );
}

/**
 * Codex models run on the user's own ChatGPT login, made by the official `codex` program. The runtime starts
 * its `codex login` and relays what it prints: a one-time code to type at OpenAI (no localhost redirect), or
 * the sign-in page it opened in the browser. The app never handles the credential itself.
 */
export function CodexLogin() {
  const [status, setStatus] = useState<CodexStatus | null>(null);
  const [busy, setBusy] = useState<"checking" | "starting" | "signing_out" | null>("checking");
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState("");

  async function run(kind: NonNullable<typeof busy>, action: () => Promise<CodexStatus>, failure: string) {
    setBusy(kind);
    setError("");
    try {
      setStatus(await action());
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : failure);
    } finally {
      setBusy(null);
    }
  }

  useEffect(() => {
    void run("checking", () => desktop.admin.codexStatus(), t("Could not reach the runtime"));
  }, []);

  // While a sign-in waits, follow it until Codex reports signed in or the sign-in ends.
  const signIn = status && !status.loggedIn ? status.signIn : undefined;
  const waiting = signIn?.state === "starting" || signIn?.state === "waiting";
  useEffect(() => {
    if (!waiting) return;
    const timer = window.setInterval(() => {
      void desktop.admin.codexStatus().then(setStatus, () => undefined);
    }, 2500);
    return () => window.clearInterval(timer);
  }, [waiting]);

  async function signOut() {
    if (!await askToConfirm(t("Sign the app out of Codex? Codex models stop working until you sign in again. Your own Codex setup stays signed in."))) return;
    await run("signing_out", () => desktop.admin.codexLogout(), t("Could not sign out"));
  }

  async function openPage() {
    try {
      await desktop.admin.codexOpenPage();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : t("Could not open the page"));
    }
  }

  async function copyCode() {
    if (!signIn?.code) return;
    try {
      await navigator.clipboard.writeText(signIn.code);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    } catch {
      setCopied(false);
    }
  }

  const method = status?.authMethod === "chatgpt" ? t("ChatGPT plan") : status?.authMethod === "api-key" ? t("API key") : "";
  return (
    <>
      <SettingsRow>
        <RowText>
          <RowLabel>{t("Codex Sign-In")}</RowLabel>
          <RowDescription>
            {status === null
              ? busy ? t("Checking…") : t("Status unavailable.")
              : status.loggedIn
                ? [t("Signed in"), method].filter(Boolean).join(" · ")
                : t("Not signed in. You sign in to the official Codex program, which uses your ChatGPT plan; the app never sees your credentials.")}
          </RowDescription>
        </RowText>
        <RowActions>
          {status ? <Status ok={status.loggedIn} label={status.loggedIn ? t("Signed in") : t("Signed out")} /> : null}
          {status?.loggedIn ? (
            <Button type="button" variant="default" size="small" disabled={busy !== null} onClick={() => void signOut()}>
              {busy === "signing_out" ? t("Signing out…") : t("Sign Out")}
            </Button>
          ) : waiting ? (
            <Button type="button" variant="outline" size="small" onClick={() => void run("checking", () => desktop.admin.codexCancelSignIn(), t("Could not cancel"))}>
              {t("Cancel")}
            </Button>
          ) : (
            <Button type="button" variant="default" size="small" disabled={busy !== null || status === null} onClick={() => void run("starting", () => desktop.admin.codexSignIn("device"), t("Could not start sign-in"))}>
              {busy === "starting" ? t("Starting…") : t("Sign In to Codex")}
            </Button>
          )}
        </RowActions>
      </SettingsRow>
      {signIn && waiting ? (
        <SettingsRow stacked>
          {signIn.mode === "device" ? (
            <>
              <RowDescription>{t("Codex shows this one-time code: open the OpenAI page, sign in, and enter it. It expires in 15 minutes; this card updates once you're done.")}</RowDescription>
              <div className={COMMAND_BOX}>
                <code className="min-w-0 flex-1 overflow-x-auto text-[15px] font-semibold tracking-widest whitespace-nowrap">{signIn.code ?? t("Waiting for a code…")}</code>
                {signIn.code ? <Button type="button" variant="outline" size="small" onClick={() => void copyCode()}>{copied ? t("Copied") : t("Copy")}</Button> : null}
                {signIn.url ? (
                  <Button type="button" variant="default" size="small" onClick={() => void openPage()}>{t("Open Page")}</Button>
                ) : null}
              </div>
              <RowDescription>
                {t("Device codes must be allowed in your ChatGPT security settings. If they aren't,")}{" "}
                <LinkButton onClick={() => void run("starting", () => desktop.admin.codexSignIn("browser"), t("Could not start sign-in"))}>
                  {t("sign in with the browser instead")}
                </LinkButton>{t(".")}
              </RowDescription>
            </>
          ) : (
            <RowDescription>
              {t("Finish signing in on the page that opened in your browser. The app keeps Codex listening for it until you're done.")}
              {signIn.url ? <> <LinkButton onClick={() => void openPage()}>{t("Open it again")}</LinkButton>{t(".")}</> : null}
            </RowDescription>
          )}
        </SettingsRow>
      ) : null}
      {error || status?.error || (signIn?.state === "failed" ? signIn.error : "") ? (
        <RowError>{error || status?.error || signIn?.error}</RowError>
      ) : null}
    </>
  );
}

/**
 * Claude Code models run on the user's own Claude login, made once with the official `claude` program in a
 * terminal. The app shows what that program reports, the command to run, and signing out through the same
 * program; it never handles the credential itself.
 */
export function ClaudeCodeLogin() {
  const [status, setStatus] = useState<Awaited<ReturnType<typeof desktop.admin.claudeCodeStatus>> | null>(null);
  const [busy, setBusy] = useState<"checking" | "signing_out" | null>("checking");
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState("");

  async function check() {
    setBusy("checking");
    setError("");
    try {
      setStatus(await desktop.admin.claudeCodeStatus());
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : t("Could not reach the runtime"));
    } finally {
      setBusy(null);
    }
  }

  useEffect(() => {
    void check();
  }, []);

  // While signed out, ask again every few seconds, so finishing the terminal sign-in shows up on its own.
  const signedOut = status !== null && !status.loggedIn && Boolean(status.loginCommand);
  useEffect(() => {
    if (!signedOut) return;
    const timer = window.setInterval(() => {
      void desktop.admin.claudeCodeStatus().then(setStatus, () => undefined);
    }, 4000);
    return () => window.clearInterval(timer);
  }, [signedOut]);

  async function signOut() {
    if (!await askToConfirm(t("Sign the app out of Claude Code? Claude (via Claude Code) models stop working until you sign in again. Your other Claude apps stay signed in."))) return;
    setBusy("signing_out");
    setError("");
    try {
      setStatus(await desktop.admin.claudeCodeLogout());
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : t("Could not sign out"));
    } finally {
      setBusy(null);
    }
  }

  async function copy() {
    if (!status?.loginCommand) return;
    try {
      await navigator.clipboard.writeText(status.loginCommand);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    } catch {
      setCopied(false);
    }
  }

  const plan = status?.subscriptionType ? t("{plan} plan", { plan: `${status.subscriptionType.charAt(0).toUpperCase()}${status.subscriptionType.slice(1)}` }) : "";
  return (
    <>
      <SettingsRow>
        <RowText>
          <RowLabel>{t("Claude Account")}</RowLabel>
          <RowDescription>
            {status === null
              ? busy ? t("Checking…") : t("Status unavailable.")
              : status.loggedIn
                ? [status.email ?? t("Signed in"), plan].filter(Boolean).join(" · ")
                : t("Not signed in. Claude (via Claude Code) models run on your own Claude login; the app never sees your credentials.")}
          </RowDescription>
        </RowText>
        <RowActions>
          {status ? <Status ok={status.loggedIn} label={status.loggedIn ? t("Signed in") : t("Signed out")} /> : null}
          {status?.loggedIn ? (
            <Button type="button" variant="default" size="small" disabled={busy !== null} onClick={() => void signOut()}>
              {busy === "signing_out" ? t("Signing out…") : t("Sign Out")}
            </Button>
          ) : (
            <Button type="button" variant="outline" size="small" disabled={busy !== null} onClick={() => void check()}>
              {busy === "checking" ? t("Checking…") : t("Check Again")}
            </Button>
          )}
        </RowActions>
      </SettingsRow>
      {status && !status.loggedIn && status.loginCommand ? (
        <SettingsRow stacked>
          <RowDescription>
            {t("To sign in, run this in Terminal and keep that window open until it says you're signed in. This card updates on its own.")}
            {t(" If the browser says it can't connect to localhost, that terminal sign-in had already ended: run the command again and use the new browser page.")}
          </RowDescription>
          <div className={COMMAND_BOX}>
            <code className="min-w-0 flex-1 overflow-x-auto text-[11.5px] whitespace-nowrap">{status.loginCommand}</code>
            <Button type="button" variant="outline" size="small" onClick={() => void copy()}>{copied ? t("Copied") : t("Copy")}</Button>
          </div>
        </SettingsRow>
      ) : null}
      {error || status?.error ? <RowError>{error || status?.error}</RowError> : null}
    </>
  );
}
