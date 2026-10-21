/// <reference lib="webworker" />
// Engine worker: owns the single WASM simulation and serialises all calls.
//
// The main thread never blocks on generation, stepping, seeking or statistics;
// it posts requests and receives terrain frames, state summaries and stats.
// Long operations (run, backward seek) are chunked so progress messages keep
// the timeline responsive.

import type {
  ColorMode,
  EngineState,
  TerrainData,
  WorkerRequest,
  WorkerResponse,
} from "../types";
import { WasmEngine } from "./wasm";

const scope = self as unknown as DedicatedWorkerGlobalScope;

let engine: WasmEngine | null = null;
/** Last flat parameter set, used to rewind and to export project files. */
let lastParams: Float64Array | null = null;
/** Serialises handlers so worker messages cannot interleave engine calls. */
let queue: Promise<void> = Promise.resolve();

function post(message: WorkerResponse, transfer: Transferable[] = []): void {
  scope.postMessage(message, transfer);
}

function errorMessage(error: unknown): string {
  if (error instanceof Error) {
    return error.message;
  }
  return String(error);
}

function requireEngine(): WasmEngine {
  if (engine === null) {
    throw new Error("engine is not initialised");
  }
  return engine;
}

/** Builds a renderable frame from the current engine state. */
function makeTerrain(state: EngineState): TerrainData {
  const elevation = requireEngine().field("terrain");
  return {
    width: state.grid.width,
    height: state.grid.height,
    elevation,
    seaLevel: state.sea_level_m,
    time: state.elapsed_yr,
  };
}

function postTerrain(
  id: number,
  kind: "generated" | "stepped",
  state: EngineState,
): void {
  const terrain = makeTerrain(state);
  if (kind === "generated") {
    post({ id, kind, state, terrain }, [terrain.elevation.buffer]);
  } else {
    post({ id, kind, state, terrain }, [terrain.elevation.buffer]);
  }
}

