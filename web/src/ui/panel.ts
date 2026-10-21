// Schema-driven parameter panel.
//
// The whole panel is generated from the JSON schema returned by the engine, so
// adding a parameter in Rust makes it appear here with no UI changes. Sliders
// use a logarithmic mapping when the schema marks a parameter as log-scaled,
// enum parameters become selects (or switches for booleans), and each row can
// be reset individually. Groups are collapsible, searchable and show live
// "modified from default" counts plus a pending-changes footer.

import type { ParamSpec, PresetSpec, Schema } from "../types";
import { clamp, formatValue } from "./format";
import { el, icon, type IconName } from "./dom";

/** Callbacks the app shell uses to react to panel edits. */
export interface PanelCallbacks {
  /** A value changed in the panel (the user may still be dragging). */
  onValuesChanged(values: Float64Array): void;
  /** The user asked to apply the current values and regenerate the world. */
  onGenerate(values: Float64Array): void;
  /** Dirty state relative to the last generated world changed. */
  onDirtyChange?(dirty: boolean, pending: number): void;
}

interface Control {
  spec: ParamSpec;
  index: number;
  group: string;
  row: HTMLElement;
  /** Re-reads the value array and refreshes this control. */
  sync(): void;
}

interface GroupView {
  key: string;
  label: string;
  section: HTMLElement;
  body: HTMLElement;
  toggle: HTMLButtonElement;
  countBadge: HTMLElement;
  modifiedBadge: HTMLElement;
  resetButton: HTMLButtonElement;
  controls: Control[];
}

/** Builds a fresh values array from schema defaults. */
export function defaultValues(schema: Schema): Float64Array {
  return Float64Array.from(schema.params.map((spec) => spec.default));
}

/** Maps schema keys to flat-array indices. */
export function valueIndex(schema: Schema): Map<string, number> {
  return new Map(schema.params.map((spec, index) => [spec.key, index]));
}

/** Applies a preset's overrides on top of the current values (a new copy). */
export function applyPreset(values: Float64Array, schema: Schema, preset: PresetSpec): Float64Array {
  const out = Float64Array.from(values);
  const index = valueIndex(schema);
  for (const [key, value] of Object.entries(preset.overrides)) {
    const at = index.get(key);
    if (at !== undefined && Number.isFinite(value)) {
      out[at] = value;
    }
  }
  return out;
}

/** True when two value arrays are equal within a tiny tolerance. */
export function valuesEqual(a: Float64Array, b: Float64Array): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i += 1) {
    const left = a[i] ?? 0;
    const right = b[i] ?? 0;
    const scale = Math.max(Math.abs(left), Math.abs(right), 1);
    if (Math.abs(left - right) > scale * 1e-9) return false;
  }
  return true;
}

/** Number of entries that differ between two value arrays. */
export function countDifferences(a: Float64Array, b: Float64Array): number {
  if (a.length !== b.length) return Math.max(a.length, b.length);
  let count = 0;
  for (let i = 0; i < a.length; i += 1) {
    const left = a[i] ?? 0;
    const right = b[i] ?? 0;
    const scale = Math.max(Math.abs(left), Math.abs(right), 1);
    if (Math.abs(left - right) > scale * 1e-9) count += 1;
  }
  return count;
}

/** Value is modified from the schema default. */
function isModified(spec: ParamSpec, value: number): boolean {
  const scale = Math.max(Math.abs(spec.default), 1);
  return Math.abs(value - spec.default) > scale * 1e-9;
}

/** Small deterministic PRNG for UI-side randomisation (not engine state). */
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

const OPEN_GROUPS_KEY = "geogenesis.groups.v3";
const DEFAULT_OPEN_GROUPS = ["world", "time"];
const JITTER_GROUPS = new Set([
  "tectonics",
  "climate",
  "geology",
  "fluvial",
  "hillslope",
  "cryosphere",
  "aeolian",
  "coastal",
  "chemistry",
  "relief",
  "hydrology",
  "ecology",
]);

