import { useQuery } from "@tanstack/react-query";
import { Fragment, useMemo, useState } from "react";
import { desktop } from "../../application/desktop";
import { t } from "../../domain/i18n";
import type { Attributes, SpanRecord, TelemetryLog, TraceSummary } from "../../domain/model";
import { Badge } from "../ui/base/Badge.js";
import { Button } from "../ui/base/Button.js";
import { Card } from "../ui/base/Card.js";
import { Checkbox } from "../ui/base/Checkbox.js";
import { classes } from "../ui/base/classes.js";
import { Input } from "../ui/base/Input.js";
import { Notice } from "../ui/base/Notice.js";
import { SubTitle } from "./Parts";
import { formatAgo, formatDuration, formatValue } from "./TelemetryFormat";
import { CHECK, Dot, Fact, HINT, LogRow, Nothing, PAGE, SECTION_LABEL, ShowMore, TRACE_ROW } from "./TelemetryViews";

export function TraceList(props: { traces: TraceSummary[]; onOpen: (traceId: string) => void }) {
  const [query, setQuery] = useState("");
  const [hideRoutine, setHideRoutine] = useState(true);
  const [errorsOnly, setErrorsOnly] = useState(false);
  const routine = props.traces.filter((trace) => trace.routine).length;
  const shown = props.traces.filter((trace) => {
    if (hideRoutine && trace.routine) return false;
    if (errorsOnly && trace.errorCount === 0) return false;
    if (query && !`${trace.rootName} ${trace.traceId}`.toLowerCase().includes(query.toLowerCase())) return false;
    return true;
  });
  const longest = Math.max(1, ...shown.map((trace) => trace.durationMs));
  const [limit, setLimit] = useState(PAGE);

  return (
    <>
      <div className="flex flex-wrap items-center gap-3">
        <Input className="min-w-50 flex-1" aria-label={t("Filter traces")} placeholder={t("Filter by span name or trace id…")} value={query} onChange={(event) => setQuery(event.target.value)} />
        <label className={CHECK}>
          <Checkbox checked={hideRoutine} onCheckedChange={setHideRoutine} />
          {t("Hide polling ({count})", { count: routine })}
        </label>
        <label className={CHECK}>
          <Checkbox checked={errorsOnly} onCheckedChange={setErrorsOnly} />
          {t("Errors only")}
        </label>
      </div>
      {shown.length === 0 ? (
        <Nothing title={t("No traces yet")}>{t("Send a message to an agent, then come back.")}</Nothing>
      ) : (
        <Card className="gap-0 overflow-hidden p-0">
          <div className={classes(TRACE_ROW, "border-t-0 text-[11px] font-semibold tracking-wider text-muted-foreground uppercase")}>
            <span>{t("Trace")}</span>
            <span>{t("Spans")}</span>
            <span>{t("Duration")}</span>
            <span>{t("When")}</span>
          </div>
          {shown.slice(0, limit).map((trace) => (
            <button
              key={trace.traceId}
              type="button"
              className={classes(TRACE_ROW, "cursor-pointer hover:bg-muted")}
              title={`${trace.rootName}\ntrace ${trace.traceId}`}
              onClick={() => props.onOpen(trace.traceId)}
            >
              <span className="flex min-w-0 items-center gap-2">
                {trace.errorCount ? <Dot tone="bad" title={t("{count} failed spans", { count: trace.errorCount })} /> : <Dot tone="good" />}
                <strong className="truncate font-medium">{trace.rootName}</strong>
              </span>
              <span className="flex items-baseline gap-1.5 tabular-nums">
                {trace.spanCount}
                {trace.errorCount ? <span className="text-[11px] text-destructive">{t("{count} err", { count: trace.errorCount })}</span> : null}
              </span>
              <span className="relative flex items-center gap-2 tabular-nums">
                <span className="h-1.5 max-w-[60%] flex-none rounded-[3px] bg-info/55" style={{ width: `${Math.max(2, (trace.durationMs / longest) * 100)}%` }} />
                <span>{formatDuration(trace.durationMs)}</span>
              </span>
              <span className={HINT} title={new Date(trace.startMs).toLocaleString()}>{formatAgo(trace.startMs)}</span>
            </button>
          ))}
          <ShowMore left={shown.length - limit} onMore={() => setLimit((current) => current + PAGE)} />
        </Card>
      )}
    </>
  );
}

type SpanNode = SpanRecord & { depth: number };

