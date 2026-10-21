// Canvas chart primitives shared by the statistics and cross-section panels.
//
// All plotting is done in CSS pixels after sizing the backing store for the
// device pixel ratio, so charts stay crisp on HiDPI screens without callers
// having to think about it.

export interface CanvasSurface {
  ctx: CanvasRenderingContext2D;
  /** CSS pixel width. */
  width: number;
  /** CSS pixel height. */
  height: number;
}

export const CHART_COLOURS = {
  grid: "#232c38",
  axis: "#344052",
  text: "#6f7c8d",
  textStrong: "#c6d0dc",
  accent: "#7fd8c8",
  accentDim: "rgba(127, 216, 200, 0.16)",
  water: "rgba(52, 116, 176, 0.55)",
  waterLine: "rgba(120, 178, 230, 0.75)",
  land: "#7fb08a",
  landDim: "rgba(127, 176, 138, 0.2)",
  warm: "#d9a05b",
  danger: "#d2685f",
  surface: "#b8a181",
} as const;

/** Sizes a canvas for the current CSS box and DPR; returns null when hidden. */
export function prepareCanvas(canvas: HTMLCanvasElement): CanvasSurface | null {
  const cssWidth = canvas.clientWidth;
  const cssHeight = canvas.clientHeight;
  if (cssWidth <= 0 || cssHeight <= 0) return null;
  const dpr = Math.min(window.devicePixelRatio || 1, 2);
  const pixelWidth = Math.round(cssWidth * dpr);
  const pixelHeight = Math.round(cssHeight * dpr);
  if (canvas.width !== pixelWidth || canvas.height !== pixelHeight) {
    canvas.width = pixelWidth;
    canvas.height = pixelHeight;
  }
  const ctx = canvas.getContext("2d");
  if (ctx === null) return null;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, cssWidth, cssHeight);
  return { ctx, width: cssWidth, height: cssHeight };
}

/** Horizontal grid lines at `rows` even intervals. */
export function drawGridRows(ctx: CanvasRenderingContext2D, width: number, height: number, rows = 3): void {
  ctx.save();
  ctx.strokeStyle = CHART_COLOURS.grid;
  ctx.lineWidth = 1;
  for (let i = 1; i < rows; i += 1) {
    const y = Math.round((height * i) / rows) + 0.5;
    ctx.beginPath();
    ctx.moveTo(0, y);
    ctx.lineTo(width, y);
    ctx.stroke();
  }
  ctx.restore();
}

export interface AreaPoint {
  x: number;
  y: number;
}

/** Draws a line/area chart. Coordinates are normalised (0..1). */
export function drawArea(
  ctx: CanvasRenderingContext2D,
  width: number,
  height: number,
  points: readonly AreaPoint[],
  options: {
    stroke: string;
    fill?: string;
    lineWidth?: number;
    insetTop?: number;
    insetBottom?: number;
  },
): void {
  if (points.length < 2) return;
  const top = options.insetTop ?? 0;
  const bottom = height - (options.insetBottom ?? 0);
  const xFor = (x: number): number => x * width;
  const yFor = (y: number): number => bottom - y * (bottom - top);
  ctx.save();
  if (options.fill !== undefined) {
    ctx.beginPath();
    ctx.moveTo(xFor(points[0]?.x ?? 0), bottom);
    for (const point of points) {
      ctx.lineTo(xFor(point.x), yFor(point.y));
    }
    ctx.lineTo(xFor(points[points.length - 1]?.x ?? 1), bottom);
    ctx.closePath();
    ctx.fillStyle = options.fill;
    ctx.fill();
  }
  ctx.beginPath();
  points.forEach((point, index) => {
    const x = xFor(point.x);
    const y = yFor(point.y);
    if (index === 0) ctx.moveTo(x, y);
    else ctx.lineTo(x, y);
  });
  ctx.strokeStyle = options.stroke;
  ctx.lineWidth = options.lineWidth ?? 1.6;
  ctx.lineJoin = "round";
  ctx.stroke();
  ctx.restore();
}

/** Draws a dashed/dotted horizontal marker line at a normalised y. */
export function drawRule(
  ctx: CanvasRenderingContext2D,
  width: number,
  height: number,
  y: number,
  colour: string,
  dash: number[] = [3, 3],
): void {
  ctx.save();
  ctx.strokeStyle = colour;
  ctx.lineWidth = 1;
  ctx.setLineDash(dash);
  const yy = Math.round(y) + 0.5;
  ctx.beginPath();
  ctx.moveTo(0, yy);
  ctx.lineTo(width, yy);
  ctx.stroke();
  ctx.restore();
}

