// Custom geology documents: validation, summaries and persistence.
//
// The engine accepts two optional JSON documents on generate: a custom rock
// catalogue (`{version, custom_rocks:[…]}`) and a strata stack
// (`{version, layers:[…]}`). The UI keeps the raw document text so projects can
// save and restore exactly what the user loaded.

import type { ProjectGeology } from "./project";

const STORAGE_KEY = "geogenesis.geology.v1";

export interface GeologyDocument {
  /** Parsed JSON text (pretty-printed for storage). */
  json: string;
  /** Number of rocks or strata layers. */
  count: number;
  /** Compact human summary for status lines. */
  summary: string;
}

export type GeologyParseResult =
  | { ok: true; document: GeologyDocument }
  | { ok: false; error: string };

function asRecord(value: unknown): Record<string, unknown> | null {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

/** Validates a custom rocks JSON document. */
export function parseRocksJson(text: string): GeologyParseResult {
  let raw: unknown;
  try {
    raw = JSON.parse(text);
  } catch {
    return { ok: false, error: "not valid JSON" };
  }
  const record = asRecord(raw);
  if (record === null) return { ok: false, error: "expected a JSON object" };
  const list = record.custom_rocks ?? record.rocks ?? record.customRocks;
  if (!Array.isArray(list)) {
    return { ok: false, error: "missing custom_rocks array" };
  }
  return {
    ok: true,
    document: {
      json: JSON.stringify(raw, null, 2),
      count: list.length,
      summary: `${list.length} custom rock${list.length === 1 ? "" : "s"}`,
    },
  };
}

/** Validates a strata stack JSON document. */
export function parseStrataJson(text: string): GeologyParseResult {
  let raw: unknown;
  try {
    raw = JSON.parse(text);
  } catch {
    return { ok: false, error: "not valid JSON" };
  }
  const record = asRecord(raw);
  if (record === null) return { ok: false, error: "expected a JSON object" };
  const layers = record.layers ?? record.strata;
  if (!Array.isArray(layers)) {
    return { ok: false, error: "missing layers array" };
  }
  return {
    ok: true,
    document: {
      json: JSON.stringify(raw, null, 2),
      count: layers.length,
      summary: `${layers.length} strata layer${layers.length === 1 ? "" : "s"}`,
    },
  };
}

/** One-line description of the active custom geology, or null when default. */
export function describeGeology(rocksJson: string | null, strataJson: string | null): string | null {
  if (rocksJson === null && strataJson === null) return null;
  const parts: string[] = [];
  if (rocksJson !== null) {
    const parsed = parseRocksJson(rocksJson);
    parts.push(parsed.ok ? parsed.document.summary : "custom rocks");
  }
  if (strataJson !== null) {
    const parsed = parseStrataJson(strataJson);
    parts.push(parsed.ok ? `strata stack (${parsed.document.count} layers)` : "strata stack");
  }
  return `custom geology: ${parts.join(" + ")}`;
}

/** Loads persisted geology documents (best effort). */
export function readStoredGeology(): { rocksJson: string | null; strataJson: string | null } {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw === null) return { rocksJson: null, strataJson: null };
    const record = asRecord(JSON.parse(raw));
    if (record === null) return { rocksJson: null, strataJson: null };
    const rocksJson = typeof record.rocksJson === "string" ? record.rocksJson : null;
    const strataJson = typeof record.strataJson === "string" ? record.strataJson : null;
    // Re-validate so corrupt storage cannot poison the next generation.
    const rocksOk = rocksJson !== null && parseRocksJson(rocksJson).ok;
    const strataOk = strataJson !== null && parseStrataJson(strataJson).ok;
    return {
      rocksJson: rocksOk ? rocksJson : null,
      strataJson: strataOk ? strataJson : null,
    };
  } catch {
    return { rocksJson: null, strataJson: null };
  }
}

/** Persists (or clears) the active geology documents. */
export function writeStoredGeology(rocksJson: string | null, strataJson: string | null): void {
  try {
    if (rocksJson === null && strataJson === null) {
      localStorage.removeItem(STORAGE_KEY);
    } else {
      localStorage.setItem(STORAGE_KEY, JSON.stringify({ rocksJson, strataJson }));
    }
  } catch {
    // Private mode: persistence is optional.
  }
}

/** Project payload for the current geology state. */
export function geologyProjectPayload(
  rocksJson: string | null,
  strataJson: string | null,
  engineJson: string | null,
): ProjectGeology | null {
  if (rocksJson === null && strataJson === null && (engineJson === null || engineJson === "null")) {
    return null;
  }
  return { rocksJson, strataJson, engineJson };
}
