import { PARAM_GROUPS, defaultParams, formatValue, renderStats } from "./params";
import type { ColorMode, EngineStats, ParamValues, WorkerRequest, WorkerResponse } from "./types";
import { TerrainView, type TerrainData } from "./view";

const status = document.getElementById("status") as HTMLElement;
const canvas = document.getElementById("scene") as HTMLCanvasElement;
const profileCanvas = document.getElementById("profile") as HTMLCanvasElement;
const statsBox = document.getElementById("stats") as HTMLElement;
const btnGenerate = document.getElementById("btn-generate") as HTMLButtonElement;
const btnStep = document.getElementById("btn-step") as HTMLButtonElement;
const btnRun = document.getElementById("btn-run") as HTMLButtonElement;
const selMode = document.getElementById("sel-mode") as HTMLSelectElement;

const view = new TerrainView(canvas);
const params: ParamValues = defaultParams();
let worker: Worker | null = null;
let requestId = 1;
let busy = false;
let current: TerrainData | null = null;
const pending = new Map<number, (response: WorkerResponse) => void>();

function setStatus(text: string): void {
  status.textContent = text;
}

function setBusy(value: boolean): void {
  busy = value;
  for (const b of [btnGenerate, btnStep, btnRun]) b.disabled = value;
}

function ensureWorker(): Worker {
  if (worker !== null) return worker;
  worker = new Worker(new URL("./worker.ts", import.meta.url), { type: "module" });
  worker.onmessage = (event: MessageEvent<WorkerResponse>) => {
    const response = event.data;
    if (response.kind === "ready") {
      setStatus("engine ready");
      return;
    }
    const resolve = pending.get(response.id);
    if (resolve !== undefined) {
      pending.delete(response.id);
      resolve(response);
    }
  };
  worker.onerror = (event) => {
    setStatus(`worker error: ${event.message}`);
    setBusy(false);
  };
  return worker;
}

function callGenerate(params: ParamValues): Promise<WorkerResponse> {
  const id = requestId;
  requestId += 1;
  const { promise, resolve } = Promise.withResolvers<WorkerResponse>();
  pending.set(id, resolve);
  ensureWorker().postMessage({ id, kind: "generate", params });
  return promise;
}

function callStep(steps: number): Promise<WorkerResponse> {
  const id = requestId;
  requestId += 1;
  const { promise, resolve } = Promise.withResolvers<WorkerResponse>();
  pending.set(id, resolve);
  ensureWorker().postMessage({ id, kind: "step", steps });
  return promise;
}

function callRun(): Promise<WorkerResponse> {
  const id = requestId;
  requestId += 1;
  const { promise, resolve } = Promise.withResolvers<WorkerResponse>();
  pending.set(id, resolve);
  ensureWorker().postMessage({ id, kind: "run" });
  return promise;
}

function callSimple(kind: "elevation" | "stats"): Promise<WorkerResponse> {
  const id = requestId;
  requestId += 1;
  const { promise, resolve } = Promise.withResolvers<WorkerResponse>();
  pending.set(id, resolve);
  ensureWorker().postMessage({ id, kind });
  return promise;
}

async function refreshTerrain(): Promise<void> {
  const elevation = (await callSimple("elevation")) as Extract<WorkerResponse, { kind: "elevation" } | { kind: "error" }>;
  if (elevation.kind === "error") {
    setStatus(`elevation failed: ${elevation.message}`);
    return;
  }
  let biomes: Uint8Array | null = null;
  if (viewMode() === "biome") {
    // Biome overlay comes from the same worker on demand.
    biomes = null;
  }
  current = { width: elevation.width, height: elevation.height, elevation: elevation.data, seaLevel: elevation.seaLevel, biomes };
  view.show(current);
  drawProfile();
  const stats = (await callSimple("stats")) as Extract<WorkerResponse, { kind: "stats" } | { kind: "error" }>;
  if (stats.kind === "stats") renderStats(statsBox, stats.stats as EngineStats);
}

function viewMode(): ColorMode {
  return (selMode.value ?? "relief") as ColorMode;
}