/** Orders spans parent-first (children by start time) and records each one's depth for indenting. */
function flatten(spans: SpanRecord[]): SpanNode[] {
  const ids = new Set(spans.map((span) => span.spanId));
  const children = new Map<string, SpanRecord[]>();
  const roots: SpanRecord[] = [];
  for (const span of spans) {
    if (span.parentSpanId && ids.has(span.parentSpanId)) {
      const list = children.get(span.parentSpanId) ?? [];
      list.push(span);
      children.set(span.parentSpanId, list);
    } else {
      roots.push(span);
    }
  }
  const out: SpanNode[] = [];
  const walk = (span: SpanRecord, depth: number) => {
    out.push({ ...span, depth });
    for (const child of (children.get(span.spanId) ?? []).sort((a, b) => a.startMs - b.startMs)) walk(child, depth + 1);
  };
  for (const root of roots.sort((a, b) => a.startMs - b.startMs)) walk(root, 0);
  return out;
}

/** A line of the waterfall: the span's name on the left, where it sits in time on the right. */
const WATERFALL_ROW = "grid grid-cols-[minmax(220px,38%)_minmax(0,1fr)] items-center";


export function TraceDetail(props: { traceId: string; live: boolean; onBack: () => void }) {
  const [open, setOpen] = useState<string | null>(null);
  const query = useQuery({
    queryKey: ["telemetry", "trace", props.traceId],
    queryFn: async () => {
      const [spans, logs] = await Promise.all([
        desktop.telemetry.trace(props.traceId),
        desktop.telemetry.logs({ traceId: props.traceId }),
      ]);
      return { spans, logs };
    },
    refetchInterval: props.live ? 3000 : false,
  });
  const spans: SpanRecord[] = query.data?.spans ?? [];
  const logs: TelemetryLog[] = query.data?.logs ?? [];
  const error = query.error instanceof Error ? query.error.message : query.error ? t("Could not load the trace") : "";

  const nodes = useMemo(() => flatten(spans), [spans]);
  const start = Math.min(...spans.map((span) => span.startMs));
  const end = Math.max(...spans.map((span) => span.startMs + span.durationMs));
  const total = Math.max(end - start, 0.001);
  const root = nodes[0];
  const errors = spans.filter((span) => span.status === "error").length;

  return (
    <>
      <div className="flex flex-wrap items-center gap-3">
        <Button type="button" variant="outline" size="small" onClick={props.onBack}>← {t("All traces")}</Button>
        <div className="flex min-w-0 flex-1 flex-col">
          <strong className="text-sm">{root?.name ?? t("Trace")}</strong>
          <span className={HINT}>
            {formatDuration(total)} · {t("{count} spans", { count: spans.length })}{errors ? ` · ${t("{count} failed", { count: errors })}` : ""}
            {spans.length ? ` · ${new Date(start).toLocaleString()}` : ""}
          </span>
        </div>
        <code className="text-[11px] text-muted-foreground" title={t("Trace id")}>{props.traceId}</code>
      </div>
      {error ? <Notice tone="bad">{error}</Notice> : null}

      <Card className="gap-0.5 overflow-hidden px-0 py-1.5">
        <div className={classes(WATERFALL_ROW, "border-b border-border pr-3.5 pb-1.5")}>
          <span />
          <div className="relative h-4 text-[10.5px] text-muted-foreground tabular-nums [&>span]:absolute [&>span]:top-0 [&>span]:-translate-x-1/2 [&>span]:whitespace-nowrap [&>span:first-child]:translate-x-0 [&>span:last-child]:-translate-x-full">
            {[0, 0.25, 0.5, 0.75, 1].map((step) => (
              <span key={step} style={{ left: `${step * 100}%` }}>{formatDuration(total * step)}</span>
            ))}
          </div>
        </div>
        {nodes.map((span) => {
          const left = ((span.startMs - start) / total) * 100;
          const width = Math.max((span.durationMs / total) * 100, 0.4);
          const selected = open === span.spanId;
          return (
            <Fragment key={span.spanId}>
              <button
                type="button"
                className={classes(WATERFALL_ROW, "w-full cursor-pointer border-0 bg-transparent py-[3px] pr-3.5 text-left text-foreground hover:bg-muted", selected && "bg-muted")}
                onClick={() => setOpen(selected ? null : span.spanId)}
              >
                <span className="flex min-w-0 items-center gap-[7px] text-[12.5px]" style={{ paddingLeft: 8 + span.depth * 14 }}>
                  <Badge tone={span.kind === "server" ? "info" : span.kind === "client" ? "violet" : "neutral"} className="h-auto w-13 flex-none rounded px-0 py-px text-[10px] font-semibold tracking-wide uppercase">{span.kind}</Badge>
                  <span className={classes("truncate", span.status === "error" && "font-medium text-destructive")}>{span.name}</span>
                </span>
                <span className="relative h-5 bg-[linear-gradient(to_right,var(--line)_1px,transparent_1px)] bg-[length:25%_100%]">
                  <span
                    className={classes("absolute top-[5px] h-2.5 rounded-[3px]", span.status === "error" ? "bg-destructive" : span.kind === "server" ? "bg-info" : span.kind === "client" ? "bg-violet" : "bg-muted-foreground")}
                    style={{ left: `${left}%`, width: `${width}%` }}
                  />
                  <span
                    className={classes("absolute text-[10.5px] whitespace-nowrap tabular-nums", left + width > 82 && width > 18 ? "top-1 text-[9.5px] font-semibold text-white" : "top-0.5 text-muted-foreground")}
                    style={
                      left + width <= 82
                        ? { left: `calc(${left + width}% + 6px)` }
                        : width > 18
                          ? { right: `calc(${100 - left - width}% + 6px)` }
                          : { right: `calc(${100 - left}% + 6px)` }
                    }
                  >
                    {formatDuration(span.durationMs)}
                  </span>
                </span>
              </button>
              {selected ? <SpanDetail span={span} traceStart={start} /> : null}
            </Fragment>
          );
        })}
      </Card>

      {logs.length ? (
        <>
          <SubTitle>{t("Logs in this trace")}</SubTitle>
          <Card className="gap-0 px-0 py-1 text-xs">
            {logs.map((line, index) => <LogRow key={`${line.timeMs}-${index}`} line={line} />)}
          </Card>
        </>
      ) : null}
    </>
  );
}

