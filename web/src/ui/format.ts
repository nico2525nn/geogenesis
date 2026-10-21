// Number and duration formatting shared by the UI panels.
//
// Everything here is pure and allocation-light: the statistics and timeline
// hot paths call these during playback.

import type { ParamSpec } from "../types";

/** Formats a parameter value using its schema unit and scale. */
export function formatValue(spec: ParamSpec, value: number): string {
  if (spec.kind === "enum") {
    return spec.options?.[Math.round(value)] ?? String(value);
  }
  if (spec.kind === "int") {
    return Math.round(value).toString();
  }
  if (spec.scale === "log") {
    if (!Number.isFinite(value)) return "—";
    if (value === 0) return "0";
    const abs = Math.abs(value);
    if (abs < 1e-4 || abs >= 1e7) return trimZeros(value.toExponential(2));
    if (abs >= 1000) {
      // Preserve three significant digits but keep integers readable.
      const rounded = abs >= 10000 ? Math.round(value / 10) * 10 : Math.round(value);
      return rounded.toLocaleString("en-US");
    }
    if (abs >= 1) return trimZeros(value.toPrecision(3));
    return trimZeros(value.toPrecision(3));
  }
  const span = Math.abs(spec.max - spec.min);
  if (span <= 1) return trimZeros(value.toFixed(4));
  if (span <= 20) return trimZeros(value.toFixed(2));
  if (span <= 500) return trimZeros(value.toFixed(1));
  if (span <= 20000) return `${Math.round(value).toLocaleString("en-US")}`;
  return value.toLocaleString("en-US", { maximumFractionDigits: 0 });
}

/** Formats a value with its unit suffix. */
export function formatWithUnit(spec: ParamSpec, value: number): string {
  const text = formatValue(spec, value);
  if (!spec.unit || text === "—") return text;
  return spec.unit === "%" ? `${text}%` : `${text} ${spec.unit}`;
}

/** Strips trailing zeros from a fixed/exponential string. */
function trimZeros(text: string): string {
  if (!text.includes(".")) return text;
  const trimmed = text.replace(/0+$/, "").replace(/\.$/, "");
  return trimmed === "-0" ? "0" : trimmed;
}

/** Formats a duration in years using geological units. */
export function formatDuration(years: number): string {
  const abs = Math.abs(years);
  if (!Number.isFinite(years)) return "—";
  if (abs >= 1e9) return `${trimZeros((years / 1e9).toFixed(2))} Gyr`;
  if (abs >= 1e6) return `${trimZeros((years / 1e6).toFixed(2))} Myr`;
  if (abs >= 1e3) return `${trimZeros((years / 1e3).toFixed(1))} kyr`;
  return `${Math.round(years)} yr`;
}

/** Short duration for axis ticks: `0`, `250 kyr`, `1.5 Myr`. */
export function formatDurationTick(years: number): string {
  const abs = Math.abs(years);
  if (abs < 1) return "0";
  if (abs >= 1e9) return `${trimZeros((years / 1e9).toFixed(1))} Gyr`;
  if (abs >= 1e6) {
    const value = years / 1e6;
    return `${trimZeros(value.toFixed(Math.abs(value) >= 10 ? 1 : 2))} Myr`;
  }
  if (abs >= 1e3) return `${Math.round(years / 1e3).toLocaleString("en-US")} kyr`;
  return `${Math.round(years)} yr`;
}

/** Formats a length in metres, switching to km for large values. */
export function formatLength(metres: number): string {
  const abs = Math.abs(metres);
  if (!Number.isFinite(metres)) return "—";
  if (abs >= 1000) return `${trimZeros((metres / 1000).toFixed(abs >= 10000 ? 0 : 1))} km`;
  if (abs >= 10) return `${Math.round(metres)} m`;
  if (abs >= 1) return `${trimZeros(metres.toFixed(1))} m`;
  return `${trimZeros(metres.toFixed(2))} m`;
}

/** Short length for axes: `0`, `40 km`. */
export function formatLengthTick(metres: number): string {
  const abs = Math.abs(metres);
  if (abs < 1) return "0";
  if (abs >= 1000) return `${trimZeros((metres / 1000).toFixed(abs >= 10000 ? 0 : 1))} km`;
  return `${Math.round(metres)} m`;
}

/** Formats a volume in cubic metres with SI-ish scaling. */
export function formatVolume(volume: number): string {
  const abs = Math.abs(volume);
  if (!Number.isFinite(volume)) return "—";
  if (abs >= 1e18) return `${trimZeros((volume / 1e18).toFixed(2))} Em³`;
  if (abs >= 1e15) return `${trimZeros((volume / 1e15).toFixed(2))} Pm³`;
  if (abs >= 1e12) return `${trimZeros((volume / 1e12).toFixed(2))} Tm³`;
  if (abs >= 1e9) return `${trimZeros((volume / 1e9).toFixed(2))} Gm³`;
  if (abs >= 1e6) return `${trimZeros((volume / 1e6).toFixed(2))} Mm³`;
  if (abs >= 1e3) return `${trimZeros((volume / 1e3).toFixed(2))} km³`;
  return `${Math.round(volume)} m³`;
}