function drawProfile(): void {
  if (current === null) return;
  const ctx = profileCanvas.getContext("2d");
  if (ctx === null) return;
  const parentWidth = profileCanvas.parentElement?.clientWidth ?? 600;
  profileCanvas.width = parentWidth;
  const { width, height, elevation, seaLevel } = current;
  const midRow = Math.floor(height / 2);
  let min = Infinity;
  let max = -Infinity;
  for (const v of elevation) {
    if (Number.isFinite(v)) {
      if (v < min) min = v;
      if (v > max) max = v;
    }
  }
  const span = Math.max(1e-9, max - min);
  const W = profileCanvas.width;
  const H = profileCanvas.height;
  ctx.clearRect(0, 0, W, H);
  ctx.strokeStyle = "#5fb878";
  ctx.lineWidth = 1.5;
  ctx.beginPath();
  for (let i = 0; i < W; i += 1) {
    const x = Math.floor((i / W) * width);
    const z = elevation[midRow * width + x] ?? 0;
    const y = H - 6 - ((z - min) / span) * (H - 12);
    if (i === 0) ctx.moveTo(i, y);
    else ctx.lineTo(i, y);
  }
  ctx.stroke();
  // Sea level line.
  const seaY = H - 6 - ((seaLevel - min) / span) * (H - 12);
  ctx.strokeStyle = "#3b82c4";
  ctx.setLineDash([4, 4]);
  ctx.beginPath();
  ctx.moveTo(0, seaY);
  ctx.lineTo(W, seaY);
  ctx.stroke();
  ctx.setLineDash([]);
}

function buildPanel(): void {
  const groups: Array<[string, string]> = [
    ["params-world", "World"],
    ["params-structure", "Structure"],
    ["params-erosion", "Erosion"],
    ["params-time", "Time"],
  ];
  for (const [id, title] of groups) {
    const host = document.getElementById(id);
    if (host === null) continue;
    const defs = PARAM_GROUPS.find((g) => g.title === title)?.params ?? [];
    for (const def of defs) {
      const row = document.createElement("div");
      row.className = "param";
      const label = document.createElement("label");
      label.textContent = def.label;
      label.htmlFor = `param-${def.key}`;
      const output = document.createElement("output");
      const input = document.createElement("input");
      input.type = "range";
      input.id = `param-${def.key}`;
      input.min = String(def.log === true ? Math.log10(Math.max(1e-12, def.min)) : def.min);
      input.max = String(def.log === true ? Math.log10(Math.max(1e-12, def.max)) : def.max);
      input.step = String(def.log === true ? (Number(input.max) - Number(input.min)) / 200 : def.step);
      const current = params[def.key];
      const numeric = typeof current === "number" ? current : def.def;
      input.value = String(def.log === true ? Math.log10(Math.max(1e-12, numeric)) : numeric);
      output.textContent = formatValue(def, numeric);
      input.addEventListener("input", () => {
        const raw = Number(input.value);
        const value = def.log === true ? 10 ** raw : raw;
        params[def.key] = value;
        output.textContent = formatValue(def, value);
      });
      label.appendChild(document.createElement("br"));
      row.append(label, output, input);
      host.appendChild(row);
    }
  }
}

async function generate(): Promise<void> {
  if (busy) return;
  setBusy(true);
  setStatus("generating…");
  try {
    const response = (await callGenerate(params)) as Extract<
      WorkerResponse,
      { kind: "generated" }
    >;
    if (!response.result.ok) {
      setStatus(`failed: ${response.result.error ?? "unknown"}`);
      return;
    }
    setStatus(`world ${response.result.width}×${response.result.height} — eroding…`);
    await refreshTerrain();
    const elapsed = current !== null ? "" : "";
    setStatus(`done (${response.result.width}×${response.result.height})${elapsed}`);
  } finally {
    setBusy(false);
  }
}

async function step(): Promise<void> {
  if (busy) return;
  setBusy(true);
  try {
    const response = (await callStep(1)) as Extract<
      WorkerResponse,
      { kind: "stepped" } | { kind: "error" }
    >;
    if (response.kind === "error") {
      setStatus(`step failed: ${response.message}`);
      return;
    }
    setStatus(`t = ${response.elapsedYr.toFixed(0)} yr`);
    await refreshTerrain();
  } finally {
    setBusy(false);
  }
}

async function run(): Promise<void> {
  if (busy) return;
  setBusy(true);
  try {
    setStatus("running to completion…");
    const response = (await callRun()) as Extract<
      WorkerResponse,
      { kind: "stepped" } | { kind: "error" }
    >;
    if (response.kind === "error") {
      setStatus(`run failed: ${response.message}`);
      return;
    }
    setStatus(`t = ${response.elapsedYr.toFixed(0)} yr`);
    await refreshTerrain();
  } finally {
    setBusy(false);
  }
}

btnGenerate.addEventListener("click", () => void generate());
btnStep.addEventListener("click", () => void step());
btnRun.addEventListener("click", () => void run());
selMode.addEventListener("change", () => {
  view.setMode(viewMode());
  if (current !== null) {
    view.show(current);
    view.render();
  }
});

buildPanel();
setStatus("engine loading…");
ensureWorker();