function readOpenGroups(): Set<string> {
  try {
    const raw = localStorage.getItem(OPEN_GROUPS_KEY);
    if (raw === null) return new Set(DEFAULT_OPEN_GROUPS);
    const parsed = JSON.parse(raw) as unknown;
    if (Array.isArray(parsed) && parsed.every((entry) => typeof entry === "string")) {
      return new Set(parsed as string[]);
    }
  } catch {
    // Corrupt storage: fall back to defaults.
  }
  return new Set(DEFAULT_OPEN_GROUPS);
}

function writeOpenGroups(open: ReadonlySet<string>): void {
  try {
    localStorage.setItem(OPEN_GROUPS_KEY, JSON.stringify([...open]));
  } catch {
    // Private mode: persistence is optional.
  }
}

export class ParameterPanel {
  private readonly host: HTMLElement;
  private readonly root: HTMLElement;
  private readonly schema: Schema;
  private readonly presets: PresetSpec[];
  private readonly callbacks: PanelCallbacks;
  private readonly indexByKey: Map<string, number>;
  private readonly controls: Control[] = [];
  private readonly groups = new Map<string, GroupView>();
  private readonly openGroups: Set<string>;
  private readonly searchInput: HTMLInputElement;
  private readonly clearSearchButton: HTMLButtonElement;
  private readonly modifiedFilter: HTMLInputElement;
  private readonly modifiedCountBadge: HTMLElement;
  private readonly summaryCount: HTMLElement;
  private readonly summaryModified: HTMLElement;
  private readonly footerStatus: HTMLElement;
  private readonly footerDot: HTMLElement;
  private readonly generateButton: HTMLButtonElement;
  private readonly resetButton: HTMLButtonElement;
  private readonly expandButton: HTMLButtonElement;
  private readonly collapseButton: HTMLButtonElement;
  private readonly emptyState: HTMLElement;
  private preFilterOpen: Set<string> | null = null;
  private busy = false;
  private applied: Float64Array;
  private values: Float64Array;

