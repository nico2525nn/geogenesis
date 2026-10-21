// Statistics panel: compact scientific readouts, charts and a mass ledger.
//
// The DOM is built once; updates only rewrite text nodes and the two canvases,
// so statistics can refresh repeatedly during playback without layout churn.

import type {
  BiomeInfo,
  EngineState,
  EngineStats,
  MineralsReport,
  TerrainData,
} from "../types";
import { el, setText } from "./dom";
import {
  CHART_COLOURS,
  drawArea,
  drawGridRows,
  drawHistogram,
  drawRule,
  elevationHistogram,
  label,
  prepareCanvas,
  type HistogramData,
} from "./charts";
import {
  formatDuration,
  formatLength,
  formatMass,
  formatPercent,
  formatSigned,
  formatUsd,
  formatVolume,
} from "./format";

export class StatsPanel {
  private readonly host: HTMLElement;
  private readonly meta: HTMLElement;
  private readonly badge: HTMLElement;
  private readonly badgeText: HTMLElement;
  private biomes: readonly BiomeInfo[] = [];
  private stats: EngineStats | null = null;
  private liveState: EngineState | null = null;
  private terrain: TerrainData | null = null;
  private histogram: HistogramData | null = null;
  private readonly refs = new Map<string, HTMLElement>();
  private readonly histogramCanvas: HTMLCanvasElement;
  private readonly hypsometryCanvas: HTMLCanvasElement;
  private readonly biomeBar: HTMLElement;
  private readonly biomeRows: HTMLElement;
  private readonly ledgerRows: HTMLElement;
  private readonly closureBadge: HTMLElement;
  private readonly tempGauge: HTMLElement;
  private readonly precipGauge: HTMLElement;
  private readonly landBar: HTMLElement;
  private readonly oceanBar: HTMLElement;
  private readonly landShare: HTMLElement;
  private readonly oceanShare: HTMLElement;
  private readonly hypsometryNote: HTMLElement;
  private readonly mineralsRows: HTMLElement;
  private readonly mineralsSummary: HTMLElement;
  private mineralsReport: MineralsReport | null = null;

