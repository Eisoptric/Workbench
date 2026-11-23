import { Fragment, useState, type ReactNode } from "react";
import type { NetworkView } from "../../application/telemetry";
import { t } from "../../domain/i18n";
import type { MetricPoint, MetricView, TelemetryLog } from "../../domain/model";
import { Badge } from "../ui/base/Badge.js";
import { Button } from "../ui/base/Button.js";
import { Card } from "../ui/base/Card.js";
import { classes } from "../ui/base/classes.js";
import { Choice } from "../ui/base/Field.js";
import { Input } from "../ui/base/Input.js";
import { SubTitle } from "./Parts";
import { formatClock, formatNumber, formatUnit, formatValue, shortId } from "./TelemetryFormat";

export const SEVERITY_FLOORS: { label: string; value: number }[] = [
  { label: t("All levels"), value: 0 },
  { label: t("Info and up"), value: 9 },
  { label: t("Warnings and up"), value: 13 },
  { label: t("Errors"), value: 17 },
];

/** How a line of a table is set; the traces list and the waterfall share it. */
export const TRACE_ROW = "grid min-h-[34px] w-full grid-cols-[minmax(120px,1fr)_48px_minmax(84px,180px)_64px] items-center gap-3.5 border-0 border-t border-border bg-transparent px-3.5 py-1.25 text-left text-[12.5px] text-foreground";
export const HINT = "text-[12.5px] text-muted-foreground";
export const SECTION_LABEL = "text-[10.5px] font-semibold tracking-wider text-muted-foreground uppercase";
export const TABLE = "w-full border-collapse text-xs [&_code]:text-[11.5px] [&_code]:break-all [&_td]:border-t [&_td]:border-border [&_td]:px-2 [&_td]:py-1.25 [&_td]:text-left [&_th]:border-t [&_th]:border-border [&_th]:px-2 [&_th]:py-1.25 [&_th]:text-left [&_th]:text-[11px] [&_th]:font-semibold [&_th]:text-muted-foreground";

/** A small round mark that says whether something went well. */
export function Dot(props: { tone: "good" | "bad" | "idle"; pulse?: boolean; title?: string }) {
  return <span className={classes("inline-block size-[7px] shrink-0 rounded-full", props.tone === "good" ? "bg-good" : props.tone === "bad" ? "bg-destructive" : "bg-muted-foreground", props.pulse && "animate-pulse")} title={props.title} />;
}

/** A short word that labels a record, such as the service it came from. */
export function Tag(props: { children: ReactNode }) {
  return <Badge className="h-auto border border-border px-1.75 py-px text-[11px]">{props.children}</Badge>;
}

/** What a list shows when there is nothing in it yet, and what to do about it. */
export function Nothing(props: { title: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-1 rounded-[10px] border border-dashed border-input px-4 py-10 text-center">
      <strong>{props.title}</strong>
      <p className={classes("m-0", HINT)}>{props.children}</p>
    </div>
  );
}

export const CHECK = "inline-flex items-center gap-1.5 text-[12.5px] whitespace-nowrap text-muted-foreground";
export const PAGE = 25;

export function ShowMore(props: { left: number; onMore: () => void }) {
  if (props.left <= 0) return null;
  return (
    <Button type="button" variant="outline" size="small" className="mx-auto my-2.5 self-center" onClick={props.onMore}>
      {t("Show {count} more ({left} left)", { count: Math.min(PAGE, props.left), left: props.left })}
    </Button>
  );
}

export function Fact(props: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="flex min-w-0 flex-col gap-0.5">
      <span className="text-[11px] text-muted-foreground">{props.label}</span>
      {props.mono ? <code className="text-[12.5px] [overflow-wrap:anywhere]">{props.value}</code> : <strong className="text-[12.5px] [overflow-wrap:anywhere]">{props.value}</strong>}
    </div>
  );
}

const GROUPS: { prefix: string; title: string }[] = [
  { prefix: "gen_ai.", title: "LLM" },
  { prefix: "workbench.agent", title: t("Agents") },
  { prefix: "workbench.tool", title: t("Tools") },
  { prefix: "eisoptric.mcp", title: "MCP" },
  { prefix: "http.", title: "HTTP" },
  { prefix: "", title: t("Runtime") },
];

export function Metrics(props: { metrics: MetricView[] }) {
  if (props.metrics.length === 0) {
    return (
      <Nothing title={t("No metrics recorded yet")}>{t("Metrics appear once the runtime has handled requests or runs.")}</Nothing>
    );
  }
  const used = new Set<string>();
  const sections = GROUPS.map((group) => {
    const items = props.metrics.filter((metric) => !used.has(metric.name) && metric.name.startsWith(group.prefix));
    for (const item of items) used.add(item.name);
    return { ...group, items };
  }).filter((group) => group.items.length);

  return (
    <>
      {sections.map((section) => (
        <Fragment key={section.title}>
          <SubTitle>{section.title}</SubTitle>
          {section.items.map((metric) => <MetricCard key={metric.name} metric={metric} />)}
        </Fragment>
      ))}
    </>
  );
}