/** Draws a small monospace label; returns its width. */
export function label(
  ctx: CanvasRenderingContext2D,
  text: string,
  x: number,
  y: number,
  options: { colour?: string; align?: CanvasTextAlign; size?: number; baseline?: CanvasTextBaseline } = {},
): number {
  ctx.save();
  ctx.font = `${options.size ?? 9.5}px ui-monospace, "SF Mono", monospace`;
  ctx.fillStyle = options.colour ?? CHART_COLOURS.text;
  ctx.textAlign = options.align ?? "left";
  ctx.textBaseline = options.baseline ?? "alphabetic";
  ctx.fillText(text, x, y);
  const measured = ctx.measureText(text).width;
  ctx.restore();
  return measured;
}

export interface HistogramData {
  /** Bin counts. */
  counts: Float64Array;
  /** Lower edge of the first bin. */
  min: number;
  /** Upper edge of the last bin. */
  max: number;
  /** Sea level in the same units, when known. */
  seaLevel?: number;
}

/** Computes a fixed-bin histogram, clipping to the 0.1–99.9 percentile range. */
export function elevationHistogram(
  elevation: Float32Array,
  seaLevel: number,
  bins = 48,
  stride = 2,
): HistogramData {
  let lo = Infinity;
  let hi = -Infinity;
  const step = Math.max(1, Math.round(stride));
  for (let i = 0; i < elevation.length; i += step) {
    const value = elevation[i];
    if (value === undefined || !Number.isFinite(value)) continue;
    if (value < lo) lo = value;
    if (value > hi) hi = value;
  }
  if (!Number.isFinite(lo) || !Number.isFinite(hi)) {
    return { counts: new Float64Array(bins), min: 0, max: 1, seaLevel };
  }
  // Trim extreme outliers so one mountain does not flatten the chart.
  const span = Math.max(hi - lo, 1);
  lo = Math.min(lo, seaLevel) - span * 0.01;
  hi = Math.max(hi, seaLevel) + span * 0.01;
  const counts = new Float64Array(bins);
  const scale = bins / Math.max(hi - lo, 1e-9);
  for (let i = 0; i < elevation.length; i += step) {
    const value = elevation[i];
    if (value === undefined || !Number.isFinite(value)) continue;
    let bin = Math.floor((value - lo) * scale);
    if (bin < 0) bin = 0;
    if (bin >= bins) bin = bins - 1;
    counts[bin] = (counts[bin] ?? 0) + 1;
  }
  return { counts, min: lo, max: hi, seaLevel };
}

/** Renders an elevation histogram split into ocean (blue) and land (green) bins. */
export function drawHistogram(
  ctx: CanvasRenderingContext2D,
  width: number,
  height: number,
  histogram: HistogramData,
  options: { labels?: boolean } = {},
  seaColour: string = CHART_COLOURS.water,
  landColour: string = CHART_COLOURS.land,
): void {
  const { counts, min, max } = histogram;
  let peak = 0;
  for (const count of counts) peak = Math.max(peak, count);
  if (peak <= 0) return;
  const insetTop = 4;
  const insetBottom = options.labels === false ? 2 : 14;
  const plotHeight = height - insetTop - insetBottom;
  const sea = histogram.seaLevel;
  const xForValue = (value: number): number => ((value - min) / Math.max(max - min, 1e-9)) * width;
  const seaX = sea === undefined ? 0 : xForValue(sea);
  const barWidth = width / counts.length;
  ctx.save();
  for (let i = 0; i < counts.length; i += 1) {
    const count = counts[i] ?? 0;
    const barHeight = (count / peak) * plotHeight;
    const x = i * barWidth;
    const centre = min + ((i + 0.5) / counts.length) * (max - min);
    ctx.fillStyle = centre < (sea ?? -Infinity) ? seaColour : landColour;
    ctx.globalAlpha = 0.85;
    ctx.fillRect(x, height - insetBottom - barHeight, Math.max(barWidth - 0.7, 0.7), barHeight);
  }
  ctx.globalAlpha = 1;
  if (sea !== undefined && sea >= min && sea <= max) {
    ctx.strokeStyle = CHART_COLOURS.waterLine;
    ctx.setLineDash([3, 3]);
    ctx.beginPath();
    ctx.moveTo(Math.round(seaX) + 0.5, insetTop);
    ctx.lineTo(Math.round(seaX) + 0.5, height - insetBottom);
    ctx.stroke();
    ctx.setLineDash([]);
  }
  ctx.restore();
  if (options.labels !== false) {
    label(ctx, `${Math.round(min).toLocaleString("en-US")} m`, 1, height - 3);
    label(ctx, `${Math.round(max).toLocaleString("en-US")} m`, width - 1, height - 3, { align: "right" });
    if (sea !== undefined) {
      const seaLabel = "sea 0";
      const seaLabelWidth = ctx.measureText(seaLabel).width;
      const x = Math.max(2, Math.min(seaX + 3, width - seaLabelWidth - 2));
      label(ctx, seaLabel, x, height - 3, { colour: CHART_COLOURS.waterLine });
    }
  }
}