  constructor(host: HTMLElement) {
    this.host = host;
    host.replaceChildren();
    host.classList.add("stats-panel");

    const head = el("header", { class: "stats-head" });
    const titleBlock = el("div", { class: "stats-title-block" });
    const title = el("h2", { class: "stats-title", text: "Statistics" });
    this.meta = el("div", { class: "stats-meta", text: "waiting for a world…" });
    titleBlock.append(title, this.meta);
    this.badge = el("div", { class: "stats-badge", hidden: true });
    this.badgeText = el("span", { text: "live" });
    this.badge.append(el("span", { class: "live-dot", "aria-hidden": "true" }), this.badgeText);
    head.append(titleBlock, this.badge);
    host.appendChild(head);

    // --- terrain ------------------------------------------------------------
    {
      const card = this.card("Terrain", "elevation distribution");
      const hero = el("div", { class: "stat-hero" });
      this.refs.set("mean", el("span", { class: "stat-hero-value", text: "—" }));
      this.refs.set("mean-unit", el("span", { class: "stat-hero-unit", text: "m mean elevation" }));
      hero.append(this.refs.get("mean")!, this.refs.get("mean-unit")!);
      card.body.appendChild(hero);
      const range = el("div", { class: "stat-range" });
      this.refs.set("range", el("span", { text: "— … — m" }));
      this.refs.set("relief", el("span", { class: "stat-range-relief", text: "relief — m" }));
      range.append(this.refs.get("range")!, this.refs.get("relief")!);
      card.body.appendChild(range);
      const grid = el("div", { class: "stat-grid" });
      card.body.appendChild(grid);
      this.kv(grid, "Median", "median");
      this.kv(grid, "Std dev", "std");
      this.kv(grid, "P10 – P90", "percentiles");
      const canvas = el("canvas", {
        class: "chart-canvas histogram-canvas",
        "aria-label": "Elevation histogram",
        role: "img",
      });
      card.body.appendChild(canvas);
      this.histogramCanvas = canvas;
      this.refs.set("histogram-caption", el("div", { class: "chart-caption", text: "cells by elevation" }));
      card.body.appendChild(this.refs.get("histogram-caption")!);
      host.appendChild(card.section);
    }

    // --- land and water -----------------------------------------------------
    {
      const card = this.card("Land & water", "sea level datum");
      const ratio = el("div", { class: "ratio-bar", role: "img", "aria-label": "Land versus ocean area" });
      this.landBar = el("span", { class: "ratio-land" });
      this.oceanBar = el("span", { class: "ratio-ocean" });
      ratio.append(this.landBar, this.oceanBar);
      card.body.appendChild(ratio);
      const ratioLabels = el("div", { class: "ratio-labels" });
      const landLabel = el("span", { class: "ratio-label" });
      this.landShare = el("strong", { text: "—" });
      landLabel.append(el("span", { class: "ratio-swatch land", "aria-hidden": "true" }), this.landShare, " land");
      const oceanLabel = el("span", { class: "ratio-label" });
      this.oceanShare = el("strong", { text: "—" });
      oceanLabel.append(el("span", { class: "ratio-swatch ocean", "aria-hidden": "true" }), this.oceanShare, " ocean");
      ratioLabels.append(landLabel, oceanLabel);
      card.body.appendChild(ratioLabels);
      const grid = el("div", { class: "stat-grid" });
      card.body.appendChild(grid);
      this.kv(grid, "Sea level", "sea");
      this.kv(grid, "Land area", "land-area");
      this.kv(grid, "Mean slope", "slope");
      this.kv(grid, "Steepest", "slope-max");
      this.kv(grid, "Drainage density", "drainage");
      this.kv(grid, "Channel cells", "channels");
      this.kv(grid, "Lake volume", "lakes");
      host.appendChild(card.section);
    }

    // --- climate ------------------------------------------------------------
    {
      const card = this.card("Climate", "surface means");
      this.tempGauge = this.gauge(card.body, "Mean temperature", "°C", -25, 40, "temp");
      this.precipGauge = this.gauge(card.body, "Precipitation", "mm/yr", 0, 3000, "precip");
      const grid = el("div", { class: "stat-grid" });
      card.body.appendChild(grid);
      this.kv(grid, "Hypsometric ∫", "hypsometry-integral");
      this.kv(grid, "Mean land height", "mean-land");
      this.kv(grid, "Simulated time", "elapsed");
      host.appendChild(card.section);
    }

    // --- mass ledger --------------------------------------------------------
    {
      const card = this.card("Mass balance", "cumulative volumes");
      this.ledgerRows = el("div", { class: "ledger-rows" });
      card.body.appendChild(this.ledgerRows);
      const footer = el("div", { class: "ledger-footer" });
      this.closureBadge = el("span", { class: "closure-badge", text: "—" });
      footer.append(el("span", { class: "ledger-note", text: "conservation closure" }), this.closureBadge);
      card.body.appendChild(footer);
      host.appendChild(card.section);
    }

    // --- biomes -------------------------------------------------------------
    {
      const card = this.card("Biomes", "share of land");
      this.biomeBar = el("div", { class: "stacked-bar", role: "img", "aria-label": "Biome composition" });
      card.body.appendChild(this.biomeBar);
      this.biomeRows = el("div", { class: "biome-rows" });
      card.body.appendChild(this.biomeRows);
      this.refs.set("biome-note", el("div", { class: "chart-caption", text: "no land yet" }));
      card.body.appendChild(this.refs.get("biome-note")!);
      host.appendChild(card.section);
    }

    // --- minerals -----------------------------------------------------------
    {
      const card = this.card("Minerals", "deposit survey");
      const grid = el("div", { class: "stat-grid" });
      card.body.appendChild(grid);
      this.kv(grid, "Deposit area", "mineral-area");
      this.kv(grid, "Total value", "mineral-value");
      this.mineralsRows = el("div", { class: "mineral-rows" });
      card.body.appendChild(this.mineralsRows);
      this.mineralsSummary = el("div", {
        class: "chart-caption",
        text: "waiting for mineral survey…",
      });
      card.body.appendChild(this.mineralsSummary);
      host.appendChild(card.section);
    }

    // --- hypsometry ---------------------------------------------------------
    {
      const card = this.card("Hypsometric curve", "height vs area");
      const canvas = el("canvas", {
        class: "chart-canvas hypsometry-canvas",
        "aria-label": "Hypsometric curve",
        role: "img",
      });
      card.body.appendChild(canvas);
      this.hypsometryCanvas = canvas;
      this.hypsometryNote = el("div", { class: "chart-caption" });
      card.body.appendChild(this.hypsometryNote);
      host.appendChild(card.section);
    }

    const resizeObserver = new ResizeObserver(() => {
      this.renderHistogram();
      this.renderHypsometry();
    });
    resizeObserver.observe(this.histogramCanvas);
    resizeObserver.observe(this.hypsometryCanvas);
    this.renderMinerals();
  }

