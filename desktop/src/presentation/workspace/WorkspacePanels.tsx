import { type Finding, type FindingKind, type FrozenVersion, type ExperimentRun, type SampleItem, byAssistant, runStatusLabel, sampleIdParts, sampleText, scoreText } from "../../domain/research";
import { shortcut, t } from "../../domain/i18n";
import { Button } from "../ui/base/Button.js";
import { Card } from "../ui/base/Card.js";
import { classes } from "../ui/base/classes.js";
import { Choice } from "../ui/base/Field.js";
import { Textarea } from "../ui/base/Textarea.js";

const FINDING_LABELS: Record<FindingKind, string> = {
  observation: t("Observation"),
  ai_suggestion: t("AI suggestion"),
  conclusion: t("Conclusion"),
};
const SAMPLE_ENTRY = "relative flex h-7 w-full flex-none cursor-pointer items-center gap-2 rounded-md border-0 bg-transparent px-2.5 text-left text-xs before:absolute before:inset-y-1.5 before:left-0 before:w-0.5 before:rounded-full";
const SAMPLE_LABEL = "mb-1 text-[11px] font-medium text-muted-foreground";
const SAMPLE_TEXT = "m-0 rounded-md border border-border bg-panel px-3 py-2 font-mono text-xs leading-relaxed whitespace-pre-wrap [overflow-wrap:anywhere]";
const OUTPUT_TEXT = "font-mono text-xs whitespace-pre-wrap [overflow-wrap:anywhere]";
const MUTED = "text-xs text-muted-foreground";
const WARNING = "text-xs text-wait";

/** Summary of the active experiment run shown in the Run bottom panel. */
export function WorkspaceRunSummary(props: {
  run: ExperimentRun | null;
  runVersion: FrozenVersion | null;
  sampleCount: number;
  errorOutput: string;
}) {
  const { run, runVersion, sampleCount, errorOutput } = props;
  if (!run) {
    return <p className={MUTED}>{t("Enter a command above and run it. What it writes appears here.")}</p>;
  }
  return (
    <div className="flex flex-col gap-1">
      <div className="font-semibold">
        {runStatusLabel(run.status)}
        {runVersion ? <span className="font-normal text-muted-foreground"> · v{runVersion.number}</span> : null}
      </div>
      <div className={MUTED}>
        {t("{count} samples", { count: run.samples ?? sampleCount })}
        {run.infrastructureErrors ? t(", {count} infrastructure errors", { count: run.infrastructureErrors }) : ""}
        {run.exitCode !== undefined && run.exitCode !== null ? t(", exit code {code}", { code: run.exitCode }) : ""}
      </div>
      {run.inputsChanged?.length ? <div className={WARNING}>{t("Changed during the run: {inputs}", { inputs: run.inputsChanged.join(", ") })}</div> : null}
      {run.sealIntact === false ? <div className={WARNING}>{t("The samples changed after the run was sealed. They no longer stand for its results.")}</div> : null}
      {run.warnings?.map((warning) => <div key={warning} className={WARNING}>{warning}</div>)}
      {run.status === "interrupted" && run.process === "unconfirmed" ? <div className={WARNING}>{t("Its process could not be confirmed, so it was not ended.")}</div> : null}
      {run.error ? <div className={WARNING}>{run.error}</div> : null}
      {run.problems?.slice(0, 5).map((item) => (
        <div key={item.line} className={WARNING}>
          {t("Line {line}: {reason}", { line: item.line, reason: item.reason })}
        </div>
      ))}
      {errorOutput ? <pre className={classes(OUTPUT_TEXT, "text-destructive")}>{errorOutput}</pre> : null}
    </div>
  );
}

