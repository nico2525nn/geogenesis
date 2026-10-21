// GeoGenesis app shell: boots the engine worker, wires the UI modules and
// translates user intent into engine requests.
//
// The engine worker executes exactly one request at a time, so this module
// tracks a single in-flight operation and locks only the actions that would
// queue behind it. Parameter edits stay possible while the world is running;
// a pending generate/seek is replayed as soon as the current operation ends.

import type {
  BiomeInfo,
  ColorMode,
  EngineState,
  EngineStats,
  MineralInfo,
  MineralsReport,
  PresetSpec,
  ProfileLine,
  ProfileSample,
  RockInfo,
  Schema,
  TerrainData,
  WorkerRequest,
  WorkerResponse,
} from "./types";
import { COLOR_MODE_LABELS } from "./types";
import { defaultValues, ParameterPanel, valueIndex, valuesEqual } from "./ui/panel";
import { Timeline } from "./ui/timeline";
import { StatsPanel } from "./ui/stats";
import { ProfilePanel } from "./ui/profile";
import { Legend } from "./ui/legend";
import { Toasts } from "./ui/toast";
import { BootOverlay } from "./ui/boot";
import {
  downloadBytes,
  downloadDataUrl,
  downloadText,
  enrichParamsJson,
  enrichStatsJson,
  exportFilename,
  setupExportMenu,
  type ExportMeta,
} from "./ui/export";
import { clamp, formatDuration, formatPercent, formatSigned } from "./ui/format";
import { el, icon, initTooltips, setText, type IconName } from "./ui/dom";
import { FpsMeter } from "./ui/fps";
import { buildProjectJson, parseProjectJson, type ProjectGeology } from "./ui/project";
import {
  describeGeology,
  geologyProjectPayload,
  parseRocksJson,
  parseStrataJson,
  readStoredGeology,
  writeStoredGeology,
} from "./ui/geology";
import {
  buildSuggestionCandidates,
  describeSuggestion,
  scoreSuggestion,
  type SuggestionMetrics,
} from "./ui/suggest";
import { TerrainView } from "./view/terrain";

const STORAGE_PARAMS = "geogenesis.params.v2";
const STORAGE_MODE = "geogenesis.mode.v2";

/** Engine operations that occupy the worker queue. */
type Operation = "generate" | "step" | "seek" | "run" | "export" | "suggest" | null;

/** Result of starting a generation (busy means it was queued for later). */
type GenerateOutcome = "ok" | "queued" | "error";

const EXAGGERATION_NEUTRAL = 60;
const EXAGGERATION_MIN = 10;
const EXAGGERATION_MAX = 120;
const EXAGGERATION_STEP = 10;
const PLAY_INTERVAL_MS = 110;

function byId<T extends HTMLElement>(id: string): T {
  const element = document.getElementById(id);
  if (element === null) {
    throw new Error(`missing element #${id}`);
  }
  return element as T;
}

// ---------------------------------------------------------------------------
// DOM handles
// ---------------------------------------------------------------------------

const app = byId<HTMLDivElement>("app");
const viewport = byId<HTMLDivElement>("viewport");
const minimap = byId<HTMLCanvasElement>("minimap");
const timelineHost = byId<HTMLDivElement>("timeline-host");
const panelHost = byId<HTMLDivElement>("panel-host");
const statsHost = byId<HTMLDivElement>("stats-host");
const legendHost = byId<HTMLDivElement>("legend-host");
const profileCanvas = byId<HTMLCanvasElement>("profile-canvas");
const sectionHead = byId<HTMLElement>("section-head");
const modeSelect = byId<HTMLSelectElement>("mode-select");
const presetSelect = byId<HTMLSelectElement>("preset-select");
const presetDescription = byId<HTMLParagraphElement>("preset-description");
const topbarPreset = byId<HTMLSpanElement>("topbar-preset");
const generateButton = byId<HTMLButtonElement>("btn-regenerate");
const randomizeButton = byId<HTMLButtonElement>("btn-randomize");
const seedButton = byId<HTMLButtonElement>("btn-seed");
const geologyButton = byId<HTMLButtonElement>("btn-geology");
const geologyStatus = byId<HTMLParagraphElement>("geology-status");
const geologyDialog = byId<HTMLDivElement>("geology-dialog");
const geologyClose = byId<HTMLButtonElement>("geology-close");
const geologyDone = byId<HTMLButtonElement>("geology-done");
const geologyClearButton = byId<HTMLButtonElement>("geology-clear");
const geologyDialogStatus = byId<HTMLParagraphElement>("geology-dialog-status");
const rocksFileInput = byId<HTMLInputElement>("rocks-file");
const strataFileInput = byId<HTMLInputElement>("strata-file");
const rocksFileStatus = byId<HTMLSpanElement>("rocks-status");
const strataFileStatus = byId<HTMLSpanElement>("strata-status");
const exportButton = byId<HTMLButtonElement>("btn-export");
const exportMenu = byId<HTMLDivElement>("export-items");
const suggestButton = byId<HTMLButtonElement>("btn-suggest");
const suggestionBar = byId<HTMLDivElement>("suggestion-bar");
const suggestionTitle = byId<HTMLSpanElement>("suggestion-title");
const suggestionDetail = byId<HTMLSpanElement>("suggestion-detail");
const suggestionApplyButton = byId<HTMLButtonElement>("suggestion-apply");
const suggestionDismissButton = byId<HTMLButtonElement>("suggestion-dismiss");
const projectFileInput = byId<HTMLInputElement>("project-file");
const paramsButton = byId<HTMLButtonElement>("btn-params");
const statsButton = byId<HTMLButtonElement>("btn-stats");
const helpButton = byId<HTMLButtonElement>("btn-help");
const helpDialog = byId<HTMLDivElement>("help-dialog");
const helpClose = byId<HTMLButtonElement>("help-close");
const drawerBackdrop = byId<HTMLDivElement>("drawer-backdrop");
const resetViewButton = byId<HTMLButtonElement>("btn-reset-view");
const exagDown = byId<HTMLButtonElement>("btn-exag-down");
const exagUp = byId<HTMLButtonElement>("btn-exag-up");
const exagValue = byId<HTMLOutputElement>("exag-value");
const hudTime = byId<HTMLSpanElement>("hud-time");
const hudSea = byId<HTMLSpanElement>("hud-sea");
const hudLand = byId<HTMLSpanElement>("hud-land");
const hudFps = byId<HTMLSpanElement>("hud-fps");
const hudStatus = byId<HTMLSpanElement>("hud-status");
const hudProgress = byId<HTMLDivElement>("hud-progress");
const hudProgressFill = byId<HTMLSpanElement>("hud-progress-fill");
const toastHost = byId<HTMLDivElement>("toast");
const bootLayer = byId<HTMLDivElement>("boot-layer");
const bootRetry = byId<HTMLButtonElement>("boot-retry");

// ---------------------------------------------------------------------------
// Module state
// ---------------------------------------------------------------------------

let schema: Schema | null = null;
let presets: PresetSpec[] = [];
let biomes: readonly BiomeInfo[] = [];
let rocks: readonly RockInfo[] = [];
let panel: ParameterPanel | null = null;
let timeline: Timeline | null = null;
let stats: StatsPanel | null = null;
let profile: ProfilePanel | null = null;
let legend: Legend | null = null;
let view: TerrainView | null = null;
let bootOverlay: BootOverlay | null = null;
let toasts: Toasts | null = null;

let engineState: EngineState | null = null;
let currentTerrain: TerrainData | null = null;
let currentProfile: ProfileSample[] = [];
let generatedValues: Float64Array | null = null;
let generatedPreset: PresetSpec | null = null;
let generatedPresetKey: string | null = null;
let generatedPresetLabel = "custom";
let currentPreset: PresetSpec | null = null;
let profileLine: ProfileLine | null = null;
let mode: ColorMode = "terrain";

let inFlight: Operation = null;
let pendingGenerate: Float64Array | null = null;
let pendingSeek: number | null = null;
let playing = false;
let playTimer = 0;
let stepsPerTick = 2;
let stepCounter = 0;
let exaggeration = EXAGGERATION_NEUTRAL;
let helpOpen = false;
let lastFocusedBeforeHelp: Element | null = null;

let profileTimer = 0;
let fieldToken = 0;
let statsToken = 0;
let profileToken = 0;
let previewToken = 0;
/** Debounce timer for the low-resolution parameter preview. */
let paramPreviewTimer: number | null = null;
/** Invalidates in-flight previews when parameters change again or a generate starts. */
let paramPreviewToken = 0;

