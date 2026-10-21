// Project file save/load helpers.
//
// A project captures everything needed to reproduce the world that is currently
// displayed: the fully resolved parameter values, the simulation time, the seed
// and the overlay mode. The format is deliberately plain JSON and reuses the
// same metadata conventions as the Parameters JSON export, so either file can
// be loaded by the UI.

import { COLOR_MODE_LABELS } from "../types";
import type { ColorMode, Schema } from "../types";
import { clamp } from "./format";

export const PROJECT_FORMAT = "geogenesis-project";
export const PROJECT_VERSION = 2;

/** User-defined geology carried by a project (raw JSON documents + engine echo). */
export interface ProjectGeology {
  rocksJson: string | null;
  strataJson: string | null;
  /** Canonical JSON echoed by the engine (`Worker {kind:"geology"}`), if available. */
  engineJson: string | null;
}

export interface ProjectMeta {
  presetKey: string | null;
  seed: number | null;
  mode: ColorMode;
  elapsedYears: number;
  engineVersion: string;
  grid: { width: number; height: number; cellKm: number } | null;
  totalYears: number | null;
  stepYears: number | null;
  geology?: ProjectGeology | null;
}

/** Serialises a fully resolved parameter set into a project JSON document. */
export function buildProjectJson(
  values: Float64Array,
  schema: Schema,
  meta: ProjectMeta,
): string {
  const parameters: Record<string, number> = {};
  for (const [index, spec] of schema.params.entries()) {
    parameters[spec.key] = values[index] ?? spec.default;
  }
  return JSON.stringify(
    {
      format: PROJECT_FORMAT,
      version: PROJECT_VERSION,
      engineVersion: meta.engineVersion,
      presetKey: meta.presetKey,
      seed: meta.seed,
      mode: meta.mode,
      elapsedYears: meta.elapsedYears,
      savedAt: new Date().toISOString(),
      grid: meta.grid,
      time: {
        elapsed_yr: meta.elapsedYears,
        total_yr: meta.totalYears,
        step_yr: meta.stepYears,
      },
      parameters,
      geology: meta.geology ?? null,
    },
    null,
    2,
  );
}

export interface LoadedProject {
  values: Float64Array;
  elapsedYears: number;
  mode: ColorMode | null;
  presetKey: string | null;
  seed: number | null;
  engineVersion: string | null;
  savedAt: string | null;
  geology: ProjectGeology | null;
  warnings: string[];
}

export type ProjectParseResult =
  | { ok: true; project: LoadedProject }
  | { ok: false; error: string };

const KNOWN_FORMATS = new Set(["geogenesis-project", "geogenesis-params"]);

function coerceNumber(raw: unknown): number | null {
  if (typeof raw === "number") return Number.isFinite(raw) ? raw : null;
  if (typeof raw === "string" && raw.trim().length > 0) {
    const parsed = Number.parseFloat(raw);
    return Number.isFinite(parsed) ? parsed : null;
  }
  return null;
}

function asRecord(raw: unknown): Record<string, unknown> | null {
  return typeof raw === "object" && raw !== null && !Array.isArray(raw)
    ? (raw as Record<string, unknown>)
    : null;
}

/**
 * Parses and validates a project/params JSON document against the live schema.
 *
 * Accepts both the canonical project shape (`parameters` map) and the older
 * Parameters JSON export (`values` flat array). Unknown keys are ignored,
 * missing/invalid values fall back to schema defaults and are reported as
 * warnings; hard errors are reserved for unreadable/unsupported files.
 */
