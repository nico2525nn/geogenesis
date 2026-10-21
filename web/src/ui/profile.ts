// Cross-section (profile) renderer.
//
// Draws a scientific section with elevation/distance axes, sea level, water
// fill, a biome colour strip and a bedrock hint band, plus a hover readout that
// shows the sample under the cursor.

import type { BiomeInfo, ProfileSample, RockInfo } from "../types";
import { el, setText } from "./dom";
import { CHART_COLOURS, label, prepareCanvas } from "./charts";
import { formatLength, formatLengthTick, formatSigned, niceStep } from "./format";

export interface ProfileOptions {
  /** Optional header element; the panel renders its title/readout there. */
  head?: HTMLElement | null;
}

const PADDING = { left: 44, right: 16, top: 12, bottom: 22 } as const;

export class ProfilePanel {
  private readonly canvas: HTMLCanvasElement;
  private readonly head: HTMLElement | null;
  private readoutEl: HTMLElement | null = null;
  private biomes: readonly BiomeInfo[] = [];
  private rocks: readonly RockInfo[] = [];
  private samples: ProfileSample[] = [];
  private seaLevel: number | null = null;
  private hoverIndex: number | null = null;
  private lastDistance = 0;
  private lastRelief = 0;

  constructor(canvas: HTMLCanvasElement, options: ProfileOptions = {}) {
    this.canvas = canvas;
    this.head = options.head ?? null;
    if (this.head !== null) this.buildHead();
    const resizeObserver = new ResizeObserver(() => this.draw());
    resizeObserver.observe(canvas);
    canvas.addEventListener("pointermove", this.onPointerMove);
    canvas.addEventListener("pointerleave", this.onPointerLeave);
    this.draw();
  }

  private buildHead(): void {
    if (this.head === null) return;
    this.head.replaceChildren();
    const titleRow = el("div", { class: "section-title-row" });
    titleRow.append(
      el("h2", { class: "section-title", text: "Cross-section" }),
      el("span", { class: "section-hint", text: "drag on the minimap" }),
    );
    const readout = el("div", { class: "section-readout", "aria-live": "polite" });
    readout.textContent = "No section yet — drag a line on the map.";
    this.head.append(titleRow, readout);
    this.readoutEl = readout;
  }

  /** Injects the biome/rock catalogues. */
  setCatalogs(biomes: readonly BiomeInfo[], rocks: readonly RockInfo[] = []): void {
    this.biomes = biomes;
    this.rocks = rocks;
    this.draw();
  }

  /** Renders a new cross-section. */
  setSamples(samples: ProfileSample[], seaLevel?: number): void {
    const sampled = samples.length >= 2;
    this.samples = samples;
    this.seaLevel = Number.isFinite(seaLevel) ? (seaLevel as number) : null;
    if (sampled) {
      this.lastDistance = samples[samples.length - 1]?.distance_m ?? 0;
      let min = Infinity;
      let max = -Infinity;
      for (const sample of samples) {
        min = Math.min(min, sample.elevation_m);
        max = Math.max(max, sample.elevation_m);
      }
      this.lastRelief = max - min;
    }
    this.updateReadout();
    this.draw();
  }

  /** Current samples (used when saving a project). */
  getSamples(): readonly ProfileSample[] {
    return this.samples;
  }

  private updateReadout(): void {
    const readout = this.readoutEl;
    if (readout === null) return;
    if (this.samples.length < 2) {
      setText(readout, "No section yet — drag a line on the map.");
      return;
    }
    if (this.hoverIndex !== null) {
      const sample = this.samples[this.hoverIndex];
      if (sample !== undefined) {
        const biome = this.biomes.find((entry) => entry.id === sample.biome);
        setText(
          readout,
          `${formatLength(sample.distance_m)} · ${formatSigned(sample.elevation_m, 0)} m · ${
            sample.water_depth_m > 0 ? `water ${Math.round(sample.water_depth_m)} m` : "dry"
          } · ${biome?.label ?? "unknown biome"}`,
        );
        return;
      }
    }
    const exaggeration = this.verticalExaggeration();
    setText(
      readout,
      `${formatLength(this.lastDistance)} · relief ${formatLength(this.lastRelief)}${
        exaggeration === null ? "" : ` · ≈${exaggeration.toFixed(0)}× vertical`
      }`,
    );
  }

  private verticalExaggeration(): number | null {
    const samples = this.samples;
    if (samples.length < 2) return null;
    const width = this.canvas.clientWidth - PADDING.left - PADDING.right;
    const height = this.canvas.clientHeight - PADDING.top - PADDING.bottom;
    const distance = samples[samples.length - 1]?.distance_m ?? 0;
    if (width <= 0 || height <= 0 || distance <= 0) return null;
    const horizontalMPerPx = distance / width;
    const verticalMPerPx = Math.max(this.lastRelief, 1) / height;
    return horizontalMPerPx / verticalMPerPx;
  }