/** Rolling HUD frame-rate meter. */
let fpsMeter: FpsMeter | null = null;
/** Candidate parameters of the currently previewed terrain suggestion. */
let suggestionValues: Float64Array | null = null;
/** Metrics of the currently previewed terrain suggestion. */
let suggestionMetrics: SuggestionMetrics | null = null;
/** True while the viewport shows a suggestion preview instead of the live world. */
let suggestionActive = false;
/** Set while a project file is being applied (prevents concurrent loads). */
let loadingProject = false;
const SUGGESTION_COUNT = 8;

/** Mineral catalogue + latest survey report for the current world. */
let minerals: readonly MineralInfo[] = [];
let mineralsReport: MineralsReport | null = null;
let mineralsTimer: number | null = null;
let mineralsToken = 0;
let lastMineralsAt = 0;

/** Raw user-defined geology documents passed to the next generate request. */
let rocksJson: string | null = null;
let strataJson: string | null = null;
/** Human summary shown in the sidebar while custom geology is active. */
let geologyLabel: string | null = null;
let geologyDialogOpen = false;
let lastFocusedBeforeGeology: Element | null = null;

const pending = new Map<number, { resolve: (response: WorkerResponse) => void; reject: (error: Error) => void }>();
let requestId = 1;
const worker = new Worker(new URL("./engine/worker.ts", import.meta.url), { type: "module" });

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

function setStatus(text: string, isError = false): void {
  setText(hudStatus, text);
  hudStatus.classList.toggle("is-error", isError);
}

function setHudProgress(fraction: number | null, visible: boolean): void {
  hudProgress.hidden = !visible;
  hudProgress.classList.toggle("is-indeterminate", fraction === null);
  if (fraction === null) {
    hudProgressFill.style.width = "";
  } else {
    hudProgressFill.style.width = `${(clamp(fraction, 0, 1) * 100).toFixed(1)}%`;
  }
}

function hydrateIcons(): void {
  for (const node of document.querySelectorAll<HTMLElement>("[data-icon]")) {
    const name = node.dataset.icon as IconName | undefined;
    if (name !== undefined) node.replaceChildren(icon(name, 16));
  }
}

function send<T extends WorkerResponse>(request: WorkerRequest): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    pending.set(request.id, {
      resolve: (response) => resolve(response as T),
      reject,
    });
    worker.postMessage(request);
  });
}

worker.onmessage = (event: MessageEvent<WorkerResponse>) => {
  const response = event.data;
  if (response.kind === "progress") {
    applyState(response.state);
    if (response.terrain !== undefined) {
      currentTerrain = response.terrain;
      view?.show(response.terrain);
      // Keep the measured statistics frozen while a long operation runs; the
      // histogram would otherwise disagree with the last full stats report.
    }
    const fraction = clamp(
      response.state.progress > 0
        ? response.state.progress
        : response.state.elapsed_yr / Math.max(response.state.total_yr, 1),
      0,
      1,
    );
    setHudProgress(fraction, true);
    const label = response.phase === "seek" ? "seeking" : "running";
    timeline?.setActivity(`${label} · ${Math.round(fraction * 100)}%`);
    setStatus(
      `${label} · ${formatDuration(response.state.elapsed_yr)} / ${formatDuration(response.state.total_yr)}`,
    );
    return;
  }
  const entry = pending.get(response.id);
  if (entry === undefined) {
    if (response.kind === "error") {
      toasts?.error(response.message);
      setStatus(response.message, true);
    }
    return;
  }
  pending.delete(response.id);
  if (response.kind === "error") {
    entry.reject(new Error(response.message));
  } else {
    entry.resolve(response);
  }
};

worker.onerror = (event) => {
  const message = event.message.length > 0 ? event.message : "engine worker crashed";
  setStatus(message, true);
  toasts?.error(`Engine error: ${message}`);
  inFlight = null;
  updateUiLock();
  bootOverlay?.showError(message, () => window.location.reload());
};

// ---------------------------------------------------------------------------
// State application
// ---------------------------------------------------------------------------

/** Land fraction of the currently displayed terrain (matches the stats definition). */
function terrainLandFraction(terrain: TerrainData | null): number | null {
  if (terrain === null || terrain.elevation.length === 0) return null;
  let land = 0;
  const elevation = terrain.elevation;
  for (let i = 0; i < elevation.length; i += 1) {
    if ((elevation[i] ?? 0) > terrain.seaLevel) land += 1;
  }
  return land / elevation.length;
}

function refreshHudLand(): void {
  const fraction = terrainLandFraction(currentTerrain);
  setText(hudLand, fraction === null ? "" : `land ${formatPercent(fraction)}`);
}

function applyState(state: EngineState): void {
  engineState = state;
  timeline?.update(state);
  stats?.setLiveState(state);
  setText(hudTime, `t = ${formatDuration(state.elapsed_yr)}`);
  setText(hudSea, `sea ${formatSigned(state.sea_level_m, 0)} m`);
  refreshHudLand();
}

/** Applies the current operation state to every control. */
function updateUiLock(): void {
  const heavy =
    inFlight === "generate" ||
    inFlight === "seek" ||
    inFlight === "run" ||
    inFlight === "export" ||
    inFlight === "suggest";
  const any = inFlight !== null;
  const generating = inFlight === "generate";
  generateButton.disabled = heavy;
  randomizeButton.disabled = any;
  seedButton.disabled = any;
  suggestButton.disabled = any || engineState === null;
  geologyButton.disabled = heavy;
  suggestionApplyButton.disabled = any;
  suggestionDismissButton.disabled = any;
  presetSelect.disabled = heavy;
  modeSelect.disabled = heavy;
  exportButton.disabled = heavy || engineState === null;
  timeline?.setBusy(heavy);
  app.classList.toggle("is-locked", any);
  app.classList.toggle("is-generating", generating);
  const label = generateButton.querySelector<HTMLElement>(".button-text");
  if (label !== null) label.textContent = generating ? "Generating…" : "Generate";
  generateButton.classList.toggle("is-busy", generating);
  const badgeLabel =
    generating
      ? "generating"
      : inFlight === "run"
        ? "running"
        : inFlight === "seek"
          ? "seeking"
          : inFlight === "export"
            ? "exporting"
            : inFlight === "suggest"
              ? "suggesting"
              : "updating";
  panel?.setBusy(heavy, badgeLabel.charAt(0).toUpperCase() + badgeLabel.slice(1) + "…");
  const statsLabel = inFlight === "run" || inFlight === "seek" ? "measuring…" : badgeLabel;
  stats?.setRunning(heavy && inFlight !== "export", statsLabel);
  if (heavy) setHudProgress(inFlight === "generate" ? null : 0.02, true);
  else if (inFlight === null) setHudProgress(0, false);
}

/** Resolves once no engine operation is in flight (bounded wait). */
async function waitForIdle(timeoutMs = 15000): Promise<boolean> {
  const start = performance.now();
  while (inFlight !== null) {
    if (performance.now() - start > timeoutMs) return false;
    await new Promise<void>((resolve) => {
      window.setTimeout(resolve, 60);
    });
  }
  return true;
}

function flushQueued(): void {
  const nextGenerate = pendingGenerate;
  const nextSeek = pendingSeek;
  pendingGenerate = null;
  pendingSeek = null;
  if (nextGenerate !== null) {
    void generate(nextGenerate);
  } else if (nextSeek !== null) {
    void seek(nextSeek);
  }
}

// ---------------------------------------------------------------------------
// Preset / dirty bookkeeping
// ---------------------------------------------------------------------------

function findPreset(values: Float64Array): PresetSpec | null {
  if (schema === null) return null;
  const index = valueIndex(schema);
  for (const preset of presets) {
    const entries = Object.entries(preset.overrides);
    const matches = entries.every(([key, expected]) => {
      const at = index.get(key);
      if (at === undefined) return true;
      const actual = values[at] ?? Number.NaN;
      return Math.abs(actual - expected) <= Math.max(Math.abs(expected), 1) * 1e-9;
    });
    if (matches) return preset;
  }
  return null;
}

function isDefaultValues(values: Float64Array): boolean {
  return schema !== null && valuesEqual(values, defaultValues(schema));
}