  /** Injects the biome catalogue so colours and labels can be resolved. */
  setCatalogs(biomes: readonly BiomeInfo[]): void {
    this.biomes = biomes;
    if (this.stats !== null) this.update(this.stats);
  }

  /** Renders the latest mineral survey (or clears it). */
  setMinerals(report: MineralsReport | null): void {
    this.mineralsReport = report;
    this.renderMinerals();
  }

  /** Renders a new statistics report. */
  update(stats: EngineStats): void {
    this.stats = stats;
    const { elevation } = stats;
    setText(this.refs.get("mean")!, Math.round(elevation.mean).toLocaleString("en-US"));
    setText(
      this.refs.get("range")!,
      `${Math.round(elevation.min).toLocaleString("en-US")} … ${Math.round(elevation.max).toLocaleString("en-US")} m`,
    );
    setText(this.refs.get("relief")!, `relief ${formatLength(stats.max_relief_m)}`);
    setText(this.refs.get("median")!, `${Math.round(elevation.median).toLocaleString("en-US")} m`);
    setText(this.refs.get("std")!, `± ${Math.round(elevation.std).toLocaleString("en-US")} m`);
    setText(
      this.refs.get("percentiles")!,
      `${Math.round(elevation.p10).toLocaleString("en-US")} – ${Math.round(elevation.p90).toLocaleString("en-US")} m`,
    );

    // Land ratio bar.
    const land = Math.max(0, Math.min(1, stats.hypsometry.land_fraction));
    this.landBar.style.width = `${(land * 100).toFixed(2)}%`;
    this.oceanBar.style.width = `${((1 - land) * 100).toFixed(2)}%`;
    setText(this.landShare, formatPercent(land));
    setText(this.oceanShare, formatPercent(1 - land));

    setText(this.refs.get("sea")!, `${formatSigned(stats.sea_level_m, 0)} m`);
    setText(this.refs.get("land-area")!, `${Math.round(stats.land_area_km2).toLocaleString("en-US")} km²`);
    setText(this.refs.get("slope")!, `${stats.slope.mean_deg.toFixed(1)}°`);
    setText(this.refs.get("slope-max")!, `${stats.slope.max_deg.toFixed(1)}° (p90 ${stats.slope.p90_deg.toFixed(1)}°)`);
    setText(this.refs.get("drainage")!, `${stats.hydrology.drainage_density.toFixed(4)} km/km²`);
    setText(this.refs.get("channels")!, stats.hydrology.channel_cells.toLocaleString("en-US"));
    setText(this.refs.get("lakes")!, `${stats.hydrology.lake_volume_km3.toFixed(1)} km³`);

    // Climate gauges.
    this.setGauge(this.tempGauge, stats.climate.mean_temp_c, -25, 40, `${stats.climate.mean_temp_c.toFixed(1)} °C`);
    this.setGauge(
      this.precipGauge,
      stats.climate.mean_precip_mm,
      0,
      3000,
      `${Math.round(stats.climate.mean_precip_mm).toLocaleString("en-US")} mm/yr`,
    );
    setText(this.refs.get("hypsometry-integral")!, stats.hypsometry.integral.toFixed(3));
    setText(this.refs.get("mean-land")!, `${Math.round(stats.hypsometry.mean_land_m).toLocaleString("en-US")} m`);
    setText(this.refs.get("elapsed")!, formatDuration(stats.elapsed_yr));

    // Header metadata.
    setText(
      this.meta,
      `${stats.grid.width}×${stats.grid.height} · ${stats.grid.cell_km} km/cell · t = ${formatDuration(stats.elapsed_yr)}`,
    );

    this.renderHistogram();
    this.renderBiomes();
    this.renderHypsometry();
    this.updateLedger();
  }

  /** Live state (ledger, elapsed time) that arrives with every terrain frame. */
  setLiveState(state: EngineState): void {
    this.liveState = state;
    this.updateLedger();
  }

