import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { desktop } from "../../application/desktop";
import { t } from "../../domain/i18n";
import { Badge } from "../ui/base/Badge.js";
import { Button } from "../ui/base/Button.js";
import { classes } from "../ui/base/classes.js";

type Which = "accessibility" | "screenRecording";

const ROWS: { which: Which; label: string; detail: string }[] = [
  { which: "accessibility", label: t("Accessibility"), detail: t("Lets agents click, type and read what is on screen.") },
  { which: "screenRecording", label: t("Screen Recording"), detail: t("Lets agents take screenshots to see what they are doing.") },
];

/**
 * Asks macOS for a permission from the app itself, so macOS shows its prompt and lists the app in System Settings;
 * in a plain browser (no desktop bridge) it opens the System Settings pane instead.
 */
export async function requestPermission(which: Which): Promise<void> {
  const request = (globalThis as { workbenchDesktop?: { requestPermission?: (which: Which) => Promise<{ granted: boolean }> } }).workbenchDesktop?.requestPermission;
  const answer = request ? await request(which).catch(() => ({ granted: false })) : { granted: false };
  if (!answer.granted) await desktop.admin.openPermissionPane(which).catch(() => undefined);
}

/** The computer-use permissions with their state, a Request button each, and a re-check while one is pending. */
export function ComputerPermissions(props: { compact?: boolean }) {
  const [asked, setAsked] = useState(false);
  // After a request the answer arrives in System Settings, not here: look again every few seconds.
  const { data: status = null } = useQuery({
    queryKey: ["admin", "computerPermissions"],
    queryFn: () => desktop.admin.computerPermissions(),
    refetchInterval: asked ? 3000 : false,
  });

  if (status && !status.required) {
    return <p className="text-[12.5px] text-muted-foreground">{status.platform === "win32" ? t("Nothing to grant on Windows: agents can take screenshots and control apps already.") : t("Nothing to grant on this system: agents can take screenshots and control apps already.")}</p>;
  }
  return (
    <ul className={classes("m-0 flex list-none flex-col p-0", props.compact && "gap-2")}>
      {ROWS.map((row) => {
        const granted = status?.[row.which] ?? false;
        return (
          <li
            key={row.which}
            className={classes(
              "grid grid-cols-[1fr_auto] items-center gap-x-3 gap-y-0.5",
              props.compact ? "rounded-[10px] border border-border bg-card px-3.5 py-2.5" : "px-4 py-3 not-first:border-t not-first:border-border",
            )}
          >
            <strong className={classes("col-start-1 text-[13px]", !props.compact && "font-medium")}>{row.label}</strong>
            <span className="col-start-1 row-start-2 text-xs text-muted-foreground">{row.detail}</span>
            {granted ? (
              <Badge tone="good" className="col-start-2 row-span-2 row-start-1 text-[13px] font-semibold">✓ {t("allowed")}</Badge>
            ) : (
              <span className="col-start-2 row-span-2 row-start-1 inline-flex flex-wrap justify-end gap-1.5">
                <Button type="button" variant="outline" size="small" onClick={() => { setAsked(true); void requestPermission(row.which); }}>{t("Request")}</Button>
                <Button type="button" variant="outline" size="small" onClick={() => void desktop.admin.openPermissionPane(row.which)}>{t("Open System Settings")}</Button>
              </span>
            )}
          </li>
        );
      })}
    </ul>
  );
}