function syncPresetUi(values: Float64Array): void {
  const match = findPreset(values);
  const isDefault = match === null && isDefaultValues(values);
  currentPreset = match;
  if (match !== null) {
    presetSelect.value = match.key;
    setText(presetDescription, `${match.description} · ${Object.keys(match.overrides).length} parameter overrides`);
    setText(topbarPreset, match.label);
  } else if (isDefault) {
    presetSelect.value = "__default";
    setText(
      presetDescription,
      "Schema defaults for a neutral world. Choose a preset for a recognisable archetype.",
    );
    setText(topbarPreset, "Defaults");
  } else {
    presetSelect.value = "__custom";
    setText(
      presetDescription,
      "Custom parameter set — tweaked values no longer match a preset. Generate to apply them.",
    );
    setText(topbarPreset, "Custom");
  }
  topbarPreset.classList.toggle("is-custom", match === null && !isDefault);
}

// ---------------------------------------------------------------------------
// Display mode
// ---------------------------------------------------------------------------

/** Applies a colour/overlay mode everywhere and refreshes its field. */
function applyMode(next: ColorMode, announce = true, refresh = true): void {
  mode = next;
  modeSelect.value = next;
  try {
    localStorage.setItem(STORAGE_MODE, next);
  } catch {
    // Ignore storage failures.
  }
  if (suggestionActive) clearSuggestion(true);
  view?.setMode(next);
  legend?.setMode(next);
  const cached = currentTerrain?.fields?.[next];
  if (cached !== undefined) view?.setField(next, cached);
  if (refresh) void refreshField();
  if (isMineralMode(next)) scheduleMineralsRefresh(0);
  if (announce) setStatus(`overlay: ${modeSelect.selectedOptions[0]?.textContent ?? next}`);
}

// ---------------------------------------------------------------------------
// Low-resolution parameter preview
//
// While the user edits parameters we render a coarse (128-cell) version of the
// same world so slider changes give immediate visual feedback; the full world
// is only rebuilt when Generate is pressed. The preview request does not touch
// the live simulation on the worker side.
// ---------------------------------------------------------------------------

function cancelParamPreview(): void {
  if (paramPreviewTimer !== null) {
    window.clearTimeout(paramPreviewTimer);
    paramPreviewTimer = null;
  }
  paramPreviewToken += 1;
}

function scheduleParamPreview(values: Float64Array): void {
  cancelParamPreview();
  if (schema === null || inFlight !== null || mode !== "terrain") return;
  const token = paramPreviewToken;
  paramPreviewTimer = window.setTimeout(() => {
    paramPreviewTimer = null;
    if (token !== paramPreviewToken || inFlight !== null || mode !== "terrain") return;
    void runParamPreview(values, token);
  }, 100);
}

async function runParamPreview(values: Float64Array, token: number): Promise<void> {
  if (inFlight !== null) return;
  try {
    const response = await send<Extract<WorkerResponse, { kind: "preview" }>>({
      id: requestId++,
      kind: "preview",
      params: Float64Array.from(values),
      grid: 96,
    });
    if (token !== paramPreviewToken || inFlight !== null || mode !== "terrain") return;
    view?.show(response.terrain);
    setStatus("preview · low resolution — Generate to apply");
  } catch {
    // Previews are best effort: a rejected parameter combination simply does
    // not update the viewport, and Generate reports the real error.
  }
}

// ---------------------------------------------------------------------------
// AI terrain suggestion
//
// Candidate worlds are rendered by the worker `preview` request at 96², which
// never touches the live simulation. The best candidate is shown in the
// viewport with an explicit "preview" banner; parameters only change when the
// user presses Apply, and the world is only rebuilt when Generate is pressed.
// ---------------------------------------------------------------------------

/** Puts the last generated world back into the viewport. */
function restoreWorldView(): void {
  if (view === null) return;
  if (currentTerrain !== null) {
    view.show(currentTerrain);
    view.setMode(mode);
    const cached = currentTerrain.fields?.[mode];
    if (cached !== undefined) view.setField(mode, cached);
    stats?.setTerrain(currentTerrain);
    refreshHudLand();
  } else {
    view.setMode(mode);
  }
}

/** Hides the suggestion banner (and optionally restores the generated world). */
function clearSuggestion(restoreView: boolean): void {
  const wasActive = suggestionActive;
  suggestionActive = false;
  suggestionValues = null;
  suggestionMetrics = null;
  suggestionBar.hidden = true;
  if (wasActive && restoreView) restoreWorldView();
}

function dismissSuggestion(): void {
  if (!suggestionActive) return;
  clearSuggestion(true);
  setStatus("world unchanged · suggestion dismissed");
  toasts?.info("Suggestion dismissed — the generated world is unchanged.");
}

function applySuggestion(): void {
  if (suggestionValues === null || panel === null) return;
  const values = Float64Array.from(suggestionValues);
  clearSuggestion(true);
  panel.applyExternalValues(values);
  setStatus("suggestion applied to parameters — Generate to build it");
  toasts?.success("Suggestion applied to parameters — press Generate to build it.");
}

/** Scores a handful of low-resolution candidates and previews the best one. */
async function suggestTerrain(): Promise<void> {
  if (schema === null || panel === null) return;
  if (engineState === null) {
    toasts?.info("Generate a world before asking for suggestions.");
    return;
  }
  if (inFlight !== null) {
    toasts?.info("Finish the current operation before asking for a suggestion.");
    return;
  }
  stopPlayback();
  cancelParamPreview();
  clearSuggestion(false);
  inFlight = "suggest";
  updateUiLock();
  setStatus(`scoring ${SUGGESTION_COUNT} candidate worlds…`);
  try {
    const base = Float64Array.from(panel.getValues());
    const index = valueIndex(schema);
    const candidates = buildSuggestionCandidates(base, schema, SUGGESTION_COUNT);
    let best: { values: Float64Array; metrics: SuggestionMetrics; terrain: TerrainData } | null = null;
    let failures = 0;
    for (let candidateIndex = 0; candidateIndex < candidates.length; candidateIndex += 1) {
      const values = candidates[candidateIndex];
      if (values === undefined) continue;
      setStatus(`scoring candidate ${candidateIndex + 1}/${candidates.length}…`);
      setHudProgress((candidateIndex + 0.5) / candidates.length, true);
      try {
        const response = await send<Extract<WorkerResponse, { kind: "preview" }>>({
          id: requestId++,
          kind: "preview",
          params: Float64Array.from(values),
          grid: 96,
        });
        const metrics = scoreSuggestion(response.terrain, values, index);
        if (best === null || metrics.score > best.metrics.score + 1e-9) {
          best = { values, metrics, terrain: response.terrain };
        }
      } catch {
        failures += 1;
      }
    }
    if (best === null) {
      throw new Error("no candidate preview could be generated");
    }
    suggestionActive = true;
    suggestionValues = best.values;
    suggestionMetrics = best.metrics;
    view?.show(best.terrain);
    const seedAt = index.get("seed");
    const seed = seedAt !== undefined ? Math.round(best.values[seedAt] ?? 0) : null;
    suggestionBar.hidden = false;
    setText(suggestionTitle, "Suggestion (preview)");
    setText(
      suggestionDetail,
      `${describeSuggestion(best.metrics.landFraction, best.metrics.reliefRatio, best.metrics.coastFraction)}${
        seed === null ? "" : ` · seed ${seed.toLocaleString("en-US")}`
      }`,
    );
    setStatus("suggestion preview · Apply to edit parameters — Generate to build");
    toasts?.info(`Suggestion: ${best.metrics.label} (preview)`);
    if (failures > 0) {
      toasts?.info(`${failures} candidate${failures === 1 ? "" : "s"} could not be previewed and were skipped.`);
    }
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    clearSuggestion(true);
    setStatus(`suggestion failed: ${message}`, true);
    toasts?.error(`Terrain suggestion failed: ${message}`);
  } finally {
    inFlight = null;
    updateUiLock();
    setHudProgress(0, false);
  }
}

// ---------------------------------------------------------------------------
// Minerals survey
// ---------------------------------------------------------------------------

const MINERALS_MIN_INTERVAL = 1200;

function isMineralMode(next: ColorMode): boolean {
  return next === "minerals" || next === "mineral_richness" || next === "mineral_deposits";
}

function applyMineralsReport(report: MineralsReport): void {
  mineralsReport = report;
  if (report.minerals.length > 0) {
    minerals = report.minerals;
    view?.setCatalogs(biomes, rocks, minerals);
    legend?.setCatalogs(biomes, rocks, minerals);
  }
  stats?.setMinerals(report);
  if (isMineralMode(mode)) {
    view?.setMode(mode);
    void refreshField();
  }
}

/** Debounced mineral survey request; also used after operations settle. */
function scheduleMineralsRefresh(delay = 180): void {
  if (mineralsTimer !== null) window.clearTimeout(mineralsTimer);
  mineralsTimer = window.setTimeout(() => {
    mineralsTimer = null;
    void refreshMinerals();
  }, delay);
}

