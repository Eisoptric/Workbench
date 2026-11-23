import { t } from "../../domain/i18n";
import type { AttributeValue } from "../../domain/model";

export function formatDuration(ms: number): string {
  if (!Number.isFinite(ms)) return "—";
  if (ms <= 0) return "0";
  if (ms < 1) return `${Math.max(1, Math.round(ms * 1000))} µs`;
  if (ms < 1000) return `${ms < 10 ? ms.toFixed(2) : ms < 100 ? ms.toFixed(1) : Math.round(ms)} ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(ms < 10_000 ? 2 : 1)} s`;
  return `${Math.floor(ms / 60_000)}m ${Math.round((ms % 60_000) / 1000)}s`;
}

export function formatClock(ms: number): string {
  const date = new Date(ms);
  return `${date.toLocaleTimeString([], { hour12: false })}.${String(date.getMilliseconds()).padStart(3, "0")}`;
}

export function formatAgo(ms: number): string {
  const seconds = Math.round((Date.now() - ms) / 1000);
  if (seconds < 5) return t("just now");
  if (seconds < 60) return t("{count}s ago", { count: seconds });
  if (seconds < 3600) return t("{count}m ago", { count: Math.round(seconds / 60) });
  return t("{count}h ago", { count: Math.round(seconds / 3600) });
}

export function formatNumber(value: number): string {
  if (Number.isInteger(value)) return value.toLocaleString();
  return value.toLocaleString(undefined, { maximumFractionDigits: 3 });
}

/** Formats a metric value by its OTel unit: seconds as durations, "{token}" style units as counts. */
export function formatUnit(value: number, unit: string): string {
  if (unit === "s") return formatDuration(value * 1000);
  if (unit === "ms") return formatDuration(value);
  const curly = /^\{(.+)\}$/.exec(unit);
  if (curly) return `${formatNumber(value)} ${curly[1]}${value === 1 ? "" : "s"}`;
  return unit ? `${formatNumber(value)} ${unit}` : formatNumber(value);
}

export function formatValue(value: AttributeValue): string {
  if (value === null || value === undefined) return "null";
  if (Array.isArray(value)) return JSON.stringify(value);
  return String(value);
}

/** Shows paths under the home folder as ~/…, the way people type them. */
export function homeRelative(file: string): string {
  return file.replace(/^\/(?:Users|home)\/[^/]+/, "~").replace(/^[A-Za-z]:\\Users\\[^\\]+/, "~");
}

export function shortId(id: string): string {
  return id.slice(0, 8);
}