  private readonly onPointerMove = (event: PointerEvent): void => {
    if (this.samples.length < 2) return;
    const rect = this.canvas.getBoundingClientRect();
    const x = event.clientX - rect.left;
    const plotWidth = Math.max(rect.width - PADDING.left - PADDING.right, 1);
    const fraction = Math.max(0, Math.min((x - PADDING.left) / plotWidth, 1));
    const index = Math.round(fraction * (this.samples.length - 1));
    if (index !== this.hoverIndex) {
      this.hoverIndex = index;
      this.updateReadout();
      this.draw();
    }
  };

  private readonly onPointerLeave = (): void => {
    if (this.hoverIndex === null) return;
    this.hoverIndex = null;
    this.updateReadout();
    this.draw();
  };

  private draw(): void {
    const surface = prepareCanvas(this.canvas);
    if (surface === null) return;
    const { ctx, width, height } = surface;
    const plotWidth = Math.max(width - PADDING.left - PADDING.right, 1);
    const plotHeight = Math.max(height - PADDING.top - PADDING.bottom, 1);
    const plotBottom = PADDING.top + plotHeight;

    // Background sky gradient.
    const sky = ctx.createLinearGradient(0, 0, 0, height);
    sky.addColorStop(0, "#0d141d");
    sky.addColorStop(1, "#080b10");
    ctx.fillStyle = sky;
    ctx.fillRect(0, 0, width, height);

    const samples = this.samples;
    if (samples.length < 2) {
      label(ctx, "drag on the minimap to draw a section", PADDING.left, height / 2, {
        colour: CHART_COLOURS.text,
      });
      ctx.strokeStyle = CHART_COLOURS.grid;
      ctx.setLineDash([4, 5]);
      ctx.strokeRect(PADDING.left + 0.5, PADDING.top + 0.5, plotWidth, plotHeight);
      ctx.setLineDash([]);
      return;
    }

    let minElevation = Infinity;
    let maxElevation = -Infinity;
    let submerged = 0;
    let waterSum = 0;
    for (const sample of samples) {
      minElevation = Math.min(minElevation, sample.elevation_m);
      maxElevation = Math.max(maxElevation, sample.elevation_m);
      if (sample.water_depth_m > 0) {
        submerged += 1;
        waterSum += sample.elevation_m + sample.water_depth_m;
      }
    }
    const seaLevel =
      this.seaLevel ?? (submerged > 0 ? waterSum / submerged : 0);
    minElevation = Math.min(minElevation, seaLevel);
    maxElevation = Math.max(maxElevation, seaLevel);
    const span = Math.max(maxElevation - minElevation, 80);
    minElevation -= span * 0.06;
    maxElevation += span * 0.1;
    const totalDistance = samples[samples.length - 1]?.distance_m ?? 1;

    const xFor = (distance: number): number => PADDING.left + (distance / totalDistance) * plotWidth;
    const yFor = (elevation: number): number =>
      PADDING.top + plotHeight - ((elevation - minElevation) / (maxElevation - minElevation)) * plotHeight;

    // Elevation grid + labels.
    const elevationStep = niceStep(maxElevation - minElevation, 4);
    const firstGrid = Math.ceil(minElevation / elevationStep) * elevationStep;
    ctx.save();
    ctx.font = '9.5px ui-monospace, "SF Mono", monospace';
    for (let value = firstGrid; value <= maxElevation; value += elevationStep) {
      const y = Math.round(yFor(value)) + 0.5;
      const isSea = Math.abs(value) < 1e-6;
      ctx.strokeStyle = isSea ? "rgba(94, 148, 200, 0.35)" : CHART_COLOURS.grid;
      ctx.beginPath();
      ctx.moveTo(PADDING.left, y);
      ctx.lineTo(width - PADDING.right, y);
      ctx.stroke();
      if (!isSea) {
        label(ctx, `${Math.round(value).toLocaleString("en-US")}`, PADDING.left - 4, y + 3, {
          align: "right",
          colour: CHART_COLOURS.text,
        });
      }
    }
    ctx.restore();

    // Distance axis ticks.
    const distanceStep = niceStep(totalDistance, 4);
    for (let value = 0; value <= totalDistance; value += distanceStep) {
      const x = Math.round(xFor(value)) + 0.5;
      ctx.strokeStyle = CHART_COLOURS.grid;
      ctx.beginPath();
      ctx.moveTo(x, plotBottom);
      ctx.lineTo(x, plotBottom + 3);
      ctx.stroke();
      const nearRight = x > width - 46;
      label(ctx, formatLengthTick(value), x, height - 7, {
        align: nearRight ? "right" : value === 0 ? "left" : "center",
        colour: CHART_COLOURS.text,
      });
    }

    // Terrain body fill.
    ctx.beginPath();
    ctx.moveTo(xFor(0), plotBottom);
    for (const sample of samples) ctx.lineTo(xFor(sample.distance_m), yFor(sample.elevation_m));
    ctx.lineTo(xFor(totalDistance), plotBottom);
    ctx.closePath();
    const ground = ctx.createLinearGradient(0, PADDING.top, 0, plotBottom);
    ground.addColorStop(0, "#3a3128");
    ground.addColorStop(1, "#221b15");
    ctx.fillStyle = ground;
    ctx.fill();

    // Water fill: spans where the surface is submerged.
    ctx.save();
    ctx.beginPath();
    let inWater = false;
    for (const sample of samples) {
      const submergedSample = sample.water_depth_m > 0;
      if (submergedSample) {
        if (!inWater) {
          ctx.moveTo(xFor(sample.distance_m), yFor(seaLevel));
          inWater = true;
        }
        ctx.lineTo(xFor(sample.distance_m), yFor(sample.elevation_m));
      } else if (inWater) {
        ctx.lineTo(xFor(sample.distance_m), yFor(seaLevel));
        inWater = false;
      }
    }
    if (inWater) {
      ctx.lineTo(xFor(totalDistance), yFor(seaLevel));
    }
    ctx.closePath();
    const waterGradient = ctx.createLinearGradient(0, yFor(seaLevel), 0, plotBottom);
    waterGradient.addColorStop(0, "rgba(74, 142, 200, 0.62)");
    waterGradient.addColorStop(1, "rgba(24, 58, 104, 0.72)");
    ctx.fillStyle = waterGradient;
    ctx.fill();
    ctx.restore();

    // Surface silhouette.
    ctx.beginPath();
    samples.forEach((sample, index) => {
      const x = xFor(sample.distance_m);
      const y = yFor(sample.elevation_m);
      if (index === 0) ctx.moveTo(x, y);
      else ctx.lineTo(x, y);
    });
    ctx.strokeStyle = CHART_COLOURS.surface;
    ctx.lineWidth = 1.2;
    ctx.lineJoin = "round";
    ctx.stroke();

    // Biome strip + bedrock hint band.
    const bandWidth = Math.max(plotWidth / samples.length, 1);
    for (let i = 0; i < samples.length; i += 1) {
      const sample = samples[i];
      if (sample === undefined) continue;
      const biome = this.biomes.find((entry) => entry.id === sample.biome);
      const biomeColour = biome?.colour ?? [120, 120, 120];
      const rock = this.rocks.find((entry) => entry.id === sample.rock);
      const rockColour = rock?.colour ?? [92, 84, 74];
      const x = xFor(sample.distance_m) - bandWidth / 2;
      const y = yFor(sample.elevation_m);
      ctx.fillStyle = `rgb(${biomeColour[0]}, ${biomeColour[1]}, ${biomeColour[2]})`;
      ctx.fillRect(x, y - 3, bandWidth + 0.5, 3);
      ctx.fillStyle = `rgba(${rockColour[0]}, ${rockColour[1]}, ${rockColour[2]}, 0.5)`;
      ctx.fillRect(x, y + 0.2, bandWidth + 0.5, 2.4);
    }

    // Sea-level line.
    const seaY = Math.round(yFor(seaLevel)) + 0.5;
    ctx.save();
    ctx.strokeStyle = CHART_COLOURS.waterLine;
    ctx.setLineDash([5, 4]);
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(PADDING.left, seaY);
    ctx.lineTo(width - PADDING.right, seaY);
    ctx.stroke();
    ctx.setLineDash([]);
    label(ctx, `sea ${formatSigned(seaLevel, 0)} m`, width - PADDING.right - 2, seaY - 4, {
      align: "right",
      colour: CHART_COLOURS.waterLine,
    });
    ctx.restore();

    // Hover cursor.
    if (this.hoverIndex !== null) {
      const sample = this.samples[this.hoverIndex];
      if (sample !== undefined) {
        const x = Math.round(xFor(sample.distance_m)) + 0.5;
        ctx.save();
        ctx.strokeStyle = "rgba(200, 214, 230, 0.45)";
        ctx.lineWidth = 1;
        ctx.beginPath();
        ctx.moveTo(x, PADDING.top);
        ctx.lineTo(x, plotBottom);
        ctx.stroke();
        ctx.fillStyle = CHART_COLOURS.accent;
        ctx.beginPath();
        ctx.arc(x, yFor(sample.elevation_m), 2.6, 0, Math.PI * 2);
        ctx.fill();
        ctx.restore();
      }
    }

    // Bullet-proof frame.
    ctx.strokeStyle = CHART_COLOURS.grid;
    ctx.strokeRect(PADDING.left + 0.5, PADDING.top + 0.5, plotWidth, plotHeight);

    this.canvas.setAttribute(
      "aria-label",
      `Terrain cross-section over ${formatLength(totalDistance)}, relief ${formatLength(this.lastRelief)}`,
    );
  }
}