  constructor(
    host: HTMLElement,
    schema: Schema,
    presets: PresetSpec[],
    values: Float64Array,
    callbacks: PanelCallbacks,
  ) {
    this.host = host;
    this.schema = schema;
    this.presets = presets;
    this.callbacks = callbacks;
    this.indexByKey = valueIndex(schema);
    this.values = Float64Array.from(values);
    this.applied = Float64Array.from(values);
    this.openGroups = readOpenGroups();

    host.replaceChildren();
    host.setAttribute("role", "region");
    host.setAttribute("aria-label", "Simulation parameters");
    this.root = el("div", { class: "param-panel" });
    host.appendChild(this.root);

    // --- sticky toolbar -----------------------------------------------------
    const toolbar = el("div", { class: "panel-toolbar" });

    const search = el("div", { class: "search-field" });
    search.appendChild(icon("search", 14));
    this.searchInput = el("input", {
      type: "search",
      class: "param-search",
      placeholder: "Filter 147 parameters…",
      "aria-label": "Filter parameters",
      autocomplete: "off",
      spellcheck: "false",
    });
    this.searchInput.placeholder = `Filter ${schema.params.length} parameters…`;
    this.clearSearchButton = el("button", {
      type: "button",
      class: "search-clear",
      "aria-label": "Clear parameter filter",
      hidden: true,
    });
    this.clearSearchButton.appendChild(icon("close", 12));
    search.append(this.searchInput, this.clearSearchButton);
    toolbar.appendChild(search);

    const tools = el("div", { class: "panel-tools" });
    const filterLabel = el("label", {
      class: "chip-toggle",
      "data-tip": "Show only parameters that differ from their schema default",
    });
    this.modifiedFilter = el("input", { type: "checkbox", class: "chip-toggle-input" });
    this.modifiedCountBadge = el("span", { class: "chip-count", text: "0" });
    filterLabel.append(this.modifiedFilter, el("span", { text: "Modified" }), this.modifiedCountBadge);
    tools.appendChild(filterLabel);

    this.expandButton = this.toolButton("expand", "Expand all groups", () => this.setAllOpen(true));
    this.collapseButton = this.toolButton("collapse", "Collapse all groups", () => this.setAllOpen(false));
    const resetAll = this.toolButton("reset", "Reset every parameter to its default", () => {
      this.resetDefaults();
    });
    resetAll.classList.add("tool-button-reset");
    tools.append(this.expandButton, this.collapseButton, resetAll);
    toolbar.appendChild(tools);

    this.summaryCount = el("span", { class: "summary-count" });
    this.summaryModified = el("span", { class: "summary-modified" });
    const summary = el("div", { class: "panel-summary", "aria-live": "polite" });
    summary.append(this.summaryCount, el("span", { class: "summary-sep", text: "·" }), this.summaryModified);
    toolbar.appendChild(summary);
    this.root.appendChild(toolbar);

    // --- groups -------------------------------------------------------------
    const groupsHost = el("div", { class: "param-groups" });
    this.root.appendChild(groupsHost);
    this.emptyState = el("div", { class: "panel-empty", hidden: true });
    this.emptyState.append(
      icon("search", 22),
      el("p", { text: "No parameters match this filter." }),
      el("button", { type: "button", class: "text-button", text: "Clear filter" }),
    );
    this.emptyState.querySelector("button")?.addEventListener("click", () => this.clearSearch());
    this.root.appendChild(this.emptyState);

    this.buildGroups(groupsHost);
    this.applyFilter();

    // --- sticky footer ------------------------------------------------------
    const footer = el("footer", { class: "panel-footer" });
    const status = el("div", { class: "panel-footer-status" });
    this.footerDot = el("span", { class: "status-dot", "aria-hidden": "true" });
    this.footerStatus = el("span", { class: "status-text" });
    status.append(this.footerDot, this.footerStatus);
    const actions = el("div", { class: "panel-footer-actions" });
    this.resetButton = el("button", {
      type: "button",
      class: "ghost-button",
      "data-tip": "Reset all parameters to their defaults",
    });
    this.resetButton.append(icon("reset", 13), el("span", { text: "Reset" }));
    this.resetButton.addEventListener("click", () => {
      this.resetDefaults();
    });
    this.generateButton = el("button", { type: "button", class: "primary generate-button" });
    this.generateButton.append(icon("run", 13), el("span", { text: "Generate" }));
    this.generateButton.addEventListener("click", () => {
      if (this.busy) return;
      this.callbacks.onGenerate(this.values);
    });
    actions.append(this.resetButton, this.generateButton);
    footer.append(status, actions);
    this.root.appendChild(footer);

    // --- wiring -------------------------------------------------------------
    this.searchInput.addEventListener("input", () => this.applyFilter());
    this.searchInput.addEventListener("keydown", (event) => {
      if (event.key === "Escape") {
        event.stopPropagation();
        this.clearSearch();
      }
    });
    this.clearSearchButton.addEventListener("click", () => this.clearSearch());
    this.modifiedFilter.addEventListener("change", () => this.applyFilter());
    this.refreshState();

    // Focused row stays visible while the modified filter is active.
    this.host.addEventListener("focusout", () => {
      if (this.modifiedFilter.checked) window.setTimeout(() => this.applyFilter(), 0);
    });
  }

  // --- public API -----------------------------------------------------------

  /** Current parameter values (live array, do not mutate). */
  getValues(): Float64Array {
    return this.values;
  }

  /** Replaces every value and refreshes the controls (does not emit). */
  setValues(values: Float64Array): void {
    if (values.length !== this.values.length) return;
    this.values = Float64Array.from(values);
    for (const control of this.controls) control.sync();
    this.refreshState();
  }