async function refreshMinerals(): Promise<void> {
  if (engineState === null) return;
  if (
    inFlight === "generate" ||
    inFlight === "seek" ||
    inFlight === "run" ||
    inFlight === "suggest" ||
    inFlight === "export"
  ) {
    scheduleMineralsRefresh(400);
    return;
  }
  const now = performance.now();
  if (now - lastMineralsAt < MINERALS_MIN_INTERVAL) {
    scheduleMineralsRefresh(Math.max(120, MINERALS_MIN_INTERVAL - (now - lastMineralsAt)));
    return;
  }
  const token = ++mineralsToken;
  lastMineralsAt = now;
  try {
    const response = await send<Extract<WorkerResponse, { kind: "minerals" }>>({
      id: requestId++,
      kind: "minerals",
    });
    if (token !== mineralsToken) return;
    applyMineralsReport(response.report);
  } catch {
    // Keep the previous survey; the card shows its waiting state on failure.
  }
}

// ---------------------------------------------------------------------------
// Custom geology
// ---------------------------------------------------------------------------

function geologyDocumentSummary(kind: "rocks" | "strata", json: string | null): string {
  if (json === null) {
    return kind === "rocks" ? "No custom rocks — engine defaults" : "No custom strata — engine defaults";
  }
  const parsed = kind === "rocks" ? parseRocksJson(json) : parseStrataJson(json);
  if (!parsed.ok) return `Invalid ${kind === "rocks" ? "rocks" : "strata"} JSON (${parsed.error})`;
  return `${parsed.document.summary} loaded`;
}

function updateGeologyUi(): void {
  geologyLabel = describeGeology(rocksJson, strataJson);
  setText(geologyStatus, geologyLabel ?? "");
  geologyStatus.hidden = geologyLabel === null;
  geologyStatus.classList.toggle("is-active", geologyLabel !== null);
  setText(rocksFileStatus, geologyDocumentSummary("rocks", rocksJson));
  setText(strataFileStatus, geologyDocumentSummary("strata", strataJson));
  rocksFileStatus.classList.toggle("is-loaded", rocksJson !== null);
  strataFileStatus.classList.toggle("is-loaded", strataJson !== null);
  if (geologyDialogStatus.textContent === "") {
    setText(
      geologyDialogStatus,
      geologyLabel ?? "No custom geology — the engine defaults will be used.",
    );
  } else if (geologyLabel !== null) {
    setText(geologyDialogStatus, `${geologyLabel} — applies on the next Generate.`);
  }
  geologyButton.classList.toggle("is-active", geologyLabel !== null);
  geologyButton.title = geologyLabel ?? "Load custom rocks / strata JSON";
}

function applyGeologyState(
  rocks: string | null,
  strata: string | null,
  options: { persist?: boolean; announce?: boolean } = {},
): void {
  rocksJson = rocks;
  strataJson = strata;
  if (options.persist !== false) writeStoredGeology(rocksJson, strataJson);
  updateGeologyUi();
  if (options.announce === true) {
    if (geologyLabel === null) {
      setStatus("custom geology cleared — regenerate to apply");
      toasts?.info("Custom geology cleared — engine defaults will apply on the next Generate.");
    } else {
      setStatus(`${geologyLabel} — regenerate to apply`);
      toasts?.success(`${geologyLabel} — press Generate to rebuild the world.`);
    }
  }
}

function readGeologyFile(kind: "rocks" | "strata", file: File): void {
  void file
    .text()
    .then((text) => {
      const parsed = kind === "rocks" ? parseRocksJson(text) : parseStrataJson(text);
      if (!parsed.ok) {
        const message = `${kind === "rocks" ? "Rocks" : "Strata"} file rejected: ${parsed.error}`;
        setText(geologyDialogStatus, message);
        geologyDialogStatus.classList.add("is-error");
        toasts?.error(message);
        return;
      }
      geologyDialogStatus.classList.remove("is-error");
      if (kind === "rocks") rocksJson = parsed.document.json;
      else strataJson = parsed.document.json;
      applyGeologyState(rocksJson, strataJson, { announce: false });
      setText(
        geologyDialogStatus,
        `${geologyLabel ?? "Custom geology"} — press Generate to rebuild the world.`,
      );
      toasts?.success(
        `${kind === "rocks" ? "Custom rocks" : "Strata stack"} loaded (${parsed.document.summary}).`,
      );
    })
    .catch((error: unknown) => {
      const message = `Cannot read file: ${error instanceof Error ? error.message : String(error)}`;
      setText(geologyDialogStatus, message);
      geologyDialogStatus.classList.add("is-error");
      toasts?.error(message);
    });
}

function clearGeologyDocuments(): void {
  applyGeologyState(null, null, { announce: false });
  geologyDialogStatus.classList.remove("is-error");
  setText(geologyDialogStatus, "Custom geology cleared — engine defaults will apply on the next Generate.");
  toasts?.info("Custom geology cleared.");
}

function openGeologyDialog(): void {
  if (geologyDialogOpen) return;
  geologyDialogOpen = true;
  lastFocusedBeforeGeology = document.activeElement;
  geologyDialog.hidden = false;
  updateGeologyUi();
  geologyClose.focus();
}

function closeGeologyDialog(): void {
  if (!geologyDialogOpen) return;
  geologyDialogOpen = false;
  geologyDialog.hidden = true;
  if (lastFocusedBeforeGeology instanceof HTMLElement) lastFocusedBeforeGeology.focus();
}

function wireGeologyControls(): void {
  geologyButton.addEventListener("click", openGeologyDialog);
  geologyClose.addEventListener("click", closeGeologyDialog);
  geologyDone.addEventListener("click", closeGeologyDialog);
  geologyDialog.addEventListener("click", (event) => {
    if (event.target === geologyDialog) closeGeologyDialog();
  });
  geologyClearButton.addEventListener("click", clearGeologyDocuments);
  rocksFileInput.addEventListener("change", () => {
    const file = rocksFileInput.files?.[0];
    rocksFileInput.value = "";
    if (file !== undefined) readGeologyFile("rocks", file);
  });
  strataFileInput.addEventListener("change", () => {
    const file = strataFileInput.files?.[0];
    strataFileInput.value = "";
    if (file !== undefined) readGeologyFile("strata", file);
  });
}

// ---------------------------------------------------------------------------
// Worker requests
// ---------------------------------------------------------------------------

async function generate(values: Float64Array): Promise<GenerateOutcome> {
  if (schema === null || panel === null) return "error";
  if (inFlight !== null) {
    pendingGenerate = Float64Array.from(values);
    return "queued";
  }
  clearSuggestion(false);
  cancelParamPreview();
  const params = Float64Array.from(values);
  stopPlayback();
  inFlight = "generate";
  updateUiLock();
  setStatus("generating world…");
  try {
    const response = await send<Extract<WorkerResponse, { kind: "generated" }>>({
      id: requestId++,
      kind: "generate",
      params,
      ...(rocksJson !== null ? { rocksJson } : {}),
      ...(strataJson !== null ? { strataJson } : {}),
    });
    generatedValues = params;
    currentTerrain = response.terrain;
    applyState(response.state);
    view?.show(response.terrain);
    view?.setMode(mode);
    stats?.setTerrain(response.terrain);
    refreshHudLand();
    panel.setApplied(params);
    generatedPreset = findPreset(params);
    const generatedIsDefault = generatedPreset === null && isDefaultValues(params);
    generatedPresetKey = generatedPreset?.key ?? (generatedIsDefault ? "defaults" : null);
    generatedPresetLabel = generatedPreset?.label ?? (generatedIsDefault ? "schema defaults" : "custom");
    syncPresetUi(panel.getValues());
    persistParams(params);
    profileLine = {
      a: [response.state.grid.width * 0.15, response.state.grid.height * 0.5],
      b: [response.state.grid.width * 0.85, response.state.grid.height * 0.5],
    };
    view?.setProfileLine(profileLine);
    legend?.setVisible(true);
    setStatus("world ready");
    app.classList.remove("is-booting");
    bootOverlay?.hide();
    await Promise.all([refreshStats(), refreshField(), requestProfile(), refreshMinerals()]);
    return "ok";
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    setStatus(`generation failed: ${message}`, true);
    toasts?.error(`Generation failed: ${message}`);
    if (generatedValues === null) {
      bootOverlay?.showError(message, () => window.location.reload());
    }
    return "error";
  } finally {
    inFlight = null;
    updateUiLock();
    flushQueued();
    // The survey is comparatively expensive: fetch it once the generator is idle.
    if (engineState !== null) scheduleMineralsRefresh(150);
  }
}