function SpanDetail(props: { span: SpanRecord; traceStart: number }) {
  const { span } = props;
  const attributes = Object.entries(span.attributes).sort(([a], [b]) => a.localeCompare(b));
  return (
    <div className="mx-3.5 mt-0.5 mb-2 flex flex-col gap-3 rounded-lg border border-border bg-background px-3.5 py-3">
      <div className="grid grid-cols-[repeat(auto-fill,minmax(150px,1fr))] gap-2.5">
        <Fact label={t("Duration")} value={formatDuration(span.durationMs)} />
        <Fact label={t("Starts at")} value={`+${formatDuration(span.startMs - props.traceStart)}`} />
        <Fact label={t("Kind")} value={span.kind} />
        <Fact label={t("Status")} value={span.status === "error" ? `error${span.statusMessage ? ` — ${span.statusMessage}` : ""}` : span.status} />
        <Fact label={t("Scope")} value={span.scope} />
        <Fact label={t("Span id")} value={span.spanId} mono />
      </div>
      {attributes.length ? <AttributeTable attributes={span.attributes} /> : <p className={HINT}>{t("No attributes.")}</p>}
      {span.events.length ? (
        <div className="flex flex-col gap-2">
          <div className={SECTION_LABEL}>{t("Events")}</div>
          {span.events.map((event, index) => (
            <div key={`${event.name}-${index}`} className={classes("flex flex-col gap-1 border-l-2 pl-2.5 text-[12.5px]", event.name === "exception" ? "border-destructive" : "border-input")}>
              <div className="flex items-baseline gap-2">
                <strong>{event.name}</strong>
                <span className={HINT}>+{formatDuration(event.timeMs - props.traceStart)}</span>
              </div>
              {event.attributes["exception.message"] ? <p className="m-0 text-destructive">{formatValue(event.attributes["exception.message"])}</p> : null}
              {event.attributes["exception.stacktrace"] ? (
                <details>
                  <summary>{t("Stack trace")}</summary>
                  <pre className="max-h-55 overflow-auto rounded-md bg-muted p-2 text-[11px]">{formatValue(event.attributes["exception.stacktrace"])}</pre>
                </details>
              ) : null}
              {Object.keys(event.attributes).some((key) => !key.startsWith("exception.")) ? (
                <AttributeTable
                  attributes={Object.fromEntries(Object.entries(event.attributes).filter(([key]) => !key.startsWith("exception.")))}
                />
              ) : null}
            </div>
          ))}
        </div>
      ) : null}
    </div>
  );
}

function AttributeTable(props: { attributes: Attributes }) {
  const rows = Object.entries(props.attributes).sort(([a], [b]) => a.localeCompare(b));
  return (
    <div className="grid grid-cols-[minmax(160px,max-content)_minmax(0,1fr)] overflow-hidden rounded-md border border-border text-xs">
      {rows.map(([key, value]) => (
        <Fragment key={key}>
          <code className="border-t border-border bg-muted px-2.5 py-1.25 text-muted-foreground [overflow-wrap:anywhere] first:border-t-0">{key}</code>
          <code className={classes("border-t border-border px-2.5 py-1.25 [overflow-wrap:anywhere] nth-2:border-t-0", typeof value === "number" && "text-info")}>{formatValue(value)}</code>
        </Fragment>
      ))}
    </div>
  );
}

/* ─── Metrics ─── */