  /** Marks a parameter set as the one currently rendered by the engine. */
  setApplied(values: Float64Array): void {
    this.applied = Float64Array.from(values);
    this.refreshState();
    // Keep the shell's dirty indicator in sync: a successful generate applies
    // the current values, so previously pending edits are no longer pending.
    const pending = countDifferences(this.values, this.applied);
    this.callbacks.onDirtyChange?.(pending > 0, pending);
  }

  /** True when the panel differs from the last generated world. */
  isDirty(): boolean {
    return !valuesEqual(this.values, this.applied);
  }

  /**
   * Applies externally computed values (for example a terrain suggestion) and
   * marks them as pending changes, exactly like a user edit would.
   */
  applyExternalValues(values: Float64Array): Float64Array {
    this.setValues(values);
    this.emitChanged();
    return this.values;
  }

  /** Applies a preset to the panel (does not regenerate the world). */
  applyPreset(preset: PresetSpec): Float64Array {
    this.setValues(applyPreset(this.values, this.schema, preset));
    this.emitChanged();
    return this.values;
  }

  /** Resets everything to schema defaults. */
  resetDefaults(): Float64Array {
    this.setValues(defaultValues(this.schema));
    this.emitChanged();
    return this.values;
  }

  /** Resets one group to defaults. */
  resetGroup(key: string): void {
    const group = this.groups.get(key);
    if (group === undefined) return;
    for (const control of group.controls) {
      const at = control.index;
      if (at >= 0) this.values[at] = control.spec.default;
    }
    for (const control of group.controls) control.sync();
    this.emitChanged();
  }

  /**
   * Produces a coherent random world: a random preset, a biased jitter on the
   * physical parameters and a new seed. Keeping the current grid resolution
   * means randomisation stays fast and predictable.
   */
  randomize(): Float64Array {
    const next = Float64Array.from(this.values);
    const preset = this.presets[Math.floor(Math.random() * this.presets.length)];
    if (preset !== undefined) {
      for (const [key, value] of Object.entries(preset.overrides)) {
        const at = this.indexByKey.get(key);
        if (at !== undefined && Number.isFinite(value)) {
          next[at] = value;
        }
      }
    }
    const rng = makeRng((Math.random() * 0xffffffff) >>> 0);
    for (let i = 0; i < this.schema.params.length; i += 1) {
      const spec = this.schema.params[i];
      if (spec === undefined) continue;
      if (spec.key === "seed" || spec.key === "width" || spec.key === "height") continue;
      if (!JITTER_GROUPS.has(spec.group) || spec.kind === "enum") continue;
      const current = next[i] ?? spec.default;
      if (spec.scale === "log" && current > 0) {
        const factor = Math.exp((rng() - 0.5) * 0.7);
        next[i] = clamp(current * factor, spec.min, spec.max);
      } else {
        const span = spec.max - spec.min;
        const delta = (rng() - 0.5) * span * 0.12;
        next[i] = clamp(current + delta, spec.min, spec.max);
        if (spec.kind === "int") next[i] = Math.round(next[i] ?? current);
      }
    }
    const seedAt = this.indexByKey.get("seed");
    if (seedAt !== undefined) {
      next[seedAt] = Math.floor(Math.random() * 0xffffffff);
    }
    this.setValues(next);
    this.emitChanged();
    return this.values;
  }

  /** Randomises only the seed value (does not regenerate). */
  rerollSeed(): void {
    const at = this.indexByKey.get("seed");
    const spec = at === undefined ? undefined : this.schema.params[at];
    if (at === undefined || spec === undefined) return;
    this.values[at] = Math.floor(Math.random() * 0xffffffff);
    for (const control of this.controls) {
      if (control.index === at) control.sync();
    }
    this.emitChanged();
  }

  /** Enables/disables the actions that start engine work. */
  setBusy(busy: boolean, activity?: string): void {
    this.busy = busy;
    this.root.classList.toggle("is-busy", busy);
    this.host.setAttribute("aria-busy", busy ? "true" : "false");
    this.generateButton.disabled = busy;
    this.generateButton.classList.toggle("is-busy", busy);
    const label = this.generateButton.querySelector("span");
    if (label !== null) label.textContent = busy ? (activity ?? "Working…") : "Generate";
  }