async function step(count: number): Promise<void> {
  if (inFlight !== null || engineState === null) return;
  if (suggestionActive) clearSuggestion(true);
  inFlight = "step";
  updateUiLock();
  try {
    const response = await send<Extract<WorkerResponse, { kind: "stepped" }>>({
      id: requestId++,
      kind: "step",
      steps: count,
    });
    currentTerrain = response.terrain;
    applyState(response.state);
    view?.show(response.terrain);
    stats?.setTerrain(response.terrain);
    refreshHudLand();
    stepCounter += 1;
    if (mode !== "terrain" && stepCounter % 3 === 0) void refreshField();
    if (stepCounter % 8 === 0) {
      void refreshStats();
    }
    if (stepCounter % 12 === 0) scheduleProfile(80);
    if (stepCounter % 16 === 0) scheduleMineralsRefresh(600);
    if (response.state.finished) {
      stopPlayback();
      setStatus(`reached the configured duration · ${formatDuration(response.state.elapsed_yr)}`);
      void refreshStats();
      scheduleMineralsRefresh(0);
    }
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    setStatus(message, true);
    toasts?.error(message);
    stopPlayback();
  } finally {
    inFlight = null;
    updateUiLock();
    flushQueued();
  }
}

async function runToEnd(): Promise<void> {
  if (inFlight !== null || engineState === null) return;
  if (suggestionActive) clearSuggestion(true);
  stopPlayback();
  inFlight = "run";
  updateUiLock();
  timeline?.setActivity("running · 0%");
  setStatus(`running to ${formatDuration(engineState.total_yr)}…`);
  try {
    const response = await send<Extract<WorkerResponse, { kind: "stepped" }>>({
      id: requestId++,
      kind: "run",
    });
    currentTerrain = response.terrain;
    applyState(response.state);
    view?.show(response.terrain);
    stats?.setTerrain(response.terrain);
    refreshHudLand();
    setHudProgress(1, true);
    setStatus(
      `run complete · ${formatDuration(response.state.elapsed_yr)} · ${response.state.iteration.toLocaleString("en-US")} iterations`,
    );
    toasts?.success(`Simulation reached ${formatDuration(response.state.elapsed_yr)}`);
    await Promise.all([refreshStats(), refreshField(), requestProfile(), refreshMinerals()]);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    setStatus(message, true);
    toasts?.error(message);
  } finally {
    inFlight = null;
    updateUiLock();
    timeline?.setActivity(null);
    setHudProgress(0, false);
    flushQueued();
    scheduleMineralsRefresh(200);
  }
}

async function seek(year: number): Promise<void> {
  if (engineState === null) return;
  if (suggestionActive) clearSuggestion(true);
  stopPlayback();
  if (inFlight !== null) {
    if (inFlight === "run" || inFlight === "step" || inFlight === "seek") {
      pendingSeek = year;
    }
    return;
  }
  const total = engineState.total_yr;
  const target = clamp(year, 0, total);
  if (Math.abs(target - engineState.elapsed_yr) < 1e-6 && currentTerrain !== null) {
    timeline?.clearPreview();
    setStatus(`t = ${formatDuration(target)}`);
    return;
  }
  inFlight = "seek";
  updateUiLock();
  timeline?.setActivity("seeking · 0%");
  setStatus(`seeking to ${formatDuration(target)}…`);
  try {
    await previewFrameAt(target, true);
    const response = await send<Extract<WorkerResponse, { kind: "stepped" }>>({
      id: requestId++,
      kind: "seek",
      year: target,
    });
    currentTerrain = response.terrain;
    applyState(response.state);
    view?.show(response.terrain);
    stats?.setTerrain(response.terrain);
    refreshHudLand();
    setStatus(`timeline seek complete · ${response.state.history.frames.length} frames`);
    await Promise.all([refreshField(), requestProfile(), refreshStats(), refreshMinerals()]);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    setStatus(message, true);
    toasts?.error(message);
  } finally {
    inFlight = null;
    updateUiLock();
    timeline?.setActivity(null);
    timeline?.clearPreview();
    setHudProgress(0, false);
    flushQueued();
    scheduleMineralsRefresh(200);
  }
}

/**
 * Seeks to the previous captured frame (backward stepping).
 *
 * Adaptive time steps can overshoot a seek target, so we aim one base step
 * before the previous frame. That guarantees the replay lands on a visibly
 * earlier time instead of bouncing back to the current state.
 */
function stepBack(): void {
  if (engineState === null || engineState.history.frames.length === 0) {
    void seek(0);
    return;
  }
  const elapsed = engineState.elapsed_yr;
  const frames = engineState.history.frames;
  let target = 0;
  for (let i = frames.length - 1; i >= 0; i -= 1) {
    const frame = frames[i];
    if (frame !== undefined && frame.time < elapsed - 1e-6) {
      target = Math.max(0, frame.time - engineState.step_yr);
      break;
    }
  }
  void seek(target);
}

/** Shows the nearest captured frame immediately (cheap; used for scrubbing). */
async function previewFrameAt(year: number, force: boolean): Promise<void> {
  const state = engineState;
  if (state === null || state.history.frames.length === 0) return;
  const frames = state.history.frames;
  let nearest = 0;
  for (let i = 0; i < frames.length; i += 1) {
    if ((frames[i]?.time ?? 0) <= year) nearest = i;
  }
  const frame = frames[nearest];
  if (frame === undefined) return;
  if (!force && currentTerrain !== null && Math.abs(currentTerrain.time - frame.time) < 1e-9) return;
  const token = ++previewToken;
  try {
    const response = await send<Extract<WorkerResponse, { kind: "historyFrame" }>>({
      id: requestId++,
      kind: "historyFrame",
      index: nearest,
    });
    if (token !== previewToken || currentTerrain === null) return;
    const preview: TerrainData = {
      width: currentTerrain.width,
      height: currentTerrain.height,
      elevation: response.elevation,
      seaLevel: state.sea_level_m,
      time: response.time,
    };
    currentTerrain = preview;
    view?.show(preview);
    stats?.setTerrain(preview);
    refreshHudLand();
  } catch {
    // A preview is best-effort; the exact seek follows anyway.
  }
}

let statsInFlight = false;

async function refreshStats(): Promise<void> {
  if (statsInFlight) return;
  statsInFlight = true;
  const token = ++statsToken;
  try {
    const response = await send<Extract<WorkerResponse, { kind: "stats" }>>({
      id: requestId++,
      kind: "stats",
    });
    if (token !== statsToken) return;
    stats?.update(response.stats);
  } catch {
    // Statistics are diagnostic: never let them break the simulation UI.
  } finally {
    statsInFlight = false;
  }
}

async function refreshField(): Promise<void> {
  if (currentTerrain === null || view === null) return;
  const requested = mode;
  if (requested === "terrain") {
    view.setMode("terrain");
    return;
  }
  const token = ++fieldToken;
  try {
    const response = await send<Extract<WorkerResponse, { kind: "field" }>>({
      id: requestId++,
      kind: "field",
      mode: requested,
    });
    if (token !== fieldToken || requested !== mode) return;
    currentTerrain.fields = { ...(currentTerrain.fields ?? {}), [requested]: response.values };
    view.setField(requested, response.values);
    view.setMode(requested);
  } catch {
    // Missing overlays simply fall back to terrain colouring.
  }
}

function scheduleProfile(delay = 120): void {
  window.clearTimeout(profileTimer);
  profileTimer = window.setTimeout(() => {
    void requestProfile();
  }, delay);
}

async function requestProfile(): Promise<void> {
  if (profileLine === null || engineState === null) return;
  const token = ++profileToken;
  try {
    const response = await send<Extract<WorkerResponse, { kind: "profile" }>>({
      id: requestId++,
      kind: "profile",
      line: profileLine,
      samples: 256,
    });
    if (token !== profileToken) return;
    currentProfile = response.samples;
    profile?.setSamples(response.samples, engineState.sea_level_m);
  } catch {
    // Keep the previous section on failure.
  }
}

// ---------------------------------------------------------------------------
// Playback
// ---------------------------------------------------------------------------

function togglePlayback(next: boolean): void {
  if (next === playing) return;
  if (next && engineState === null) return;
  if (next && engineState?.finished === true) {
    toasts?.info("The timeline is at the end — jump to the start (Home) to replay.");
    return;
  }
  if (next && suggestionActive) clearSuggestion(true);
  playing = next;
  timeline?.setPlaying(playing);
  if (playing) {
    setStatus("playing — advancing geological time");
    schedulePlayTick();
  } else {
    setStatus("paused");
    window.clearTimeout(playTimer);
    scheduleMineralsRefresh(0);
  }
}

