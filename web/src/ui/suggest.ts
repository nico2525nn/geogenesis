// AI terrain suggestion: deterministic candidate generation + scoring.
//
// The UI evaluates a handful of low-resolution previews produced by the engine
// worker and keeps the most plausible "interesting world". Everything here is
// pure and deterministic: the same base parameters always produce the same
// candidates and therefore the same suggestion.

import type { Schema, TerrainData } from "../types";
import { clamp } from "./format";

/** Parameter groups that meaningfully change the shape of a world. */
const JITTER_GROUPS = new Set([
  "relief",
  "tectonics",
  "geology",
  "climate",
  "ecology",
  "hydrology",
  "fluvial",
  "hillslope",
  "cryosphere",
  "aeolian",
  "coastal",
  "chemistry",
]);

/** Small deterministic PRNG (Mulberry32-ish); UI-only, never engine state. */
function makeRng(seed: number): () => number {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x9e3779b9) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 16), 0x21f0aaad);
    t = Math.imul(t ^ (t >>> 15), 0x735a2d97);
    t = (t ^ (t >>> 15)) >>> 0;
    return t / 4294967296;
  };
}

/**
 * Builds `count` deterministic coherent variants of the given parameter set.
 *
 * Each candidate gets a new seed plus bounded jitter on world-shaping groups,
 * and the sea level is distributed relative to the vertical relief envelope so
 * land/ocean balance varies. Values are clamped/rounded exactly like the
 * parameter panel would.
 */
export function buildSuggestionCandidates(
  base: Float64Array,
  schema: Schema,
  count = 8,
): Float64Array[] {
  const index = new Map<string, number>(schema.params.map((spec, i) => [spec.key, i]));
  const seedAt = index.get("seed");
  const baseSeed = seedAt !== undefined ? Math.round(base[seedAt] ?? 0) >>> 0 : 0x9e3779b9;
  const seaAt = index.get("sea_level_m");
  const verticalAt = index.get("vertical_m");
  const candidates: Float64Array[] = [];
  for (let candidateIndex = 0; candidateIndex < count; candidateIndex += 1) {
    const rng = makeRng((baseSeed + Math.imul(candidateIndex + 1, 0x9e3779b9)) >>> 0);
    const values = Float64Array.from(base);
    if (seedAt !== undefined) {
      values[seedAt] = Math.floor(rng() * 0xffffffff);
    }
    for (let indexOnSchema = 0; indexOnSchema < schema.params.length; indexOnSchema += 1) {
      const spec = schema.params[indexOnSchema];
      if (spec === undefined || spec.key === "seed") continue;
      if (!JITTER_GROUPS.has(spec.group)) continue;
      const current = base[indexOnSchema] ?? spec.default;
      if (spec.kind === "enum") {
        const options = spec.options ?? [];
        if (options.length > 1 && rng() < 0.3) {
          values[indexOnSchema] = Math.floor(rng() * options.length);
        }
        continue;
      }
      if (spec.scale === "log" && current > 0) {
        const factor = Math.exp((rng() - 0.5) * 0.9);
        values[indexOnSchema] = clamp(current * factor, spec.min, spec.max);
      } else {
        const span = spec.max - spec.min;
        values[indexOnSchema] = clamp(
          current + (rng() - 0.5) * span * 0.18,
          spec.min,
          spec.max,
        );
      }
      if (spec.kind === "int") {
        values[indexOnSchema] = Math.round(values[indexOnSchema] ?? current);
      }
    }
    // Vary the land/water split within the vertical envelope so the scorer can
    // pick a world with a plausible coastline instead of one extreme.
    if (seaAt !== undefined && verticalAt !== undefined) {
      const seaSpec = schema.params[seaAt];
      const vertical = values[verticalAt] ?? 0;
      if (seaSpec !== undefined && vertical > 0) {
        const sea = (rng() * 0.5 - 0.15) * vertical;
        values[seaAt] = clamp(specRound(sea, seaSpec.kind), seaSpec.min, seaSpec.max);
      }
    }
    candidates.push(values);
  }
  return candidates;
}