  /** Focuses the filter box (keyboard shortcut). */
  focusSearch(): void {
    this.searchInput.focus();
    this.searchInput.select();
  }

  // --- construction ---------------------------------------------------------

  private toolButton(name: IconName, tip: string, action: () => void): HTMLButtonElement {
    const button = el("button", {
      type: "button",
      class: "tool-button",
      "aria-label": tip,
      "data-tip": tip,
    });
    button.appendChild(icon(name, 14));
    button.addEventListener("click", action);
    return button;
  }

  private buildGroups(host: HTMLElement): void {
    const paramsByGroup = new Map<string, ParamSpec[]>();
    for (const spec of this.schema.params) {
      const list = paramsByGroup.get(spec.group);
      if (list === undefined) paramsByGroup.set(spec.group, [spec]);
      else list.push(spec);
    }
    for (const group of this.schema.groups) {
      const params = paramsByGroup.get(group.key);
      if (params === undefined || params.length === 0) continue;
      this.groups.set(group.key, this.buildGroup(group.key, group.label, group.label_ja, params, host));
    }
    // Any parameters in groups the schema did not list still need a home.
    for (const [key, params] of paramsByGroup) {
      if (this.groups.has(key)) continue;
      this.groups.set(key, this.buildGroup(key, key, "", params, host));
    }
  }

  private buildGroup(
    key: string,
    label: string,
    labelJa: string,
    params: ParamSpec[],
    host: HTMLElement,
  ): GroupView {
    const section = el("section", { class: "param-group", "data-group": key });
    const head = el("header", { class: "param-group-head" });
    const toggle = el("button", {
      type: "button",
      class: "group-toggle",
      "aria-expanded": "false",
      "aria-controls": `param-group-${key}`,
      "data-tip": labelJa,
    });
    const chevron = el("span", { class: "group-chevron", "aria-hidden": "true" });
    chevron.appendChild(icon("chevron-right", 13));
    const countBadge = el("span", { class: "group-count", text: String(params.length) });
    toggle.append(chevron, el("span", { class: "group-title", text: label }), countBadge);
    const modifiedBadge = el("span", { class: "group-modified", hidden: true });
    const resetButton = el("button", {
      type: "button",
      class: "group-reset",
      "aria-label": `Reset ${label} group to defaults`,
      "data-tip": `Reset ${label} to defaults`,
      hidden: true,
    });
    resetButton.appendChild(icon("reset", 12));
    head.append(toggle, modifiedBadge, resetButton);
    section.appendChild(head);

    const body = el("div", { class: "param-body", id: `param-group-${key}` });
    const controls: Control[] = [];
    for (const spec of params) {
      const control = this.buildControl(spec, body);
      controls.push(control);
      this.controls.push(control);
    }
    section.appendChild(body);
    host.appendChild(section);

    const group: GroupView = {
      key,
      label,
      section,
      body,
      toggle,
      countBadge,
      modifiedBadge,
      resetButton,
      controls,
    };
    toggle.addEventListener("click", () => {
      this.setGroupOpen(group, !this.openGroups.has(key));
    });
    resetButton.addEventListener("click", (event) => {
      event.stopPropagation();
      this.resetGroup(key);
    });
    this.setGroupOpen(group, this.openGroups.has(key), false);
    return group;
  }