function MetricCard(props: { metric: MetricView }) {
  const { metric } = props;
  const total = metric.type === "histogram"
    ? metric.points.reduce((sum, point) => sum + (point.histogram?.count ?? 0), 0)
    : metric.type === "sum"
      ? metric.points.reduce((sum, point) => sum + (point.value ?? 0), 0)
      : null;
  return (
    <Card className="gap-2.5">
      <div className="flex items-start justify-between gap-3">
        <div>
          <code className="text-[13px] font-semibold text-foreground">{metric.name}</code>
          {metric.description ? <p className={classes("mt-0.5", HINT)}>{metric.description}</p> : null}
        </div>
        <div className="flex flex-none items-center gap-1.5">
          <Tag>{metric.type === "sum" ? (metric.monotonic ? t("counter") : t("up-down")) : metric.type}</Tag>
          {metric.unit ? <Tag>{metric.unit}</Tag> : null}
          {total !== null ? (
            <strong className="ml-1 text-[13px] tabular-nums">
              {metric.type === "histogram" ? t("{count} recorded", { count: formatNumber(total) }) : formatUnit(total, metric.unit)}
            </strong>
          ) : null}
        </div>
      </div>
      <div className="flex flex-col border-t border-border">
        {metric.type === "histogram" ? (
          <div className={classes(POINT, HISTOGRAM_POINT, "text-[11px] font-semibold tracking-wider text-muted-foreground uppercase")}>
            <span>{t("Count")}</span>
            <span>{t("Avg")}</span>
            <span>{t("Min / Max")}</span>
            <span>{t("Distribution")}</span>
          </div>
        ) : null}
        {metric.points.map((point, index) => (
          <MetricRow key={index} point={point} metric={metric} />
        ))}
      </div>
    </Card>
  );
}

/** A line of a metric: what it was measured on, and the figure. A histogram's has more columns. */
const POINT = "grid grid-cols-[minmax(0,1fr)_auto] items-center gap-3.5 border-b border-border py-1.5 text-[12.5px] tabular-nums last:border-b-0";
const HISTOGRAM_POINT = "grid-cols-[minmax(50px,0.7fr)_minmax(70px,1fr)_minmax(110px,1.4fr)_minmax(80px,1.2fr)] gap-y-1";

function MetricRow(props: { point: MetricPoint; metric: MetricView }) {
  const { point, metric } = props;
  const chips = Object.entries(point.attributes);
  const labels = chips.length ? (
    <span className="flex min-w-0 flex-wrap gap-1">
      {chips.map(([key, value]) => (
        <span key={key} className="inline-flex max-w-full gap-1.25 overflow-hidden rounded-[5px] bg-muted px-1.75 py-px font-mono text-[11px] text-ellipsis whitespace-nowrap" title={key}>
          <em className="text-muted-foreground not-italic">{key.split(".").pop()}</em>
          {formatValue(value)}
        </span>
      ))}
    </span>
  ) : (
    <span className={HINT}>{t("(no attributes)")}</span>
  );
  if (point.histogram) {
    const { count, sum, min, max, counts } = point.histogram;
    const peak = Math.max(1, ...counts);
    return (
      <div className={classes(POINT, HISTOGRAM_POINT, "[&>:first-child]:col-span-full")}>
        {labels}
        <strong>{formatNumber(count)}</strong>
        <span>{count ? formatUnit(sum / count, metric.unit) : "—"}</span>
        <span className={HINT}>
          {min !== undefined ? formatUnit(min, metric.unit) : "—"} / {max !== undefined ? formatUnit(max, metric.unit) : "—"}
        </span>
        <span className="flex h-5.5 items-end gap-px [&>span]:min-w-0.5 [&>span]:flex-1 [&>span]:rounded-t-[1px] [&>span]:bg-info/70" title={t("Bucket counts, low to high")}>
          {counts.map((value, index) => (
            <span key={index} style={{ height: `${value ? Math.max(12, (value / peak) * 100) : 0}%` }} />
          ))}
        </span>
      </div>
    );
  }
  return (
    <div className={POINT}>
      {labels}
      <strong>{formatUnit(point.value ?? 0, metric.unit)}</strong>
    </div>
  );
}

/* ─── Logs ─── */

