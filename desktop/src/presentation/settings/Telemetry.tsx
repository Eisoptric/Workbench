import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { desktop } from "../../application/desktop";
import type { TelemetryExportFormat } from "../../application/telemetry";
import { t } from "../../domain/i18n";
import { Button } from "../ui/base/Button.js";
import { Menu, MenuContent, MenuItem, MenuTrigger } from "../ui/base/Menu.js";
import { Notice } from "../ui/base/Notice.js";
import { Tabs, TabsList, TabsTrigger } from "../ui/base/Tabs.js";
import { formatDuration, homeRelative } from "./TelemetryFormat";
import { TraceDetail, TraceList } from "./TelemetryTraces";
import { Dot, HINT, Logs, Metrics, Network } from "./TelemetryViews";

export { formatDuration };

type Tab = "traces" | "metrics" | "logs" | "network";

const EXPORTS: { format: TelemetryExportFormat; label: string; detail: string }[] = [
  { format: "workbench", label: "The app JSON", detail: t("telemetry.json as saved, for this page") },
  { format: "otlp", label: "OTLP JSON", detail: t("JSON Lines for the OpenTelemetry Collector (otlpjsonfile)") },
  { format: "siem", label: "SIEM · ECS NDJSON", detail: t("One event per line for Elastic, Splunk, Sentinel") },
];

function formatTelemetryError(reason: unknown): string {
  const message = reason instanceof Error ? reason.message : "";
  return /not found|cannot get/i.test(message) || message.startsWith("<")
    ? t("This runtime does not serve telemetry yet. Restart it to load the new version.")
    : message || t("Could not reach the runtime");
}

