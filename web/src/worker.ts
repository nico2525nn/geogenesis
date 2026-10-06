/**
 * Engine worker: loads the real WASM module and runs generation off-thread.
 *
 * No fallback generator lives here. If the WASM binary is missing or fails to
 * instantiate, every request fails loudly with the loader error — a silent JS
 * reimplementation would render different terrain than the engine and the UI
 * would present fake output as real.
 */

import type { ParamValues, WorkerRequest, WorkerResponse } from "./types";

interface WasmExports extends WebAssembly.Exports {
  memory: WebAssembly.Memory;
  gg_create(ptr: number, len: number): number;
  gg_generate_initial(): number;
  gg_step(steps: number): number;
  gg_run(): number;
  gg_width(): number;
  gg_height(): number;
  gg_elapsed_yr(): number;
  gg_sea_level_m(): number;
  gg_elevation_ptr(out_len: number): number;
  gg_biomes_ptr(out_len: number): number;
  gg_stats_ptr(out_len: number): number;
  gg_free(ptr: number, len: number): void;
  gg_last_error(out_len: number): number;
}

let wasm: WasmExports | null = null;
let loadError: string | null = null;

async function load(): Promise<void> {
  if (wasm !== null || loadError !== null) return;
  try {
    // The build copies the wasm binary next to the bundle as engine.wasm.
    // `new URL(..., import.meta.url)` keeps working under Vite dev and build.
    const url = new URL("./engine.wasm", import.meta.url);
    const response = await fetch(url);
    if (!response.ok) {
      throw new Error(`engine.wasm: HTTP ${response.status}`);
    }
    const module = await WebAssembly.compileStreaming(response);
    const instance = await WebAssembly.instantiate(module, {});
    wasm = instance.exports as unknown as WasmExports;
  } catch (err) {
    loadError = err instanceof Error ? err.message : String(err);
  }
}

function requireWasm(): WasmExports {
  if (wasm === null) {
    throw new Error(loadError ?? "engine not loaded");
  }
  return wasm;
}

function readJsonParams(params: ParamValues): { ptr: number; len: number } {
  const w = requireWasm();
  const text = JSON.stringify(params);
  const bytes = new TextEncoder().encode(text);
  const ptr = malloc(bytes.length);
  new Uint8Array(w.memory.buffer).set(bytes, ptr);
  return { ptr, len: bytes.length };
}

/** Bump allocator: the module has no malloc, so carve input space from a cursor. */
let heapCursor = 1024 * 1024;

function malloc(size: number): number {
  const w = requireWasm();
  const pagesNeeded = Math.ceil((heapCursor + size - w.memory.buffer.byteLength) / 65536);
  if (pagesNeeded > 0) w.memory.grow(pagesNeeded);
  const ptr = heapCursor;
  heapCursor += size + 15 - ((size + 15) % 16);
  return ptr;
}

function readBuffer(ptr: number, byteLen: number): Uint8Array {
  const w = requireWasm();
  return new Uint8Array(w.memory.buffer.slice(ptr, ptr + byteLen));
}

function lastError(): string {
  try {
    const w = requireWasm();
    const lenPtr = malloc(8);
    new BigUint64Array(w.memory.buffer)[lenPtr / 8] = 0n;
    // gg_last_error writes through a usize out-pointer (4 bytes on wasm32).
    const view = new DataView(w.memory.buffer);
    const ptr = w.gg_last_error(lenPtr);
    const len = view.getUint32(lenPtr, true);
    const msg = new TextDecoder().decode(readBuffer(ptr, len));
    w.gg_free(ptr, len);
    return msg || "unknown engine error";
  } catch {
    return "unknown engine error";
  }
}

function checkStatus(code: number): void {
  if (code !== 0) throw new Error(lastError());
}

function post(message: WorkerResponse): void {
  self.postMessage(message);
}

self.onmessage = async (event: MessageEvent<WorkerRequest>) => {
  const request = event.data;
  try {
    await load();
    const w = requireWasm();
    switch (request.kind) {
      case "generate": {
        const { ptr, len } = readJsonParams(request.params);
        const status = w.gg_create(ptr, len);
        checkStatus(status);
        w.gg_generate_initial();
        post({
          id: request.id,
          kind: "generated",
          result: {
            ok: true,
            width: w.gg_width(),
            height: w.gg_height(),
            seaLevel: w.gg_sea_level_m(),
          },
        });
        break;
      }
      case "step": {
        const done = w.gg_step(request.steps);
        if (done < 0) throw new Error(lastError());
        post({ id: request.id, kind: "stepped", steps: done, elapsedYr: w.gg_elapsed_yr() });
        break;
      }
      case "run": {
        const done = w.gg_run();
        if (done < 0) throw new Error(lastError());
        post({ id: request.id, kind: "stepped", steps: done, elapsedYr: w.gg_elapsed_yr() });
        break;
      }
      case "elevation": {
        const lenPtr = malloc(8);
        const view = new DataView(w.memory.buffer);
        const ptr = w.gg_elevation_ptr(lenPtr);
        const byteLen = view.getUint32(lenPtr, true);
        const width = w.gg_width();
        const height = w.gg_height();
        const seaLevel = w.gg_sea_level_m();
        const data = new Float64Array(readBuffer(ptr, byteLen).buffer);
        w.gg_free(ptr, byteLen);
        post({ id: request.id, kind: "elevation", width, height, seaLevel, data });
        break;
      }
      case "stats": {
        const lenPtr = malloc(8);
        const view = new DataView(w.memory.buffer);
        const ptr = w.gg_stats_ptr(lenPtr);
        const byteLen = view.getUint32(lenPtr, true);
        const stats = JSON.parse(new TextDecoder().decode(readBuffer(ptr, byteLen)));
        w.gg_free(ptr, byteLen);
        post({ id: request.id, kind: "stats", stats });
        break;
      }
    }
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    if (request.kind === "generate") {
      post({ id: request.id, kind: "generated", result: { ok: false, error: message } });
    } else {
      post({ id: request.id, kind: "error", message });
    }
  }
};

// Announce readiness once the module evaluates.
void load().then(() => {
  if (loadError !== null) post({ id: 0, kind: "error", message: loadError });
  else post({ id: 0, kind: "ready" });
});