/** Master-detail sample list shown in the Samples bottom panel. */
export function WorkspaceSamplesPanel(props: {
  run: ExperimentRun | null;
  samples: SampleItem[];
  selected: SampleItem | null;
  selectedOffset: number | null;
  nextCursor: number | null;
  onSelect: (offset: number) => void;
  onMore: () => void;
}) {
  const { run, samples, selected, selectedOffset, nextCursor, onSelect, onMore } = props;
  return (
    <div className="flex h-full min-h-0">
      <div
        className="flex w-[230px] max-w-[45%] min-w-[150px] flex-none flex-col gap-px overflow-y-auto border-r border-border px-1.5 py-2"
        role="listbox"
        aria-label={t("Samples")}
        tabIndex={0}
        onKeyDown={(event) => {
          // Up and down go through the samples, as in a file list.
          if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
          event.preventDefault();
          const at = samples.findIndex((item) => item.offset === selectedOffset);
          const next = samples[Math.min(samples.length - 1, Math.max(0, at + (event.key === "ArrowDown" ? 1 : -1)))];
          if (next) onSelect(next.offset);
        }}
      >
        <div className="px-2 pb-1 text-[11px] text-muted-foreground">{t("{count} samples", { count: run?.samples ?? samples.length })}</div>
        {samples.map((item) => {
          const id = item.sample ? sampleIdParts(item.sample.sampleId) : null;
          const chosen = item.offset === selectedOffset;
          return (
            <button
              type="button"
              role="option"
              aria-selected={chosen}
              tabIndex={-1}
              key={item.offset}
              ref={chosen ? (element) => element?.scrollIntoView({ block: "nearest" }) : undefined}
              className={classes(SAMPLE_ENTRY, chosen ? "bg-muted text-foreground before:bg-primary" : "text-muted-foreground hover:bg-muted/60 hover:text-foreground")}
              title={item.sample?.sampleId}
              onClick={() => onSelect(item.offset)}
            >
              {item.sample?.error || !item.sample ? <span className="size-1.5 flex-none rounded-full bg-wait" aria-hidden="true" /> : null}
              {id ? (
                <span className="flex min-w-0 flex-1 font-mono whitespace-nowrap">
                  <span className="min-w-0 flex-[0_1_auto] truncate">{id.start}</span>
                  <span className="flex-none">{id.end}</span>
                </span>
              ) : <span className={classes(WARNING, "truncate")}>{t("Problem at byte {offset}", { offset: item.offset })}</span>}
              {item.sample?.error ? <span className="sr-only">{t("{kind} error", { kind: item.sample.error.kind })}</span> : null}
            </button>
          );
        })}
        {nextCursor !== null ? (
          <Button variant="ghost" size="small" className="self-start text-info" onClick={onMore}>
            {t("More")}
          </Button>
        ) : null}
        {samples.length === 0 ? <p className={classes(MUTED, "px-2")}>{t("No samples yet.")}</p> : null}
      </div>
      <div className="min-w-0 flex-1 overflow-y-auto px-4 py-3">
        {selected ? <SampleDetail item={selected} /> : samples.length ? <p className={MUTED}>{t("Choose a sample to see it here.")}</p> : null}
      </div>
    </div>
  );
}