function specRound(value: number, kind: string): number {
  return kind === "int" || kind === "enum" ? Math.round(value) : value;
}

export interface SuggestionMetrics {
  /** Fraction of preview cells above sea level. */
  landFraction: number;
  /** Robust relief divided by the world's vertical envelope (~0.35–1.0 ideal). */
  reliefRatio: number;
  /** Fraction of cells close to the sea level (coastline interest). */
  coastFraction: number;
  /** Combined plausibility score in 0..1. */
  score: number;
  /** Human readable summary used by the toast/suggestion bar. */
  label: string;
}

const TARGET_LAND = 0.35;

/** Scores one low-resolution preview against the plausibility heuristics. */
export function scoreSuggestion(
  terrain: TerrainData,
  values: Float64Array,
  index: ReadonlyMap<string, number>,
): SuggestionMetrics {
  const elevation = terrain.elevation;
  const total = elevation.length;
  if (total === 0) {
    return {
      landFraction: 0,
      reliefRatio: 0,
      coastFraction: 0,
      score: 0,
      label: "empty preview",
    };
  }
  const seaLevel = terrain.seaLevel;
  let land = 0;
  let min = Infinity;
  let max = -Infinity;
  for (let i = 0; i < total; i += 1) {
    const value = elevation[i];
    if (value === undefined || !Number.isFinite(value)) continue;
    if (value > seaLevel) land += 1;
    if (value < min) min = value;
    if (value > max) max = value;
  }
  const landFraction = land / total;
  const relief = Number.isFinite(min) && Number.isFinite(max) ? Math.max(max - min, 0) : 0;
  const verticalAt = index.get("vertical_m");
  const vertical = verticalAt === undefined ? 0 : values[verticalAt] ?? 0;
  const reliefRatio = relief / Math.max(vertical > 0 ? vertical : relief, 1);

  const band = Math.max(relief * 0.03, 120);
  let coast = 0;
  for (let i = 0; i < total; i += 1) {
    const value = elevation[i];
    if (value !== undefined && Number.isFinite(value) && Math.abs(value - seaLevel) <= band) {
      coast += 1;
    }
  }
  const coastFraction = coast / total;

  // Land fraction: triangle around the 0.35 target.
  const landScore = Math.max(0, 1 - Math.abs(landFraction - TARGET_LAND) / TARGET_LAND);
  // Relief: flat worlds and runaway spikes are both implausible.
  const reliefScore =
    reliefRatio < 0.35
      ? clamp(reliefRatio / 0.35, 0, 1)
      : reliefRatio <= 1
        ? 1
        : Math.max(0, 1 - (reliefRatio - 1) * 0.75);
  // Coastline: a little coast is interesting, saturation is not.
  const coastScore =
    coastFraction < 0.02
      ? clamp(coastFraction / 0.02, 0, 1) * 0.7
      : coastFraction <= 0.32
        ? 1
        : Math.max(0, 1 - (coastFraction - 0.32) * 1.4);
  // Both ocean and land should exist.
  const balanceScore = clamp(Math.min(landFraction, 1 - landFraction) / 0.2, 0, 1);

  const score = clamp(
    0.45 * landScore + 0.3 * reliefScore + 0.15 * coastScore + 0.1 * balanceScore,
    0,
    1,
  );
  return {
    landFraction,
    reliefRatio,
    coastFraction,
    score,
    label: describeSuggestion(landFraction, reliefRatio, coastFraction),
  };
}

function landName(landFraction: number): string {
  if (landFraction < 0.18) return "island world";
  if (landFraction < 0.3) return "archipelago";
  if (landFraction <= 0.42) return "balanced coastlines";
  if (landFraction <= 0.58) return "land-heavy continents";
  return "supercontinent";
}

/** One-line description such as "balanced coastlines · land 34% · …". */
export function describeSuggestion(
  landFraction: number,
  reliefRatio: number,
  coastFraction: number,
): string {
  return [
    landName(landFraction),
    `land ${(landFraction * 100).toFixed(0)}%`,
    `relief ${Math.round(reliefRatio * 100)}% of envelope`,
    `coast ${(coastFraction * 100).toFixed(0)}%`,
  ].join(" · ");
}