  /** Provides the current elevation grid for the histogram. */
  setTerrain(terrain: TerrainData | null): void {
    const changed =
      terrain === null ||
      this.terrain === null ||
      this.terrain.time !== terrain.time ||
      this.terrain.elevation !== terrain.elevation ||
      this.terrain.seaLevel !== terrain.seaLevel;
    this.terrain = terrain;
    if (changed) {
      this.histogram = null;
      this.renderHistogram();
    }
  }

  /** Shows a pulsing "measuring" badge while the engine is busy. */
  setRunning(running: boolean, text = "updating"): void {
    this.badge.hidden = !running;
    setText(this.badgeText, text);
    this.host.classList.toggle("is-running", running);
    if (running) {
      this.badge.classList.add("is-running");
      this.badge.setAttribute("title", "Measured values refresh when the current operation completes");
    } else {
      this.badge.classList.remove("is-running");
      this.badge.removeAttribute("title");
    }
  }

  // --- builders -------------------------------------------------------------

  private card(title: string, note: string): { section: HTMLElement; body: HTMLElement } {
    const section = el("section", { class: "stat-card" });
    const head = el("header", { class: "card-head" });
    head.append(el("h3", { text: title }), el("span", { class: "card-note", text: note }));
    const body = el("div", { class: "card-body" });
    section.append(head, body);
    return { section, body };
  }

  private kv(parent: HTMLElement, labelText: string, key: string): void {
    const row = el("div", { class: "stat-row" });
    const name = el("span", { class: "stat-label", text: labelText });
    const value = el("span", { class: "stat-value", text: "—" });
    row.append(name, value);
    parent.appendChild(row);
    this.refs.set(key, value);
  }

  private gauge(
    parent: HTMLElement,
    title: string,
    unit: string,
    min: number,
    max: number,
    key: string,
  ): HTMLElement {
    const wrapper = el("div", { class: "gauge" });
    const head = el("div", { class: "gauge-head" });
    const value = el("span", { class: "gauge-value", text: "—" });
    head.append(el("span", { class: "gauge-label", text: title }), value, el("span", { class: "gauge-unit", text: unit }));
    const track = el("div", { class: "gauge-track", role: "img", "aria-label": title });
    const fill = el("span", { class: "gauge-fill" });
    track.appendChild(fill);
    wrapper.append(head, track);
    parent.appendChild(wrapper);
    this.refs.set(key, value);
    this.refs.set(`${key}-fill`, fill);
    this.refs.set(`${key}-min`, el("span", { class: "gauge-edge", text: String(min) }));
    this.refs.set(`${key}-max`, el("span", { class: "gauge-edge", text: String(max) }));
    const scale = el("div", { class: "gauge-scale" });
    scale.append(this.refs.get(`${key}-min`)!, this.refs.get(`${key}-max`)!);
    wrapper.appendChild(scale);
    return wrapper;
  }

  private setGauge(wrapper: HTMLElement, value: number, min: number, max: number, text: string): void {
    const fill = wrapper.querySelector<HTMLElement>(".gauge-fill");
    const valueEl = wrapper.querySelector<HTMLElement>(".gauge-value");
    const fraction = Math.max(0, Math.min(1, (value - min) / Math.max(max - min, 1e-9)));
    if (fill !== null) fill.style.width = `${(fraction * 100).toFixed(1)}%`;
    if (valueEl !== null) valueEl.textContent = text;
  }

  // --- renderers ------------------------------------------------------------

  private renderMinerals(): void {
    this.mineralsRows.replaceChildren();
    const report = this.mineralsReport;
    if (report === null) {
      setText(this.refs.get("mineral-area")!, "—");
      setText(this.refs.get("mineral-value")!, "—");
      setText(this.mineralsSummary, "waiting for mineral survey…");
      return;
    }
    const cells = Math.max(report.map.cells, 1);
    setText(
      this.refs.get("mineral-area")!,
      `${formatPercent(report.map.deposit_cells / cells)} · ${Math.round(
        report.inventory.deposit_area_km2,
      ).toLocaleString("en-US")} km²`,
    );
    setText(this.refs.get("mineral-value")!, formatUsd(report.inventory.total_value_usd));

    const catalogue = report.minerals;
    const top = [...report.inventory.entries].sort((a, b) => b.tonnage_t - a.tonnage_t).slice(0, 3);
    for (const entry of top) {
      const info = catalogue[entry.index];
      const colour = info?.colour ?? [128, 128, 128];
      const row = el("div", { class: "mineral-row" });
      const swatch = el("span", { class: "mineral-swatch", "aria-hidden": "true" });
      swatch.style.background = `rgb(${colour[0]}, ${colour[1]}, ${colour[2]})`;
      const name = el("span", { class: "mineral-name", text: info?.label ?? entry.key });
      const detail = el("span", {
        class: "mineral-detail",
        text: `${formatPercent(entry.coverage)} · ${formatMass(entry.tonnage_t)} · ${formatUsd(entry.value_usd)}`,
      });
      row.append(swatch, name, detail);
      this.mineralsRows.appendChild(row);
    }
    const dominant = report.map.dominant[0];
    const dominantInfo = dominant === undefined ? undefined : catalogue[dominant.index];
    const summaryParts = [
      `${catalogue.length} minerals`,
      `${report.inventory.entries.length} with deposits`,
    ];
    if (dominant !== undefined) {
      summaryParts.push(
        `dominant ${dominantInfo?.label ?? dominant.key} ${formatPercent(dominant.fraction)}`,
      );
    }
    setText(this.mineralsSummary, summaryParts.join(" · "));
  }