export function Telemetry() {
  const queryClient = useQueryClient();
  const [tab, setTab] = useState<Tab>("traces");
  const [openTrace, setOpenTrace] = useState<string | null>(null);
  const [live, setLive] = useState(true);
  const [actionError, setActionError] = useState("");
  const [downloading, setDownloading] = useState(false);

  const refetchInterval = live ? 3000 : false;
  const statusQuery = useQuery({ queryKey: ["telemetry", "status"], queryFn: () => desktop.telemetry.status(), refetchInterval });
  const tracesQuery = useQuery({ queryKey: ["telemetry", "traces"], queryFn: () => desktop.telemetry.traces(), enabled: tab === "traces" && !openTrace, refetchInterval });
  const metricsQuery = useQuery({ queryKey: ["telemetry", "metrics"], queryFn: () => desktop.telemetry.metrics(), enabled: tab === "metrics", refetchInterval });
  const logsQuery = useQuery({ queryKey: ["telemetry", "logs"], queryFn: () => desktop.telemetry.logs(), enabled: tab === "logs", refetchInterval });
  const networkQuery = useQuery({ queryKey: ["telemetry", "network"], queryFn: () => desktop.telemetry.network(), enabled: tab === "network", refetchInterval });

  const status = statusQuery.data ?? null;
  const traces = tracesQuery.data ?? [];
  const metrics = metricsQuery.data ?? [];
  const logs = logsQuery.data ?? [];
  const network = networkQuery.data ?? null;
  const activeError = statusQuery.error
    ?? (tab === "traces" && !openTrace ? tracesQuery.error : null)
    ?? (tab === "metrics" ? metricsQuery.error : null)
    ?? (tab === "logs" ? logsQuery.error : null)
    ?? (tab === "network" ? networkQuery.error : null);
  const error = actionError || (activeError ? formatTelemetryError(activeError) : "");

  function refresh() {
    setActionError("");
    void queryClient.invalidateQueries({ queryKey: ["telemetry"] });
  }

  async function download(format: TelemetryExportFormat) {
    setDownloading(true);
    try {
      const { blob, filename } = await desktop.telemetry.export(format);
      const url = URL.createObjectURL(blob);
      const link = Object.assign(window.document.createElement("a"), { href: url, download: filename });
      link.click();
      window.setTimeout(() => URL.revokeObjectURL(url), 1000);
      refresh();
    } catch (reason) {
      setActionError(reason instanceof Error ? reason.message : t("Could not download telemetry"));
    } finally {
      setDownloading(false);
    }
  }

  async function clear() {
    try {
      await desktop.telemetry.clear();
      setOpenTrace(null);
      queryClient.setQueryData(["telemetry", "traces"], []);
      queryClient.setQueryData(["telemetry", "logs"], []);
      refresh();
    } catch (reason) {
      setActionError(reason instanceof Error ? reason.message : t("Could not clear telemetry"));
    }
  }

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center justify-between gap-2.5">
        <Tabs value={tab} onValueChange={(id) => setTab(id as Tab)}>
          <TabsList>
            {(["traces", "metrics", "logs", "network"] as Tab[]).map((id) => (
              <TabsTrigger key={id} value={id} className="px-3.5">
                {id === "traces" ? t("Traces") : id === "metrics" ? t("Metrics") : id === "logs" ? t("Logs") : t("Network")}
              </TabsTrigger>
            ))}
          </TabsList>
        </Tabs>
        <div className="flex items-center gap-1.5">
          <Button
            type="button"
            variant={live ? "soft" : "outline"}
            size="small"
            onClick={() => setLive((value) => !value)}
            title={t("Refresh every 3 seconds")}
          >
            <Dot tone={live ? "good" : "idle"} pulse={live} />
            {t("Live")}
          </Button>
          <Button type="button" variant="outline" size="small" onClick={refresh}>{t("Refresh")}</Button>
          <DownloadMenu
            disabled={!status || status.mode === "disabled"}
            busy={downloading}
            savedTo={status?.file ? homeRelative(status.file) : null}
            onPick={(format) => void download(format)}
          />
          <Button type="button" variant="outline" size="small" onClick={() => void clear()} title={t("Empties the page and telemetry.json")}>{t("Clear")}</Button>
        </div>
      </div>

      {status?.mode === "disabled" ? (
        <p className={HINT}>{t("Telemetry is off (OTEL_SDK_DISABLED=true), so nothing is recorded or saved.")}</p>
      ) : null}

      {error ? <Notice tone="bad">{error}</Notice> : null}

      {tab === "traces" ? (
        openTrace ? (
          <TraceDetail traceId={openTrace} live={live} onBack={() => setOpenTrace(null)} />
        ) : (
          <TraceList traces={traces} onOpen={setOpenTrace} />
        )
      ) : null}
      {tab === "metrics" ? <Metrics metrics={metrics} /> : null}
      {tab === "network" ? <Network view={network} /> : null}
      {tab === "logs" ? (
        <Logs
          logs={logs}
          onOpenTrace={(traceId) => {
            setTab("traces");
            setOpenTrace(traceId);
          }}
        />
      ) : null}
    </div>
  );
}

function DownloadMenu(props: {
  disabled: boolean;
  busy: boolean;
  savedTo: string | null;
  onPick: (format: TelemetryExportFormat) => void;
}) {
  return (
    <Menu>
      <MenuTrigger
        disabled={props.disabled || props.busy}
        title={props.savedTo ? t("Saved to {path}", { path: props.savedTo }) : undefined}
        render={<Button type="button" variant="outline" size="small" />}
      >
        {props.busy ? t("Saving…") : t("Download")}
        <svg width="10" height="10" viewBox="0 0 16 16" fill="none" aria-hidden="true">
          <path d="M4 6l4 4 4-4" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
      </MenuTrigger>
      <MenuContent className="w-72">
        {EXPORTS.map((item) => (
          <MenuItem key={item.format} className="flex-col items-start gap-px px-2.5 py-1.75" onClick={() => props.onPick(item.format)}>
            <strong className="text-[12.5px] font-medium">{item.label}</strong>
            <span className="text-[11.5px] text-muted-foreground">{item.detail}</span>
          </MenuItem>
        ))}
        {props.savedTo ? <div className="mt-1 border-t border-border px-2.5 pt-1.5 pb-1 text-[11px] text-muted-foreground">{t("Saved to {path}", { path: props.savedTo })}</div> : null}
      </MenuContent>
    </Menu>
  );
}

/* ─── Traces ─── */