  private buildControl(spec: ParamSpec, body: HTMLElement): Control {
    const index = this.indexByKey.get(spec.key) ?? -1;
    const group = spec.group;
    const row = el("div", { class: "param", "data-key": spec.key });
    const head = el("div", { class: "param-head" });
    const inputId = `param-${spec.key}`;
    const helpId = `param-help-${spec.key}`;

    const label = el("label", { class: "param-label", for: inputId });
    label.append(
      el("span", { class: "mod-dot", "aria-hidden": "true", "data-tip": "Modified from default" }),
      el("span", { class: "param-name", text: spec.label }),
    );
    if (spec.unit.length > 0) {
      label.appendChild(el("span", { class: "param-unit", text: spec.unit }));
    }
    head.appendChild(label);

    const help = el("button", {
      type: "button",
      class: "param-help",
      "aria-label": `What does ${spec.label} do?`,
      "data-tip": `${spec.help}${spec.label_ja.length > 0 ? ` — ${spec.label_ja}` : ""}`,
      tabindex: "-1",
    });
    help.appendChild(icon("info", 12));
    head.appendChild(help);

    const reset = el("button", {
      type: "button",
      class: "param-reset",
      "aria-label": `Reset ${spec.label} to default`,
      "data-tip": `Reset to default (${formatValue(spec, spec.default)})`,
      tabindex: "-1",
    });
    reset.appendChild(icon("reset", 11));
    reset.addEventListener("click", () => {
      this.setValue(index, spec.default);
      control.sync();
      this.emitChanged();
    });
    head.appendChild(reset);

    const helpText = el("span", { class: "visually-hidden", id: helpId, text: spec.help });
    row.appendChild(helpText);

    const controlArea = el("div", { class: "param-control" });
    let sync: () => void;

    if (spec.kind === "enum" && spec.options?.length === 2 && spec.options[0] === "Off" && spec.options[1] === "On") {
      // Boolean switch.
      const toggle = el("label", { class: "switch" });
      const input = el("input", {
        type: "checkbox",
        id: inputId,
        "aria-describedby": helpId,
        class: "switch-input",
      });
      const track = el("span", { class: "switch-track", "aria-hidden": "true" });
      const text = el("span", { class: "switch-text" });
      toggle.append(input, track, text);
      controlArea.appendChild(toggle);
      input.addEventListener("change", () => {
        this.setValue(index, input.checked ? 1 : 0);
        this.emitChanged();
      });
      sync = () => {
        const value = this.valueAt(index, spec);
        input.checked = value >= 0.5;
        text.textContent = input.checked ? (spec.options?.[1] ?? "On") : (spec.options?.[0] ?? "Off");
      };
    } else if (spec.kind === "enum") {
      const select = el("select", { class: "param-select", id: inputId, "aria-describedby": helpId });
      for (const [i, option] of (spec.options ?? []).entries()) {
        const optionEl = el("option", { value: String(i), text: option });
        if (option === spec.options?.[Math.round(spec.default)]) optionEl.selected = true;
        select.appendChild(optionEl);
      }
      controlArea.appendChild(select);
      select.addEventListener("change", () => {
        this.setValue(index, Number(select.value));
        this.emitChanged();
      });
      sync = () => {
        select.value = String(Math.round(this.valueAt(index, spec)));
      };
    } else {
      const valueInput = el("input", {
        type: "text",
        class: "param-value",
        id: inputId,
        inputmode: "decimal",
        spellcheck: "false",
        autocomplete: "off",
        "aria-describedby": helpId,
        "aria-label": `${spec.label} value${spec.unit ? ` in ${spec.unit}` : ""}`,
      });
      head.appendChild(valueInput);

      const commitText = (): void => {
        const parsed = Number.parseFloat(valueInput.value.replaceAll(",", ""));
        if (!Number.isFinite(parsed)) {
          sync();
          return;
        }
        let value = clamp(parsed, spec.min, spec.max);
        if (spec.kind === "int") value = Math.round(value);
        this.setValue(index, value);
        sync();
        this.emitChanged();
      };
      valueInput.addEventListener("focus", () => valueInput.select());
      valueInput.addEventListener("change", commitText);
      valueInput.addEventListener("keydown", (event) => {
        if (event.key === "Enter") {
          event.preventDefault();
          commitText();
          valueInput.blur();
        } else if (event.key === "Escape") {
          event.preventDefault();
          sync();
          valueInput.blur();
        }
      });

      if (spec.key === "seed") {
        // Seeds span 2^53: a slider would be meaningless, so offer a dice roll.
        const dice = el("button", {
          type: "button",
          class: "seed-dice",
          "aria-label": "Roll a new random seed",
          "data-tip": "Roll a new random seed",
        });
        dice.appendChild(icon("dice", 14));
        dice.addEventListener("click", () => {
          this.rerollSeed();
        });
        controlArea.appendChild(dice);
        sync = () => {
          valueInput.value = String(Math.round(this.valueAt(index, spec)));
        };
      } else {
        const slider = el("input", {
          type: "range",
          class: "param-slider",
          id: `${inputId}-slider`,
          min: "0",
          max: "1",
          step: "0.0005",
          tabindex: "-1",
          "aria-label": `${spec.label} slider`,
          "aria-describedby": helpId,
        });
        const toPosition = (value: number): number => {
          if (spec.scale === "log") {
            const lo = Math.log10(Math.max(spec.min, 1e-12));
            const hi = Math.log10(Math.max(spec.max, spec.min * 1.0001));
            return clamp((Math.log10(Math.max(value, 1e-12)) - lo) / Math.max(hi - lo, 1e-12), 0, 1);
          }
          return clamp((value - spec.min) / Math.max(spec.max - spec.min, 1e-12), 0, 1);
        };
        const fromPosition = (position: number): number => {
          if (spec.scale === "log") {
            const lo = Math.log10(Math.max(spec.min, 1e-12));
            const hi = Math.log10(Math.max(spec.max, spec.min * 1.0001));
            return 10 ** (lo + (hi - lo) * position);
          }
          const raw = spec.min + (spec.max - spec.min) * position;
          return spec.kind === "int" ? Math.round(raw) : raw;
        };
        const fill = (position: number): void => {
          slider.style.setProperty("--pct", `${(position * 100).toFixed(2)}%`);
        };
        slider.addEventListener("input", () => {
          const position = Number(slider.value);
          const value = fromPosition(position);
          fill(position);
          this.setValue(index, value);
          valueInput.value = formatValue(spec, value);
          slider.setAttribute("aria-valuetext", formatValue(spec, value));
          this.emitChanged();
        });
        controlArea.appendChild(slider);
        sync = () => {
          const value = this.valueAt(index, spec);
          const safe = clamp(value, spec.min, spec.max);
          const position = toPosition(safe);
          slider.value = String(position);
          fill(position);
          valueInput.value = formatValue(spec, safe);
          slider.setAttribute("aria-valuetext", formatValue(spec, safe));
        };
      }
    }

    row.appendChild(head);
    row.appendChild(controlArea);
    body.appendChild(row);

    const control: Control = {
      spec,
      index,
      group,
      row,
      sync,
    };
    return control;
  }