  private renderHistogram(): void {
    const surface = prepareCanvas(this.histogramCanvas);
    if (surface === null) return;
    const { ctx, width, height } = surface;
    if (this.terrain !== null) {
      this.histogram ??= elevationHistogram(this.terrain.elevation, this.terrain.seaLevel, 48, 2);
    }
    if (this.histogram === null) {
      label(ctx, "no terrain yet", 4, height / 2, { colour: CHART_COLOURS.text });
      return;
    }
    drawHistogram(ctx, width, height, this.histogram);
  }

  private renderBiomes(): void {
    const stats = this.stats;
    if (stats === null) return;
    const landFraction = Math.max(stats.hypsometry.land_fraction, 1e-6);
    const entries = stats.biomes
      .filter((biome) => biome.fraction > 0.0005)
      .map((biome) => {
        const catalog = this.biomes.find((candidate) => candidate.key === biome.key);
        const colour = catalog?.colour ?? [120, 120, 120];
        return {
          ...biome,
          colour,
          share: Math.min(biome.fraction / landFraction, 1),
        };
      })
      .sort((a, b) => b.fraction - a.fraction)
      .slice(0, 8);

    this.biomeBar.replaceChildren();
    this.biomeRows.replaceChildren();
    let assigned = 0;
    for (const biome of entries) {
      const segment = el("span", {
        class: "stacked-segment",
        title: `${biome.label} ${(biome.share * 100).toFixed(1)}% of land`,
      });
      segment.style.width = `${(biome.share * 100).toFixed(2)}%`;
      segment.style.background = `rgb(${biome.colour[0]}, ${biome.colour[1]}, ${biome.colour[2]})`;
      this.biomeBar.appendChild(segment);
      assigned += biome.share;

      const row = el("div", { class: "biome-row" });
      const swatch = el("span", { class: "biome-swatch" });
      swatch.style.background = `rgb(${biome.colour[0]}, ${biome.colour[1]}, ${biome.colour[2]})`;
      const bar = el("span", { class: "biome-bar" });
      const fill = el("span", { class: "biome-bar-fill" });
      fill.style.width = `${Math.max(biome.share * 100, 0.6).toFixed(2)}%`;
      fill.style.background = `rgb(${biome.colour[0]}, ${biome.colour[1]}, ${biome.colour[2]})`;
      bar.appendChild(fill);
      const name = el("span", { class: "biome-name", text: biome.label });
      const value = el("span", { class: "biome-value", text: formatPercent(biome.share) });
      row.append(swatch, name, bar, value);
      this.biomeRows.appendChild(row);
    }
    if (entries.length === 0) {
      this.biomeRows.appendChild(el("div", { class: "chart-caption", text: "no land biomes yet" }));
    } else if (assigned < 0.995) {
      const row = el("div", { class: "biome-row" });
      row.append(
        el("span", { class: "biome-swatch is-other" }),
        el("span", { class: "biome-name", text: "other" }),
        el("span", { class: "biome-bar" }),
        el("span", { class: "biome-value", text: formatPercent(Math.max(1 - assigned, 0)) }),
      );
      this.biomeRows.appendChild(row);
    }
    setText(
      this.refs.get("biome-note")!,
      `land covers ${formatPercent(stats.hypsometry.land_fraction)} of the grid · ${entries.length} biomes`,
    );
    this.biomeBar.setAttribute(
      "aria-label",
      `Biome composition: ${entries.map((biome) => `${biome.label} ${formatPercent(biome.share)}`).join(", ")}`,
    );
  }