/** Compact US-dollar amount such as `$12.4B`. */
export function formatUsd(value: number): string {
  if (!Number.isFinite(value)) return "—";
  const abs = Math.abs(value);
  const sign = value < 0 ? "−" : "";
  if (abs >= 1e12) return `${sign}$${trimZeros((abs / 1e12).toFixed(2))}T`;
  if (abs >= 1e9) return `${sign}$${trimZeros((abs / 1e9).toFixed(2))}B`;
  if (abs >= 1e6) return `${sign}$${trimZeros((abs / 1e6).toFixed(2))}M`;
  if (abs >= 1e3) return `${sign}$${trimZeros((abs / 1e3).toFixed(1))}k`;
  return `${sign}$${Math.round(abs).toLocaleString("en-US")}`;
}

/** Compact tonnage such as `1.24 Gt`. */
export function formatMass(tonnes: number): string {
  if (!Number.isFinite(tonnes)) return "—";
  const abs = Math.abs(tonnes);
  if (abs >= 1e9) return `${trimZeros((tonnes / 1e9).toFixed(2))} Gt`;
  if (abs >= 1e6) return `${trimZeros((tonnes / 1e6).toFixed(2))} Mt`;
  if (abs >= 1e3) return `${trimZeros((tonnes / 1e3).toFixed(1))} kt`;
  return `${Math.round(tonnes).toLocaleString("en-US")} t`;
}

/** Compact count such as `1.2M` or `384`. */
export function formatCount(value: number): string {
  const abs = Math.abs(value);
  if (!Number.isFinite(value)) return "—";
  if (abs >= 1e9) return `${trimZeros((value / 1e9).toFixed(1))}B`;
  if (abs >= 1e6) return `${trimZeros((value / 1e6).toFixed(1))}M`;
  if (abs >= 1e4) return `${trimZeros((value / 1e3).toFixed(1))}k`;
  return Math.round(value).toLocaleString("en-US");
}

/** Formats a compact percentage. */
export function formatPercent(fraction: number, digits = 1): string {
  if (!Number.isFinite(fraction)) return "—";
  return `${(fraction * 100).toFixed(digits)}%`;
}

/** Formats a signed value with an explicit `+` for positives. */
export function formatSigned(value: number, digits = 1): string {
  const rounded = Number(value.toFixed(digits));
  const text = Math.abs(rounded).toLocaleString("en-US", {
    minimumFractionDigits: digits,
    maximumFractionDigits: digits,
  });
  return `${rounded > 0 ? "+" : rounded < 0 ? "−" : ""}${text}`;
}

/** Clamps a number to a range. */
export function clamp(value: number, min: number, max: number): number {
  return value < min ? min : value > max ? max : value;
}

/**
 * Parses a duration string such as `20000`, `20k`, `2.5Myr`, `500 kyr`, `1.2e6`.
 * Returns `null` when the text cannot be interpreted.
 */
export function parseDuration(text: string): number | null {
  const cleaned = text.trim().toLowerCase().replaceAll(",", "").replaceAll("_", "");
  if (cleaned.length === 0) return null;
  const match = /^(-?\d*\.?\d+(?:e[+-]?\d+)?)\s*(gyr|myr|kyr|yr|g|m|k|y|a)?$/.exec(cleaned);
  if (match === null) return null;
  const value = Number.parseFloat(match[1] ?? "");
  if (!Number.isFinite(value)) return null;
  switch (match[2]) {
    case "gyr":
    case "g":
      return value * 1e9;
    case "myr":
    case "m":
      return value * 1e6;
    case "kyr":
    case "k":
      return value * 1e3;
    case "yr":
    case "y":
    case "a":
    case undefined:
      return value;
    default:
      return null;
  }
}

/** A "nice" axis step (1, 2, 2.5 or 5 × 10ⁿ) covering `range` in ~`target` ticks. */
export function niceStep(range: number, target = 5): number {
  if (!(range > 0) || !Number.isFinite(range)) return 1;
  const rough = range / Math.max(target, 2);
  const magnitude = 10 ** Math.floor(Math.log10(rough));
  const residual = rough / magnitude;
  const multiplier = residual >= 5 ? 5 : residual >= 2.5 ? 2.5 : residual >= 2 ? 2 : 1;
  return multiplier * magnitude;
}

/** CSS colour from an RGB triplet. */
export function rgb(colour: readonly [number, number, number]): string {
  return `rgb(${colour[0]}, ${colour[1]}, ${colour[2]})`;
}

/** CSS colour with alpha from an RGB triplet. */
export function rgba(colour: readonly [number, number, number], alpha: number): string {
  return `rgba(${colour[0]}, ${colour[1]}, ${colour[2]}, ${alpha})`;
}

/** Interpolates between two RGB triplets. */
export function mixColour(
  a: readonly [number, number, number],
  b: readonly [number, number, number],
  t: number,
): [number, number, number] {
  const k = clamp(t, 0, 1);
  return [
    Math.round(a[0] + (b[0] - a[0]) * k),
    Math.round(a[1] + (b[1] - a[1]) * k),
    Math.round(a[2] + (b[2] - a[2]) * k),
  ];
}