async function handle(request: WorkerRequest): Promise<void> {
  switch (request.kind) {
    case "init": {
      engine = await WasmEngine.load();
      const schema = engine.schema();
      const presets = engine.presets();
      const biomes = engine.biomes();
      const rocks = engine.rocks();
      post({ id: request.id, kind: "ready", schema, presets, biomes, rocks });
      return;
    }
    case "generate": {
      const wasm = requireEngine();
      if (request.rocksJson !== undefined || request.strataJson !== undefined) {
        wasm.createWithGeology(request.params, request.rocksJson, request.strataJson);
      } else {
        wasm.create(request.params);
      }
      lastParams = request.params.slice();
      const state = wasm.state();
      postTerrain(request.id, "generated", state);
      return;
    }
    case "preview": {
      const wasm = requireEngine();
      const terrain = wasm.preview(request.params, request.grid);
      post({ id: request.id, kind: "preview", terrain }, [terrain.elevation.buffer]);
      return;
    }
    case "geology": {
      const wasm = requireEngine();
      post({ id: request.id, kind: "geology", json: wasm.geologyJson() });
      return;
    }
    case "minerals": {
      const wasm = requireEngine();
      const report = wasm.mineralsReport();
      post({ id: request.id, kind: "minerals", report });
      return;
    }
    case "step": {
      const wasm = requireEngine();
      wasm.step(request.steps);
      postTerrain(request.id, "stepped", wasm.state());
      return;
    }
    case "run": {
      const wasm = requireEngine();
      const startIteration = wasm.state().iteration;
      let chunks = 0;
      for (;;) {
        const before = wasm.state();
        if (before.finished || before.iteration - startIteration > 5000) {
          break;
        }
        wasm.step(4);
        chunks += 1;
        const state = wasm.state();
        if (chunks % 2 === 0 || state.finished) {
          const terrain = makeTerrain(state);
          post({ id: request.id, kind: "progress", phase: "run", state, terrain }, [
            terrain.elevation.buffer,
          ]);
        }
        if (state.finished) {
          break;
        }
      }
      postTerrain(request.id, "stepped", wasm.state());
      return;
    }
    case "seek": {
      const wasm = requireEngine();
      let state = wasm.state();
      const target = Math.max(0, Math.min(request.year, state.total_yr));
      if (target < state.elapsed_yr - 1e-6) {
        wasm.rewind();
        state = wasm.state();
      }
      let chunks = 0;
      for (;;) {
        state = wasm.state();
        const remaining = target - state.elapsed_yr;
        const stepYr = Math.max(1, state.step_yr);
        if (remaining <= 0 || state.finished || remaining <= 6 * stepYr) {
          break;
        }
        wasm.step(4);
        chunks += 1;
        if (chunks % 3 === 0) {
          const snapshot = wasm.state();
          const terrain = makeTerrain(snapshot);
          post({ id: request.id, kind: "progress", phase: "seek", state: snapshot, terrain }, [
            terrain.elevation.buffer,
          ]);
        }
      }
      // Let the engine land on the exact requested year with a final
      // sub-step instead of stopping on an adaptive-step boundary.
      wasm.seek(target);
      postTerrain(request.id, "stepped", wasm.state());
      return;
    }
    case "field": {
      const values = requireEngine().field(request.mode as ColorMode);
      post({ id: request.id, kind: "field", mode: request.mode, values }, [values.buffer]);
      return;
    }
    case "stats": {
      post({ id: request.id, kind: "stats", stats: requireEngine().stats() });
      return;
    }
    case "state": {
      post({ id: request.id, kind: "state", state: requireEngine().state() });
      return;
    }
    case "historyFrame": {
      const wasm = requireEngine();
      const state = wasm.state();
      const time = state.history.frames[request.index]?.time ?? 0;
      const elevation = wasm.historyFrame(request.index);
      post({ id: request.id, kind: "historyFrame", index: request.index, time, elevation }, [
        elevation.buffer,
      ]);
      return;
    }
    case "profile": {
      const samples = requireEngine().profile(request.line, request.samples);
      post({ id: request.id, kind: "profile", samples });
      return;
    }
    case "export": {
      const wasm = requireEngine();
      const format = request.format;
      if (format === "params") {
        const values = lastParams === null ? [] : Array.from(lastParams);
        post({
          id: request.id,
          kind: "export",
          format,
          text: JSON.stringify({ version: 1, values }),
        });
        return;
      }
      if (format === "stats") {
        post({
          id: request.id,
          kind: "export",
          format,
          text: JSON.stringify(wasm.stats(), null, 2),
        });
        return;
      }
      const binaryFormats: Record<string, number> = {
        height16: 0,
        normals: 1,
        gltf: 2,
        profile: 3,
        exr: 4,
        netcdf: 5,
      };
      const kind = binaryFormats[format];
      if (kind !== undefined) {
        const bytes = wasm.exportBytes(kind);
        post({ id: request.id, kind: "export", format, bytes }, [bytes.buffer]);
        return;
      }
      if (format === "raw") {
        const values = wasm.field("terrain");
        const bytes = new Uint8Array(values.buffer.slice(0));
        post({ id: request.id, kind: "export", format, bytes }, [bytes.buffer]);
        return;
      }
      const text = format === "obj" ? wasm.exportObj() : wasm.exportXyz(2);
      post({ id: request.id, kind: "export", format, text });
      return;
    }
    default: {
      // Exhaustiveness guard: unknown messages are a protocol bug.
      const never: never = request;
      throw new Error(`unknown worker request: ${JSON.stringify(never)}`);
    }
  }
}

scope.onmessage = (event: MessageEvent<WorkerRequest>) => {
  const request = event.data;
  queue = queue
    .then(() => handle(request))
    .catch((error: unknown) => {
      post({ id: request.id, kind: "error", message: errorMessage(error) });
    });
};

export {};