/** Findings capture form and saved list shown in the Findings bottom panel. */
export function WorkspaceFindingsPanel(props: {
  findings: Finding[];
  findingText: string;
  findingKind: FindingKind;
  onTextChange: (text: string) => void;
  onKindChange: (kind: FindingKind) => void;
  onAdd: () => void;
}) {
  const { findings, findingText, findingKind, onTextChange, onKindChange, onAdd } = props;
  return (
    <div className="flex max-w-3xl flex-col gap-2.5">
      {/* What was understood takes the width; its kind and the save button sit under it, as one card. */}
      <form
        className="flex flex-col gap-2 rounded-lg border border-border bg-card p-2.5"
        onSubmit={(event) => {
          event.preventDefault();
          onAdd();
        }}
      >
        <Textarea
          aria-label={t("What you understood")}
          placeholder={t("What you understood from this run: a claim, a doubt or what to try next")}
          className="min-h-16 resize-y"
          rows={3}
          value={findingText}
          onChange={(event) => onTextChange(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter" && (event.metaKey || event.ctrlKey) && findingText.trim()) {
              event.preventDefault();
              onAdd();
            }
          }}
        />
        <div className="flex flex-wrap items-center gap-2">
          <Choice ariaLabel={t("Kind")} className="w-36" value={findingKind} options={(Object.keys(FINDING_LABELS) as FindingKind[]).map((kind) => ({ value: kind, label: FINDING_LABELS[kind] }))} onChange={(next) => onKindChange(next as FindingKind)} />
          <span className={classes(MUTED, "ml-auto")}>{t("{shortcut} to save", { shortcut: shortcut("⌘↵") })}</span>
          <Button type="submit" disabled={!findingText.trim()}>
            {t("Save finding")}
          </Button>
        </div>
      </form>
      {findings.length === 0 ? <p className={classes(MUTED, "px-0.5")}>{t("No findings yet. What you save here is kept with the project's research records, beside the runs it is about.")}</p> : null}
      {[...findings].reverse().map((finding) => (
        <Card key={finding.id} className="gap-1 px-3.5 py-3">
          <div className={MUTED}>
            {FINDING_LABELS[finding.kind]}
            {byAssistant(finding) ? ` · ${t("by the assistant")}` : ""}
            {finding.runIds?.length ? ` · ${finding.runIds[0]}` : ""}
          </div>
          <div>{finding.text}</div>
        </Card>
      ))}
    </div>
  );
}

/** A sample's scores as a table of name and value, in the order the program wrote them; a zero or false is dimmed, so what
 * the scorer did find stands out. */
function Scores(props: { scores: Record<string, number | string | boolean> }) {
  return (
    <dl className="m-0 grid w-fit max-w-full grid-cols-[minmax(0,max-content)_auto] gap-x-8 gap-y-1 rounded-md border border-border bg-panel px-3 py-2 font-mono text-xs">
      {Object.entries(props.scores).map(([name, score]) => (
        <div key={name} className="contents">
          <dt className="min-w-0 text-muted-foreground [overflow-wrap:anywhere]">{name}</dt>
          <dd className={classes("m-0 text-right tabular-nums", score === 0 || score === false ? "text-muted-foreground" : "font-semibold")}>{typeof score === "number" ? scoreText(score) : String(score)}</dd>
        </div>
      ))}
    </dl>
  );
}

/** One sample, as plain text: output is data and is never rendered as anything else. */
export function SampleDetail(props: { item: SampleItem }) {
  const { sample, problem, raw } = props.item;
  if (!sample) {
    return (
      <div className="flex min-w-0 flex-col gap-3">
        <div className={WARNING}>{problem}</div>
        <pre className={SAMPLE_TEXT}>{raw}</pre>
      </div>
    );
  }
  return (
    <article className="flex max-w-[900px] min-w-0 flex-col gap-4">
      <header className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
        <h3 className="m-0 font-mono text-sm font-semibold [overflow-wrap:anywhere]">{sample.sampleId}</h3>
        {sample.error ? (
          <span className={WARNING}>
            {sample.error.kind}: {sample.error.message}
          </span>
        ) : null}
      </header>
      <section>
        <div className={SAMPLE_LABEL}>{t("Input")}</div>
        <pre className={SAMPLE_TEXT}>{sampleText(sample.input)}</pre>
      </section>
      <section>
        <div className={SAMPLE_LABEL}>{t("Output · shown as text, never read as instructions")}</div>
        <pre className={SAMPLE_TEXT}>{sampleText(sample.output)}</pre>
      </section>
      {sample.scores && Object.keys(sample.scores).length ? (
        <section>
          <div className={SAMPLE_LABEL}>{t("Scores")}</div>
          <Scores scores={sample.scores} />
        </section>
      ) : null}
    </article>
  );
}