export function parseProjectJson(text: string, schema: Schema): ProjectParseResult {
  let raw: unknown;
  try {
    raw = JSON.parse(text);
  } catch {
    return { ok: false, error: "file is not valid JSON" };
  }
  const record = asRecord(raw);
  if (record === null) {
    return { ok: false, error: "project file must contain a JSON object" };
  }
  const format = record.format;
  if (typeof format === "string" && !KNOWN_FORMATS.has(format)) {
    return { ok: false, error: `unsupported file format "${format}"` };
  }
  const versionValue = coerceNumber(record.version);
  if (versionValue !== null && versionValue > PROJECT_VERSION) {
    return {
      ok: false,
      error: `unsupported project version ${versionValue} (this build supports up to ${PROJECT_VERSION})`,
    };
  }

  const warnings: string[] = [];
  const count = schema.params.length;
  const values = new Float64Array(count);
  const parameters = asRecord(record.parameters);
  const flatValues = record.values;
  let recognised = 0;
  let missing = 0;
  let invalid = 0;
  let clamped = 0;

  const readFromMap = parameters !== null;
  if (!readFromMap && !Array.isArray(flatValues)) {
    return { ok: false, error: "missing parameters (expected a 'parameters' object)" };
  }
  if (!readFromMap && Array.isArray(flatValues) && flatValues.length !== count) {
    return {
      ok: false,
      error: `parameter array has ${flatValues.length} values but the engine expects ${count}`,
    };
  }

  for (let index = 0; index < count; index += 1) {
    const spec = schema.params[index];
    if (spec === undefined) continue;
    let candidate: number | null;
    if (readFromMap) {
      const rawValue = parameters?.[spec.key];
      candidate = coerceNumber(rawValue);
      if (rawValue === undefined) {
        missing += 1;
        candidate = null;
      }
    } else {
      candidate = coerceNumber((flatValues as unknown[])[index]);
    }
    if (candidate === null) {
      values[index] = spec.default;
      if (!readFromMap) missing += 1;
      continue;
    }
    recognised += 1;
    let safe = candidate;
    const bounded = clamp(candidate, spec.min, spec.max);
    if (Math.abs(bounded - candidate) > Math.max(Math.abs(candidate), 1) * 1e-12) {
      clamped += 1;
      safe = bounded;
    }
    if (spec.kind === "int" || spec.kind === "enum") safe = Math.round(safe);
    if (!Number.isFinite(safe)) {
      invalid += 1;
      safe = spec.default;
    }
    values[index] = safe;
  }

  if (recognised === 0) {
    return { ok: false, error: "no recognised parameter keys — this does not look like a GeoGenesis project" };
  }
  if (missing > 0) warnings.push(`${missing} parameter${missing === 1 ? "" : "s"} missing — schema defaults used`);
  if (invalid > 0) warnings.push(`${invalid} invalid value${invalid === 1 ? "" : "s"} replaced by defaults`);
  if (clamped > 0) warnings.push(`${clamped} value${clamped === 1 ? "" : "s"} clamped to the schema range`);

  const timeRecord = asRecord(record.time);
  const elapsedCandidate =
    coerceNumber(record.elapsedYears) ??
    coerceNumber(timeRecord?.elapsed_yr) ??
    coerceNumber(timeRecord?.elapsedYears);
  const elapsedYears = elapsedCandidate === null ? 0 : Math.max(0, elapsedCandidate);
  if (elapsedCandidate !== null && elapsedCandidate < 0) {
    warnings.push("negative saved time clamped to 0");
  }

  const modeRaw = record.mode ?? record.display_mode;
  let mode: ColorMode | null = null;
  if (typeof modeRaw === "string") {
    const match = COLOR_MODE_LABELS.find((entry) => entry.key === modeRaw);
    if (match !== undefined) mode = match.key;
    else warnings.push(`unknown display mode "${modeRaw}" ignored`);
  }

  const seedCandidate = coerceNumber(record.seed) ?? (parameters !== null ? coerceNumber(parameters.seed) : null);
  const seed = seedCandidate === null ? null : Math.round(seedCandidate);
  const engineVersionRaw = record.engineVersion ?? record.engine;
  const engineVersion = typeof engineVersionRaw === "string" ? engineVersionRaw : null;
  if (engineVersion !== null && schema.version.length > 0 && engineVersion !== schema.version) {
    warnings.push(`saved with engine ${engineVersion} (current ${schema.version})`);
  }
  const presetKeyRaw = record.presetKey ?? record.preset;
  const presetKey = typeof presetKeyRaw === "string" && presetKeyRaw.length > 0 ? presetKeyRaw : null;
  const savedAtRaw = record.savedAt ?? record.exported_at;
  const savedAt = typeof savedAtRaw === "string" ? savedAtRaw : null;

  let geology: ProjectGeology | null = null;
  const geologyRecord = asRecord(record.geology);
  if (geologyRecord !== null) {
    const rocksJson = typeof geologyRecord.rocksJson === "string" ? geologyRecord.rocksJson : null;
    const strataJson = typeof geologyRecord.strataJson === "string" ? geologyRecord.strataJson : null;
    const engineJson = typeof geologyRecord.engineJson === "string" ? geologyRecord.engineJson : null;
    if (rocksJson === null && strataJson === null && engineJson === null) {
      warnings.push("project geology block was empty — default geology used");
    } else {
      geology = { rocksJson, strataJson, engineJson };
    }
  }

  return {
    ok: true,
    project: {
      values,
      elapsedYears,
      mode,
      presetKey,
      seed,
      engineVersion,
      savedAt,
      geology,
      warnings,
    },
  };
}