  // --- state helpers --------------------------------------------------------

  private valueAt(index: number, spec: ParamSpec): number {
    return this.values[index] ?? spec.default;
  }

  private setValue(index: number, value: number): void {
    if (index < 0) return;
    this.values[index] = value;
  }

  private emitChanged(): void {
    this.refreshState();
    this.applyFilter();
    this.callbacks.onValuesChanged(this.values);
    this.callbacks.onDirtyChange?.(this.isDirty(), countDifferences(this.values, this.applied));
  }

  /** Recomputes modified markers, group badges and footer state. */
  private refreshState(): void {
    const groupCounts = new Map<string, number>();
    let modifiedTotal = 0;
    for (const control of this.controls) {
      const value = this.valueAt(control.index, control.spec);
      const modified = isModified(control.spec, value);
      control.row.classList.toggle("modified", modified);
      if (modified) {
        modifiedTotal += 1;
        groupCounts.set(control.group, (groupCounts.get(control.group) ?? 0) + 1);
      }
    }
    for (const group of this.groups.values()) {
      const count = groupCounts.get(group.key) ?? 0;
      group.section.classList.toggle("has-modified", count > 0);
      group.modifiedBadge.hidden = count === 0;
      group.modifiedBadge.textContent = `${count}`;
      group.modifiedBadge.setAttribute("aria-label", `${count} modified`);
      group.resetButton.hidden = count === 0;
    }
    this.summaryCount.textContent = `${this.controls.length} parameters`;
    this.summaryModified.textContent =
      modifiedTotal === 0 ? "no changes" : `${modifiedTotal} modified from default`;
    this.modifiedCountBadge.textContent = String(modifiedTotal);
    this.modifiedCountBadge.classList.toggle("is-active", modifiedTotal > 0);

    const pending = countDifferences(this.values, this.applied);
    const dirty = pending > 0;
    this.footerStatus.textContent = dirty
      ? `${pending} pending change${pending === 1 ? "" : "s"}`
      : "All changes applied";
    this.footerDot.classList.toggle("is-pending", dirty);
    this.footerStatus.classList.toggle("is-pending", dirty);
    this.generateButton.classList.toggle("dirty", dirty);
    this.generateButton.disabled = this.busy;
    this.resetButton.disabled = modifiedTotal === 0 && pending === 0;
    if (!this.busy) {
      const label = this.generateButton.querySelector("span");
      if (label !== null) label.textContent = dirty ? "Generate" : "Regenerate";
      this.generateButton.setAttribute(
        "aria-label",
        dirty ? "Generate world with modified parameters" : "Regenerate the current world",
      );
    }
    this.host.classList.toggle("has-pending", dirty);
  }