function schedulePlayTick(): void {
  window.clearTimeout(playTimer);
  playTimer = window.setTimeout(() => {
    void (async () => {
      if (!playing) return;
      if (inFlight !== null) {
        schedulePlayTick();
        return;
      }
      if (engineState?.finished === true) {
        stopPlayback();
        toasts?.info("Reached the end of the simulation");
        return;
      }
      await step(stepsPerTick);
      if (playing) schedulePlayTick();
    })();
  }, PLAY_INTERVAL_MS);
}

function stopPlayback(): void {
  if (!playing) return;
  playing = false;
  window.clearTimeout(playTimer);
  timeline?.setPlaying(false);
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

function currentExportMeta(): ExportMeta {
  const seedAt = schema === null ? undefined : valueIndex(schema).get("seed");
  const seed =
    generatedValues !== null && seedAt !== undefined ? Math.round(generatedValues[seedAt] ?? 0) : null;
  return {
    appVersion: schema?.version ?? "unknown",
    preset: generatedPresetKey,
    seed,
    mode,
    elapsedYear: engineState?.elapsed_yr ?? 0,
    totalYear: engineState?.total_yr ?? 0,
    stepYear: engineState?.step_yr ?? 0,
    grid:
      engineState === null
        ? null
        : {
            width: engineState.grid.width,
            height: engineState.grid.height,
            cellKm: engineState.grid.cell_km,
          },
  };
}

type ExportFormat =
  | "obj"
  | "xyz"
  | "raw"
  | "params"
  | "stats"
  | "height16"
  | "normals"
  | "gltf"
  | "profile"
  | "exr"
  | "netcdf";

const BINARY_EXPORT_SPECS: Record<
  "raw" | "height16" | "normals" | "gltf" | "profile" | "exr" | "netcdf",
  { what: string; extension: string; mime: string }
> = {
  raw: { what: "heightmap", extension: "f32", mime: "application/octet-stream" },
  height16: { what: "heightmap-16bit", extension: "png", mime: "image/png" },
  normals: { what: "normals", extension: "png", mime: "image/png" },
  gltf: { what: "mesh", extension: "gltf", mime: "model/gltf+json" },
  profile: { what: "profile", extension: "png", mime: "image/png" },
  exr: { what: "heightmap", extension: "exr", mime: "application/octet-stream" },
  netcdf: { what: "fields", extension: "nc", mime: "application/x-netcdf" },
};

async function exportFile(format: ExportFormat): Promise<void> {
  if (inFlight !== null) {
    toasts?.info("Finish the current operation before exporting.");
    return;
  }
  inFlight = "export";
  updateUiLock();
  setStatus(`exporting ${format.toUpperCase()}…`);
  const meta = currentExportMeta();
  try {
    const response = await send<Extract<WorkerResponse, { kind: "export" }>>({
      id: requestId++,
      kind: "export",
      format,
    });
    if (response.bytes !== undefined) {
      const spec =
        BINARY_EXPORT_SPECS[format as keyof typeof BINARY_EXPORT_SPECS] ?? {
          what: format,
          extension: "bin",
          mime: "application/octet-stream",
        };
      downloadBytes(exportFilename(meta, spec.what, spec.extension), response.bytes, spec.mime);
    } else if (response.text !== undefined) {
      const keys = schema?.params.map((param) => param.key) ?? [];
      if (format === "params") {
        downloadText(exportFilename(meta, "params", "json"), enrichParamsJson(response.text, keys, meta), "application/json");
      } else if (format === "stats") {
        downloadText(exportFilename(meta, "stats", "json"), enrichStatsJson(response.text, meta), "application/json");
      } else {
        downloadText(exportFilename(meta, format, format), response.text, "text/plain");
      }
    }
    toasts?.success(`Exported ${formatLabel(format)}`);
    setStatus(`exported ${formatLabel(format)}`);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    setStatus(`export failed: ${message}`, true);
    toasts?.error(`Export failed: ${message}`);
  } finally {
    inFlight = null;
    updateUiLock();
    flushQueued();
  }
}

function formatLabel(format: string): string {
  switch (format) {
    case "obj":
      return "terrain mesh (OBJ)";
    case "xyz":
      return "point cloud (XYZ)";
    case "raw":
      return "heightmap (F32)";
    case "params":
      return "parameters JSON";
    case "stats":
      return "statistics JSON";
    case "height16":
      return "16-bit heightmap (PNG)";
    case "normals":
      return "normal map (PNG)";
    case "gltf":
      return "3D mesh (glTF)";
    case "profile":
      return "cross-section (PNG)";
    case "exr":
      return "OpenEXR heightmap";
    case "netcdf":
      return "NetCDF fields";
    default:
      return format.toUpperCase();
  }
}

function screenshot(): void {
  const dataUrl = view?.screenshot();
  if (dataUrl === undefined || dataUrl === null) {
    toasts?.error("No rendered frame to capture yet.");
    return;
  }
  const meta = currentExportMeta();
  downloadDataUrl(exportFilename(meta, `viewport-${mode}`, "png"), dataUrl);
  toasts?.success("Saved viewport PNG");
}

// ---------------------------------------------------------------------------
// Project save / load
// ---------------------------------------------------------------------------

/** Downloads the generated world's parameters, time and display mode. */
async function saveProject(): Promise<void> {
  if (schema === null) return;
  if (inFlight !== null) {
    toasts?.info("Finish the current operation before saving a project.");
    return;
  }
  const values = generatedValues ?? panel?.getValues();
  if (values === undefined) {
    toasts?.error("No parameters to save yet — generate a world first.");
    return;
  }
  inFlight = "export";
  updateUiLock();
  setStatus("saving project…");
  try {
    // The engine echoes the active user-defined geology; it is stored next to
    // the raw documents so a project can be restored exactly.
    let engineGeology: string | null = null;
    try {
      const geologyResponse = await send<Extract<WorkerResponse, { kind: "geology" }>>({
        id: requestId++,
        kind: "geology",
      });
      engineGeology = geologyResponse.json;
    } catch {
      // Geology is optional metadata; save the project without it.
    }
    const meta = currentExportMeta();
    const pendingChanges = panel?.isDirty() ?? false;
    const json = buildProjectJson(values, schema, {
      presetKey: generatedPresetKey,
      seed: meta.seed,
      mode,
      elapsedYears: engineState?.elapsed_yr ?? 0,
      engineVersion: schema.version,
      grid: meta.grid,
      totalYears: engineState?.total_yr ?? null,
      stepYears: engineState?.step_yr ?? null,
      geology: geologyProjectPayload(rocksJson, strataJson, engineGeology),
    });
    downloadText(exportFilename(meta, "project", "json"), json, "application/json");
    toasts?.success(
      pendingChanges
        ? "Project saved (generated parameter set — pending edits excluded)"
        : `Project saved (parameters + time + display mode${geologyLabel !== null ? " + custom geology" : ""})`,
    );
    setStatus("project saved");
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    setStatus(`project save failed: ${message}`, true);
    toasts?.error(`Project save failed: ${message}`);
  } finally {
    inFlight = null;
    updateUiLock();
    flushQueued();
  }
}

/** Applies a parsed project document: parameters, time and overlay mode. */
async function loadProjectText(text: string): Promise<void> {
  if (schema === null || panel === null) {
    toasts?.error("Engine is not ready yet.");
    return;
  }
  if (loadingProject) {
    toasts?.info("A project is already being loaded.");
    return;
  }
  if (inFlight !== null) {
    const becameIdle = await waitForIdle(15000);
    if (!becameIdle) {
      toasts?.info("Engine is still busy — try loading the project again in a moment.");
      setStatus("project load postponed — engine busy", true);
      return;
    }
  }
  const parsed = parseProjectJson(text, schema);
  if (!parsed.ok) {
    toasts?.error(`Cannot load project: ${parsed.error}`);
    setStatus(`project load failed: ${parsed.error}`, true);
    return;
  }
  const project = parsed.project;
  loadingProject = true;
  stopPlayback();
  clearSuggestion(false);
  cancelParamPreview();
  try {
    if (project.mode !== null) applyMode(project.mode, false, false);
    let loadRocks: string | null = null;
    let loadStrata: string | null = null;
    if (project.geology !== null) {
      if (project.geology.rocksJson !== null) {
        const rocksParsed = parseRocksJson(project.geology.rocksJson);
        if (rocksParsed.ok) loadRocks = rocksParsed.document.json;
        else project.warnings.push(`custom rocks ignored: ${rocksParsed.error}`);
      }
      if (project.geology.strataJson !== null) {
        const strataParsed = parseStrataJson(project.geology.strataJson);
        if (strataParsed.ok) loadStrata = strataParsed.document.json;
        else project.warnings.push(`strata stack ignored: ${strataParsed.error}`);
      }
      if (
        loadRocks === null &&
        loadStrata === null &&
        project.geology.engineJson !== null &&
        project.geology.engineJson !== "null"
      ) {
        project.warnings.push(
          "project carried engine geology without the raw documents — default geology used",
        );
      }
    }
    applyGeologyState(loadRocks, loadStrata, { persist: true, announce: false });
    panel.setValues(project.values);
    syncPresetUi(project.values);
    setStatus(`loading project · ${project.presetKey ?? "custom"}…`);
    // `generate` can queue behind an operation that started while we were
    // applying the project (background refresh, a playback tick, …). Wait for
    // idle and retry a few times instead of reporting a bogus rejection.
    let outcome: GenerateOutcome = "queued";
    for (let attempt = 0; attempt < 20 && outcome === "queued"; attempt += 1) {
      await waitForIdle(15000);
      outcome = await generate(project.values);
    }
    if (outcome !== "ok") {
      setStatus("project load failed: the engine could not generate the saved parameters", true);
      if (outcome === "queued") {
        toasts?.error("Project load failed: the engine stayed busy.");
      }
      return;
    }
    const total = engineState?.total_yr ?? 0;
    const target = Math.max(0, Math.min(project.elapsedYears, total));
    if (project.elapsedYears > total + 0.5) {
      project.warnings.push(
        `saved time ${formatDuration(project.elapsedYears)} exceeds this world's duration; seeked to ${formatDuration(total)}`,
      );
    }
    if (target > 0.5) {
      await seek(target);
    }
    const presetLabel =
      project.presetKey === null
        ? "custom"
        : presets.find((entry) => entry.key === project.presetKey)?.label ?? project.presetKey;
    const seedLabel = project.seed === null ? "" : ` · seed ${project.seed.toLocaleString("en-US")}`;
    toasts?.success(
      `Loaded project · ${presetLabel}${seedLabel} · ${formatDuration(
        engineState?.elapsed_yr ?? 0,
      )}${geologyLabel !== null ? " · custom geology" : ""}`,
    );
    for (const warning of project.warnings) {
      toasts?.info(warning);
    }
    setStatus("project loaded");
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    setStatus(`project load failed: ${message}`, true);
    toasts?.error(`Project load failed: ${message}`);
  } finally {
    loadingProject = false;
  }
}

function loadProjectFile(file: File): void {
  if (loadingProject) return;
  setStatus(`reading ${file.name}…`);
  void file
    .text()
    .then((text) => loadProjectText(text))
    .catch((error: unknown) => {
      const message = error instanceof Error ? error.message : String(error);
      setStatus(`cannot read project: ${message}`, true);
      toasts?.error(`Cannot read project file: ${message}`);
    });
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

function persistParams(values: Float64Array): void {
  try {
    localStorage.setItem(STORAGE_PARAMS, JSON.stringify(Array.from(values)));
  } catch {
    // Private mode: persistence is optional.
  }
}

function loadStoredValues(): Float64Array {
  if (schema === null) return new Float64Array(0);
  const defaults = defaultValues(schema);
  try {
    const stored = localStorage.getItem(STORAGE_PARAMS);
    if (stored === null) return defaults;
    const parsed = JSON.parse(stored) as unknown;
    if (!Array.isArray(parsed) || parsed.length !== defaults.length) return defaults;
    const index = valueIndex(schema);
    for (const [key, at] of index) {
      const spec = schema.params[at];
      if (spec === undefined) continue;
      const raw = Number(parsed[at]);
      if (!Number.isFinite(raw)) continue;
      defaultValues(schema); // keep values sanitised below
      const value = clamp(raw, spec.min, spec.max);
      defaults[at] = spec.kind === "int" || spec.kind === "enum" ? Math.round(value) : value;
      void key;
    }
    return defaults;
  } catch {
    return defaults;
  }
}

function loadStoredMode(): ColorMode {
  try {
    const stored = localStorage.getItem(STORAGE_MODE);
    if (stored !== null && COLOR_MODE_LABELS.some((entry) => entry.key === stored)) {
      return stored as ColorMode;
    }
  } catch {
    // Ignore storage failures.
  }
  return "terrain";
}

// ---------------------------------------------------------------------------
// Static UI wiring
// ---------------------------------------------------------------------------

const narrowLayout = window.matchMedia("(max-width: 1100px)");

function updateDrawerState(): void {
  const drawerOpen =
    app.classList.contains("drawer-left-open") || app.classList.contains("drawer-right-open");
  drawerBackdrop.hidden = !drawerOpen;
  if (narrowLayout.matches) {
    paramsButton.setAttribute("aria-expanded", String(app.classList.contains("drawer-left-open")));
    statsButton.setAttribute("aria-expanded", String(app.classList.contains("drawer-right-open")));
  } else {
    paramsButton.setAttribute("aria-expanded", String(!app.classList.contains("left-collapsed")));
    statsButton.setAttribute("aria-expanded", String(!app.classList.contains("right-collapsed")));
  }
}

function closeDrawers(): void {
  app.classList.remove("drawer-left-open", "drawer-right-open");
  updateDrawerState();
}

function toggleSidebar(side: "left" | "right"): void {
  if (narrowLayout.matches) {
    const openClass = side === "left" ? "drawer-left-open" : "drawer-right-open";
    const otherClass = side === "left" ? "drawer-right-open" : "drawer-left-open";
    const willOpen = !app.classList.contains(openClass);
    app.classList.remove(otherClass);
    app.classList.toggle(openClass, willOpen);
    updateDrawerState();
  } else {
    const collapsedClass = side === "left" ? "left-collapsed" : "right-collapsed";
    app.classList.toggle(collapsedClass);
    updateDrawerState();
  }
}

function openHelp(): void {
  if (helpOpen) return;
  helpOpen = true;
  lastFocusedBeforeHelp = document.activeElement;
  helpDialog.hidden = false;
  helpClose.focus();
}

function closeHelp(): void {
  if (!helpOpen) return;
  helpOpen = false;
  helpDialog.hidden = true;
  if (lastFocusedBeforeHelp instanceof HTMLElement) lastFocusedBeforeHelp.focus();
}

function updateExaggerationUi(): void {
  exagValue.textContent = `×${(exaggeration / EXAGGERATION_NEUTRAL).toFixed(1)}`;
  exagDown.disabled = exaggeration <= EXAGGERATION_MIN;
  exagUp.disabled = exaggeration >= EXAGGERATION_MAX;
  exagValue.title = `Vertical exaggeration ${exaggeration} (neutral ${EXAGGERATION_NEUTRAL})`;
}

function setExaggeration(next: number): void {
  exaggeration = clamp(Math.round(next), EXAGGERATION_MIN, EXAGGERATION_MAX);
  view?.setExaggeration(exaggeration);
  updateExaggerationUi();
}

function resetView(): void {
  view?.resetCamera();
  setExaggeration(EXAGGERATION_NEUTRAL);
  setStatus("camera and relief reset");
}

function wireStaticControls(): void {
  paramsButton.addEventListener("click", () => toggleSidebar("left"));
  statsButton.addEventListener("click", () => toggleSidebar("right"));
  drawerBackdrop.addEventListener("click", closeDrawers);
  helpButton.addEventListener("click", () => (helpOpen ? closeHelp() : openHelp()));
  helpClose.addEventListener("click", closeHelp);
  helpDialog.addEventListener("click", (event) => {
    if (event.target === helpDialog) closeHelp();
  });
  resetViewButton.addEventListener("click", resetView);
  exagDown.addEventListener("click", () => setExaggeration(exaggeration - EXAGGERATION_STEP));
  exagUp.addEventListener("click", () => setExaggeration(exaggeration + EXAGGERATION_STEP));
  wireGeologyControls();
  suggestButton.addEventListener("click", () => void suggestTerrain());
  suggestionApplyButton.addEventListener("click", applySuggestion);
  suggestionDismissButton.addEventListener("click", dismissSuggestion);
  projectFileInput.addEventListener("change", () => {
    const file = projectFileInput.files?.[0];
    projectFileInput.value = "";
    if (file !== undefined) loadProjectFile(file);
  });
  narrowLayout.addEventListener("change", () => {
    if (!narrowLayout.matches) {
      app.classList.remove("drawer-left-open", "drawer-right-open");
    } else {
      app.classList.remove("left-collapsed", "right-collapsed");
    }
    updateDrawerState();
  });
  updateDrawerState();
}

function wireGlobalKeys(): void {
  document.addEventListener("keydown", (event) => {
    if (event.defaultPrevented) return;
    const target = event.target;
    if (target instanceof HTMLElement) {
      if (target.closest("input, select, textarea, [contenteditable='true'], button, a, [role='slider']")) {
        return;
      }
    }
    if (event.metaKey || event.ctrlKey || event.altKey) return;
    const total = engineState?.total_yr ?? 0;
    switch (event.key) {
      case " ":
        event.preventDefault();
        togglePlayback(!playing);
        break;
      case "ArrowRight":
        event.preventDefault();
        if (event.shiftKey) void seek((engineState?.elapsed_yr ?? 0) + total * 0.05);
        else void step(1);
        break;
      case "ArrowLeft":
        event.preventDefault();
        if (event.shiftKey) void seek((engineState?.elapsed_yr ?? 0) - total * 0.05);
        else stepBack();
        break;
      case "Home":
        event.preventDefault();
        void seek(0);
        break;
      case "End":
        event.preventDefault();
        void seek(total);
        break;
      case "g":
      case "G":
        if (!event.shiftKey && panel !== null) void generate(panel.getValues());
        break;
      case "r":
      case "R":
        view?.resetCamera();
        break;
      case "/":
        event.preventDefault();
        panel?.focusSearch();
        break;
      case "?":
        event.preventDefault();
        openHelp();
        break;
      case "Escape":
        closeHelp();
        closeGeologyDialog();
        closeDrawers();
        break;
      default:
        break;
    }
  });
}

// ---------------------------------------------------------------------------
// Boot
// ---------------------------------------------------------------------------

async function boot(): Promise<void> {
  initTooltips();
  hydrateIcons();
  wireStaticControls();
  wireGlobalKeys();
  toasts = new Toasts(toastHost);
  bootOverlay = new BootOverlay(bootLayer, bootRetry);
  bootOverlay.show("Booting GeoGenesis", "loading the WebAssembly simulation engine…");
  // No engine work can succeed before the worker reports ready.
  generateButton.disabled = true;
  randomizeButton.disabled = true;
  seedButton.disabled = true;
  exportButton.disabled = true;
  presetSelect.disabled = true;
  modeSelect.disabled = true;

  timeline = new Timeline(timelineHost, {
    onPlayToggle: (next) => togglePlayback(next),
    onStep: () => void step(1),
    onStepBack: () => stepBack(),
    onRun: () => void runToEnd(),
    onRewind: () => void seek(0),
    onSeek: (year) => void seek(year),
    onScrubPreview: (year) => {
      if (inFlight === null) void previewFrameAt(year, false);
    },
    onSpeedChange: (steps) => {
      stepsPerTick = steps;
      if (playing) setStatus(`playing at ${steps} steps per tick`);
    },
  });
  timeline.setBusy(true);

  setStatus("loading engine…");
  const ready = await send<Extract<WorkerResponse, { kind: "ready" }>>({
    id: requestId++,
    kind: "init",
  });
  schema = ready.schema;
  presets = ready.presets;
  biomes = ready.biomes;
  rocks = ready.rocks;

  // Mode selector: every display mode, in schema order (new modes appear
  // automatically because the list is shared).
  for (const entry of COLOR_MODE_LABELS) {
    modeSelect.appendChild(el("option", { value: entry.key, text: entry.label, title: entry.labelJa }));
  }
  mode = loadStoredMode();
  modeSelect.value = mode;

  // Preset selector + a live "Custom" indicator.
  for (const preset of presets) {
    presetSelect.appendChild(el("option", { value: preset.key, text: preset.label, title: preset.description }));
  }
  presetSelect.appendChild(el("option", { value: "__default", text: "Schema defaults" }));
  presetSelect.appendChild(el("option", { value: "__custom", text: "Custom" }));

  bootOverlay.show("Preparing the viewport", "building the terrain renderer…");

  view = new TerrainView(viewport, { minimap });
  fpsMeter = new FpsMeter(hudFps);
  view.setCatalogs(biomes, rocks);
  view.setMode(mode);
  exaggeration = clamp(Math.round(view.getExaggeration()), EXAGGERATION_MIN, EXAGGERATION_MAX);
  updateExaggerationUi();
  view.onProfileChange((line) => {
    profileLine = line;
    scheduleProfile(60);
  });

  stats = new StatsPanel(statsHost);
  stats.setCatalogs(biomes);
  profile = new ProfilePanel(profileCanvas, { head: sectionHead });
  profile.setCatalogs(biomes, rocks);
  legend = new Legend(legendHost);
  legend.setCatalogs(biomes, rocks);
  legend.setMode(mode);
  legend.setVisible(false);

  // Restore user-defined geology (if any) so the first generation uses it.
  const storedGeology = readStoredGeology();
  applyGeologyState(storedGeology.rocksJson, storedGeology.strataJson, {
    persist: false,
    announce: false,
  });

  const initial = loadStoredValues();
  panel = new ParameterPanel(panelHost, schema, presets, initial, {
    onValuesChanged: (values) => {
      if (suggestionActive) clearSuggestion(false);
      syncPresetUi(values);
      scheduleParamPreview(values);
    },
    onGenerate: (values) => {
      void generate(values);
    },
    onDirtyChange: (dirty, pendingCount) => {
      generateButton.classList.toggle("dirty", dirty);
      generateButton.setAttribute(
        "aria-label",
        dirty ? `Generate world (${pendingCount} pending changes)` : "Generate world",
      );
      generateButton.title = dirty
        ? `${pendingCount} pending parameter change${pendingCount === 1 ? "" : "s"}`
        : "Generate the world from the current parameters";
    },
  });
  panel.setValues(initial);
  panel.setApplied(initial);
  syncPresetUi(initial);

  modeSelect.addEventListener("change", () => {
    applyMode(modeSelect.value as ColorMode);
  });
  presetSelect.addEventListener("change", () => {
    if (panel === null) return;
    if (presetSelect.value === "__custom") {
      syncPresetUi(panel.getValues());
      return;
    }
    if (presetSelect.value === "__default") {
      const values = panel.resetDefaults();
      presetSelect.blur();
      void generate(values);
      return;
    }
    const preset = presets.find((entry) => entry.key === presetSelect.value);
    if (preset === undefined) return;
    const values = panel.applyPreset(preset);
    presetSelect.blur();
    void generate(values);
  });
  seedButton.addEventListener("click", () => {
    if (panel === null) return;
    panel.rerollSeed();
    void generate(panel.getValues());
  });
  randomizeButton.addEventListener("click", () => {
    if (panel === null) return;
    const values = panel.randomize();
    void generate(values);
  });
  generateButton.addEventListener("click", () => {
    if (panel === null) return;
    void generate(panel.getValues());
  });
  setupExportMenu(exportButton, exportMenu, {
    onSaveProject: () => void saveProject(),
    onLoadProject: () => {
      projectFileInput.value = "";
      projectFileInput.click();
    },
    onScreenshot: screenshot,
    onObj: () => void exportFile("obj"),
    onXyz: () => void exportFile("xyz"),
    onRaw: () => void exportFile("raw"),
    onParams: () => void exportFile("params"),
    onStats: () => void exportFile("stats"),
    onHeight16: () => void exportFile("height16"),
    onNormals: () => void exportFile("normals"),
    onGltf: () => void exportFile("gltf"),
    onProfile: () => void exportFile("profile"),
    onExr: () => void exportFile("exr"),
    onNetcdf: () => void exportFile("netcdf"),
  });

  window.addEventListener("resize", () => {
    if (currentProfile.length > 0) profile?.setSamples(currentProfile, engineState?.sea_level_m);
  });
  window.addEventListener("pagehide", () => {
    fpsMeter?.dispose();
  });

  bootOverlay.show("Generating the first world", "plate tectonics · relief · climate · initial drainage…", null);
  await generate(initial);
  bootOverlay.setProgress(1);
  timeline?.setBusy(false);
}

void boot().catch((error: unknown) => {
  const message = error instanceof Error ? error.message : String(error);
  setStatus(`failed to start: ${message}`, true);
  toasts?.error(`Engine failed to start: ${message}`);
  bootOverlay?.showError(message, () => window.location.reload());
  console.error(error);
});