  private renderHypsometry(): void {
    const surface = prepareCanvas(this.hypsometryCanvas);
    if (surface === null) return;
    const { ctx, width, height } = surface;
    const stats = this.stats;
    const insetTop = 6;
    const insetBottom = 16;
    const insetLeft = 18;
    drawGridRows(ctx, width, height, 3);
    if (stats === null) {
      label(ctx, "waiting for statistics…", 4, height / 2, { colour: CHART_COLOURS.text });
      return;
    }
    const curve = stats.hypsometry.curve;
    const plotWidth = width - insetLeft;
    const points = curve.map(([h, area]) => ({
      x: insetLeft + Math.max(0, Math.min(area, 1)) * plotWidth,
      y: 1 - Math.max(0, Math.min(h, 1)),
    }));
    // drawArea wants normalised x/y; keep the axes labels separate for clarity.
    drawArea(
      ctx,
      width,
      height,
      points.map((point) => ({ x: point.x / width, y: point.y })),
      {
        stroke: CHART_COLOURS.accent,
        fill: CHART_COLOURS.accentDim,
        lineWidth: 1.5,
        insetTop,
        insetBottom,
      },
    );
    // Axes.
    ctx.save();
    ctx.strokeStyle = CHART_COLOURS.axis;
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(insetLeft + 0.5, insetTop);
    ctx.lineTo(insetLeft + 0.5, height - insetBottom);
    ctx.lineTo(width, height - insetBottom);
    ctx.stroke();
    ctx.restore();
    label(ctx, "1", 4, insetTop + 8, { size: 9 });
    label(ctx, "0", 4, height - insetBottom, { size: 9 });
    label(ctx, "0", insetLeft, height - 4, { size: 9 });
    label(ctx, "1", width - 6, height - 4, { size: 9, align: "right" });
    label(ctx, "area", width / 2 - 8, height - 4, { size: 9 });
    drawRule(ctx, width, height, height - insetBottom, CHART_COLOURS.grid, [2, 3]);
    setText(
      this.hypsometryNote,
      `∫ = ${stats.hypsometry.integral.toFixed(3)} · mean land ${Math.round(stats.hypsometry.mean_land_m).toLocaleString("en-US")} m`,
    );
  }

  private updateLedger(): void {
    const ledger = this.liveState?.ledger ?? null;
    if (ledger === null) {
      this.ledgerRows.replaceChildren(
        el("div", { class: "chart-caption", text: "waiting for a simulation state…" }),
      );
      return;
    }
    const erosion = Math.max(Math.abs(ledger.erosion_m3), 1);
    const rows: Array<[string, number, string]> = [
      ["Eroded", ledger.erosion_m3, "erosion"],
      ["Deposited", ledger.deposition_m3, "deposition"],
      ["Exported", ledger.export_m3, "export"],
      ["In storage", ledger.storage_m3, "storage"],
    ];
    this.ledgerRows.replaceChildren();
    for (const [labelText, value, kind] of rows) {
      const row = el("div", { class: `ledger-row ledger-${kind}` });
      const name = el("span", { class: "ledger-label", text: labelText });
      const bar = el("span", { class: "ledger-bar" });
      const fill = el("span", { class: "ledger-bar-fill" });
      const fraction = Math.min(Math.abs(value) / erosion, 1);
      fill.style.width = `${Math.max(fraction * 100, 0.5).toFixed(2)}%`;
      bar.appendChild(fill);
      const amount = el("span", { class: "ledger-value", text: formatVolume(value) });
      row.append(name, bar, amount);
      this.ledgerRows.appendChild(row);
    }
    const closurePct = (Math.abs(ledger.closure_error_m3) / erosion) * 100;
    this.closureBadge.textContent = closurePct < 0.05 ? "exact" : `${closurePct.toFixed(2)}% error`;
    this.closureBadge.classList.toggle("is-good", closurePct < 0.05);
    this.closureBadge.classList.toggle("is-warn", closurePct >= 0.05 && closurePct < 1);
    this.closureBadge.classList.toggle("is-bad", closurePct >= 1);
    this.closureBadge.setAttribute(
      "data-tip",
      `Closure error ${formatVolume(ledger.closure_error_m3)} relative to ${formatVolume(ledger.erosion_m3)} eroded`,
    );
  }
}
