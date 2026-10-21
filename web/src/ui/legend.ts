// Overlay legend driven by the shared colour ramps.
//
// Continuous modes show the gradient with min/mid/max labels, categorical
// modes show swatch rows (two columns when the list is long). The whole legend
// is collapsible, and starts collapsed on narrow viewports so it does not
// cover the terrain.

import { legendFor, type LegendSpec } from "../colors";
import type { BiomeInfo, ColorMode, MineralInfo, RockInfo } from "../types";
import { el, icon, setText } from "./dom";

const TEXT: Record<string, string> = {
  elevation: "Elevation",
  biomes: "Biomes",
  bedrock: "Bedrock",
  rivers: "Rivers",
  major_river: "Major river",
  channel: "Channel",
  minerals: "Minerals",
  mineral_deposits: "Mineral deposits",
  deposit: "Deposit",
  none: "None",
};

const COLLAPSE_KEY = "geogenesis.legend.collapsed";

function readCollapsed(): boolean | null {
  try {
    const raw = localStorage.getItem(COLLAPSE_KEY);
    if (raw === null) return null;
    return raw === "1";
  } catch {
    return null;
  }
}

function writeCollapsed(collapsed: boolean): void {
  try {
    localStorage.setItem(COLLAPSE_KEY, collapsed ? "1" : "0");
  } catch {
    // Persistence is optional.
  }
}

export class Legend {
  private readonly host: HTMLElement;
  private readonly body: HTMLElement;
  private readonly title: HTMLElement;
  private readonly toggleButton: HTMLButtonElement;
  private mode: ColorMode = "terrain";
  private biomes: readonly BiomeInfo[] = [];
  private rocks: readonly RockInfo[] = [];
  private minerals: readonly MineralInfo[] = [];
  private collapsed = false;

  constructor(host: HTMLElement) {
    this.host = host;
    host.replaceChildren();
    host.classList.add("legend-panel");

    const stored = readCollapsed();
    const narrow = window.matchMedia("(max-width: 980px)").matches;
    this.collapsed = stored ?? narrow;

    const head = el("div", { class: "legend-head" });
    this.title = el("h3", { class: "legend-title", text: "Elevation" });
    this.toggleButton = el("button", {
      type: "button",
      class: "legend-toggle",
      "aria-label": "Collapse legend",
    });
    this.toggleButton.appendChild(icon("chevron-right", 13));
    this.toggleButton.addEventListener("click", () => this.setCollapsed(!this.collapsed));
    head.append(this.title, this.toggleButton);
    this.body = el("div", { class: "legend-body", id: "legend-body" });
    host.append(head, this.body);
    this.toggleButton.setAttribute("aria-controls", "legend-body");
    this.applyCollapsed();
    host.hidden = true;
  }

  /** Provides catalogues for categorical modes (biomes, rocks, minerals). */
  setCatalogs(
    biomes: readonly BiomeInfo[],
    rocks: readonly RockInfo[],
    minerals: readonly MineralInfo[] = [],
  ): void {
    this.biomes = biomes;
    this.rocks = rocks;
    this.minerals = minerals;
    this.render();
  }

  /** Changes the displayed overlay mode. */
  setMode(mode: ColorMode): void {
    this.mode = mode;
    this.render();
  }

  /** Collapses or expands the legend body. */
  setCollapsed(collapsed: boolean): void {
    this.collapsed = collapsed;
    writeCollapsed(collapsed);
    this.applyCollapsed();
  }

  /** Shows/hides the legend entirely (e.g. before a world exists). */
  setVisible(visible: boolean): void {
    this.host.hidden = !visible;
  }

  private applyCollapsed(): void {
    this.host.classList.toggle("is-collapsed", this.collapsed);
    this.body.hidden = this.collapsed;
    this.toggleButton.setAttribute("aria-expanded", this.collapsed ? "false" : "true");
    this.toggleButton.setAttribute("aria-label", this.collapsed ? "Expand legend" : "Collapse legend");
  }

  private render(): void {
    const spec = legendFor(
      this.mode,
      this.biomes,
      this.rocks,
      (key) => TEXT[key] ?? key,
      this.minerals,
    );
    this.host.hidden = false;
    this.host.dataset.mode = this.mode;
    setText(this.title, spec.unit ? `${spec.title} (${spec.unit})` : spec.title);
    this.body.replaceChildren();
    if (spec.kind === "ramp" && spec.stops !== undefined) {
      this.body.appendChild(this.ramp(spec));
    } else if (spec.kind === "list" && spec.items !== undefined) {
      this.body.appendChild(this.list(spec));
    }
    this.body.setAttribute(
      "aria-label",
      spec.kind === "list"
        ? `${spec.title} legend with ${spec.items?.length ?? 0} categories`
        : `${spec.title} colour ramp from ${formatEdge(spec.min ?? 0)} to ${formatEdge(spec.max ?? 1)}`,
    );
  }

  private ramp(spec: LegendSpec): HTMLElement {
    const wrapper = el("div", { class: "legend-ramp-wrap" });
    const strip = el("div", { class: "legend-ramp" });
    const stops = (spec.stops ?? []).map(
      ([t, colour]) => `rgb(${colour[0]},${colour[1]},${colour[2]}) ${(t * 100).toFixed(1)}%`,
    );
    strip.style.background = `linear-gradient(90deg, ${stops.join(", ")})`;
    const labels = el("div", { class: "legend-labels" });
    const min = spec.min ?? 0;
    const max = spec.max ?? 1;
    const mid = spec.title.toLowerCase().includes("elevation") ? 0 : (min + max) / 2;
    labels.append(
      el("span", { class: "legend-edge", text: formatEdge(min) }),
      el("span", { class: "legend-edge is-mid", text: formatEdge(mid) }),
      el("span", { class: "legend-edge", text: formatEdge(max) }),
    );
    wrapper.append(strip, labels);
    return wrapper;
  }

  private list(spec: LegendSpec): HTMLElement {
    const wrapper = el("div", { class: "legend-list" });
    const items = spec.items ?? [];
    if (items.length > 6) wrapper.classList.add("is-multicolumn");
    for (const item of items) {
      const row = el("div", { class: "legend-row" });
      const swatch = el("span", { class: "legend-swatch", "aria-hidden": "true" });
      swatch.style.background = `rgb(${item.colour[0]},${item.colour[1]},${item.colour[2]})`;
      row.append(swatch, el("span", { class: "legend-label", text: item.label }));
      wrapper.appendChild(row);
    }
    return wrapper;
  }
}

/** Compact edge label: km for large values, otherwise metres-ish. */
function formatEdge(value: number): string {
  const abs = Math.abs(value);
  if (abs >= 10000) return `${(value / 1000).toFixed(0)}k`;
  if (abs >= 1000) return `${(value / 1000).toFixed(1)}k`;
  if (abs >= 10) return value.toFixed(0);
  if (abs >= 1) return value.toFixed(1);
  if (abs === 0) return "0";
  return value.toFixed(2);
}