export function Logs(props: { logs: TelemetryLog[]; onOpenTrace: (traceId: string) => void }) {
  const [limit, setLimit] = useState(PAGE);
  const [floor, setFloor] = useState(0);
  const [query, setQuery] = useState("");
  const shown = props.logs
    .filter((line) => line.severityNumber >= floor)
    .filter((line) => !query || line.body.toLowerCase().includes(query.toLowerCase()))
    .slice()
    .reverse();
  return (
    <>
      <div className="flex flex-wrap items-center gap-3">
        <Input className="min-w-50 flex-1" aria-label={t("Search log messages")} placeholder={t("Search log messages…")} value={query} onChange={(event) => setQuery(event.target.value)} />
        <Choice ariaLabel={t("Minimum severity")} value={String(floor)} options={SEVERITY_FLOORS.map((item) => ({ value: String(item.value), label: item.label }))} onChange={(next) => setFloor(Number(next))} />
      </div>
      {shown.length === 0 ? (
        <Nothing title={t("No log records")}>{t("Runtime log lines show up here as OpenTelemetry log records.")}</Nothing>
      ) : (
        <Card className="gap-0 px-0 py-1 text-xs">
          {shown.slice(0, limit).map((line, index) => (
            <LogRow key={`${line.timeMs}-${index}`} line={line} onOpenTrace={props.onOpenTrace} />
          ))}
          <ShowMore left={shown.length - limit} onMore={() => setLimit((current) => current + PAGE)} />
        </Card>
      )}
    </>
  );
}

/** How loud a log record is, as the tone of its label. */
const SEVERITY_TONES = { error: "bad", warn: "wait", info: "info", debug: "neutral" } as const;

function severityClass(severity: number): keyof typeof SEVERITY_TONES {
  if (severity >= 17) return "error";
  if (severity >= 13) return "warn";
  if (severity >= 9) return "info";
  return "debug";
}

export function LogRow(props: { line: TelemetryLog; onOpenTrace?: (traceId: string) => void }) {
  const { line } = props;
  const service = line.attributes["workbench.service"];
  return (
    <div className="grid grid-cols-[96px_58px_max-content_minmax(0,1fr)_max-content] items-baseline gap-2.5 border-b border-border px-3.5 py-1.25 last:border-b-0">
      <span className="font-mono text-[11px] text-muted-foreground">{formatClock(line.timeMs)}</span>
      <Badge tone={SEVERITY_TONES[severityClass(line.severityNumber)]} className="h-auto w-full rounded px-0 py-px text-[10px] font-semibold tracking-wide">{line.severity}</Badge>
      {service ? <Tag>{formatValue(service)}</Tag> : <span />}
      <span className="[overflow-wrap:anywhere]">{line.body}</span>
      {line.traceId && props.onOpenTrace ? (
        <button type="button" className="cursor-pointer border-0 bg-transparent p-0 font-mono text-[11px] text-info hover:underline" onClick={() => props.onOpenTrace?.(line.traceId!)} title={line.traceId}>
          {t("trace {id}", { id: shortId(line.traceId) })}
        </button>
      ) : <span />}
    </div>
  );
}

/**
 * Every host the runtime has sent a request to since it started, and the latest requests: the model providers and
 * tools you use, and nothing else. The same records are appended to network.log for `tail -f` or your own tools.
 */
export function Network(props: { view: NetworkView | null }) {
  const view = props.view;
  if (!view) return <p className={HINT}>{t("Loading…")}</p>;
  const remote = view.hosts.filter((host) => !host.local);
  return (
    <div className="flex flex-col gap-3">
      <p className={HINT}>
        {t("Every HTTP(S) request the app's runtime has sent since {time}: hosts and paths only, never query strings or bodies. Claude Code, Codex and git run as their own programs and are not listed here; watch them with your system's firewall or network monitor.", { time: new Date(view.since).toLocaleString() })}
        {view.file ? <> {t("Also written to:")} <code>{view.file}</code></> : null}
      </p>
      <Card className="gap-0 overflow-hidden p-0">
        <div className="flex items-baseline gap-2.5 p-2">
          <strong>{remote.length === 1 ? t("1 outside host") : t("{count} outside hosts", { count: remote.length })}</strong>
          <span className={HINT}>{t("{count} on this computer", { count: view.hosts.length - remote.length })}</span>
        </div>
        {view.hosts.length === 0 ? <p className={classes("px-2 pb-2", HINT)}>{t("No requests yet.")}</p> : null}
        <table className={TABLE}>
          <thead>
            <tr><th>{t("Host")}</th><th>{t("Where")}</th><th>{t("Requests")}</th><th>{t("Methods")}</th><th>{t("Last")}</th></tr>
          </thead>
          <tbody>
            {view.hosts.map((host) => (
              <tr key={host.host}>
                <td><code>{host.host}</code></td>
                <td>{host.local ? t("this computer") : t("internet")}</td>
                <td>{host.requests}</td>
                <td>{host.methods.join(", ")}</td>
                <td>{new Date(host.last).toLocaleTimeString()}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </Card>
      <Card className="gap-0 overflow-hidden p-0">
        <div className="flex items-baseline gap-2.5 p-2"><strong>{t("Latest requests")}</strong></div>
        <table className={TABLE}>
          <tbody>
            {view.recent.slice(0, 100).map((entry, index) => (
              <tr key={`${entry.at}-${index}`}>
                <td>{new Date(entry.at).toLocaleTimeString()}</td>
                <td>{entry.method}</td>
                <td><code>{entry.host}{entry.path}</code></td>
              </tr>
            ))}
          </tbody>
        </table>
      </Card>
    </div>
  );
}