  // --- filtering ------------------------------------------------------------

  private applyFilter(): void {
    const query = this.searchInput.value.trim().toLowerCase();
    const modifiedOnly = this.modifiedFilter.checked;
    const searching = query.length > 0;
    const filtering = searching || modifiedOnly;
    if (filtering && this.preFilterOpen === null) {
      this.preFilterOpen = new Set(this.openGroups);
    }
    this.clearSearchButton.hidden = !searching;
    const matches: Control[] = [];
    for (const control of this.controls) {
      const spec = control.spec;
      const haystack = `${spec.key} ${spec.label} ${spec.label_ja} ${spec.help} ${spec.group}`.toLowerCase();
      const textMatch = !searching || haystack.includes(query);
      const modified = isModified(spec, this.valueAt(control.index, spec));
      const modifiedMatch = !modifiedOnly || modified;
      control.row.hidden = !(textMatch && modifiedMatch);
      if (!control.row.hidden) matches.push(control);
    }
    for (const group of this.groups.values()) {
      const visible = group.controls.filter((control) => !control.row.hidden);
      group.section.hidden = visible.length === 0;
      group.countBadge.textContent = String(visible.length);
      group.countBadge.title = `${visible.length} of ${group.controls.length} shown`;
      if (filtering && visible.length > 0 && !this.openGroups.has(group.key)) {
        this.setGroupOpen(group, true, false);
      }
    }
    this.emptyState.hidden = matches.length > 0;
    if (searching) {
      this.summaryCount.textContent = `${matches.length} match${matches.length === 1 ? "" : "es"}`;
    }
    if (!filtering && this.preFilterOpen !== null) {
      for (const group of this.groups.values()) {
        this.setGroupOpen(group, this.preFilterOpen.has(group.key), false);
      }
      this.preFilterOpen = null;
      writeOpenGroups(this.openGroups);
    }
  }

  private clearSearch(): void {
    this.searchInput.value = "";
    this.modifiedFilter.checked = false;
    this.applyFilter();
    this.searchInput.focus();
  }

  private setGroupOpen(group: GroupView, open: boolean, persist = true): void {
    if (open) this.openGroups.add(group.key);
    else this.openGroups.delete(group.key);
    group.body.hidden = !open;
    group.toggle.setAttribute("aria-expanded", open ? "true" : "false");
    group.section.classList.toggle("is-open", open);
    if (persist) writeOpenGroups(this.openGroups);
  }

  private setAllOpen(open: boolean): void {
    for (const group of this.groups.values()) this.setGroupOpen(group, open, false);
    writeOpenGroups(this.openGroups);
  }
}
