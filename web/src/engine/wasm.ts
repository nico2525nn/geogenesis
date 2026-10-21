// Typed wrapper around the GeoGenesis WebAssembly C ABI.
//
// The module exports plain `extern "C"` functions and a linear memory. This
// class owns the buffer-copy protocol: every `gg_*_ptr` call returns an
// engine-owned buffer plus a byte length; we copy it out and free it
// immediately, so a later memory growth can never leave a stale view alive.

import type {
  BiomeInfo,
  ColorMode,
  EngineState,
  EngineStats,
  PresetSpec,
  ProfileLine,
  MineralsReport,
  ProfileSample,
  RockInfo,
  Schema,
  TerrainData,
} from "../types";
import { FIELD_IDS } from "../types";

/** Where the compiled engine lives (copied there by scripts/build-wasm.sh). */
export const WASM_URL = `${import.meta.env.BASE_URL}engine.wasm`;

type Ptr = number;

interface EngineExports extends WebAssembly.Exports {
  memory: WebAssembly.Memory;
  gg_version_ptr: (outLen: Ptr) => Ptr;
  gg_schema_ptr: (outLen: Ptr) => Ptr;
  gg_presets_ptr: (outLen: Ptr) => Ptr;
  gg_biomes_ptr: (outLen: Ptr) => Ptr;
  gg_rocks_ptr: (outLen: Ptr) => Ptr;
  gg_last_error_ptr: (outLen: Ptr) => Ptr;
  gg_alloc: (size: number) => Ptr;
  gg_free: (ptr: Ptr, len: number) => void;
  gg_create: (ptr: Ptr, count: number) => number;
  gg_step: (steps: number) => number;
  gg_run: () => number;
  gg_reset: () => number;
  gg_rewind: () => number;
  gg_seek: (year: number) => number;
  gg_state_ptr: (outLen: Ptr) => Ptr;
  gg_stats_ptr: (outLen: Ptr) => Ptr;
  gg_field_ptr: (kind: number, outLen: Ptr) => Ptr;
  gg_history_frame_ptr: (index: number, outLen: Ptr) => Ptr;
  gg_profile_ptr: (
    x0: number,
    y0: number,
    x1: number,
    y1: number,
    samples: number,
    outLen: Ptr,
  ) => Ptr;
  gg_obj_ptr: (outLen: Ptr) => Ptr;
  gg_xyz_ptr: (step: number, outLen: Ptr) => Ptr;
  gg_preview: (ptr: Ptr, count: number, targetWidth: number) => number;
  gg_preview_info_ptr: (outLen: Ptr) => Ptr;
  gg_preview_ptr: (outLen: Ptr) => Ptr;
  gg_export_ptr: (kind: number, outLen: Ptr) => Ptr;
  gg_minerals_json_ptr: (outLen: Ptr) => Ptr;
  gg_create_with_geology: (
    ptr: Ptr,
    count: number,
    rocksPtr: Ptr,
    rocksLen: number,
    strataPtr: Ptr,
    strataLen: number,
  ) => number;
  gg_geology_json_ptr: (outLen: Ptr) => Ptr;
}

/** Error thrown when the engine rejects a request. */
export class EngineError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "EngineError";
  }
}

export class WasmEngine {
  private readonly exports: EngineExports;
  private readonly outLenPtr: Ptr;
  private readonly scratchPtr: Ptr;
  private readonly scratchSize = 8;
  private schemaCache: Schema | null = null;

  private constructor(exports: EngineExports) {
    this.exports = exports;
    // A permanent 8-byte result-length slot and a small parameter staging
    // buffer. Both survive memory growth because they are raw offsets.
    this.outLenPtr = exports.gg_alloc(8);
    this.scratchPtr = exports.gg_alloc(this.scratchSize);
  }

  /** Instantiates the module from a URL. */
  static async load(url: string = WASM_URL): Promise<WasmEngine> {
    const response = await fetch(url);
    if (!response.ok) {
      throw new EngineError(`cannot load engine.wasm (${response.status})`);
    }
    const bytes = await response.arrayBuffer();
    const result = await WebAssembly.instantiate(bytes, {});
    const exports = result.instance.exports as EngineExports;
    if (!exports.memory || !exports.gg_create) {
      throw new EngineError("engine.wasm does not export the expected ABI");
    }
    return new WasmEngine(exports);
  }

  /** Engine version string. */
  version(): string {
    return this.readString(() => this.exports.gg_version_ptr(this.outLenPtr));
  }

  /** Parameter schema. */
  schema(): Schema {
    if (this.schemaCache === null) {
      this.schemaCache = JSON.parse(this.readString(() => this.exports.gg_schema_ptr(this.outLenPtr))) as Schema;
    }
    return this.schemaCache;
  }

  /** Preset catalogue. */
  presets(): PresetSpec[] {
    const json = this.readString(() => this.exports.gg_presets_ptr(this.outLenPtr));
    return (JSON.parse(json) as { presets: PresetSpec[] }).presets;
  }

  /** Biome catalogue. */
  biomes(): BiomeInfo[] {
    const json = this.readString(() => this.exports.gg_biomes_ptr(this.outLenPtr));
    return (JSON.parse(json) as { biomes: BiomeInfo[] }).biomes;
  }

  /** Rock catalogue. */
  rocks(): RockInfo[] {
    const json = this.readString(() => this.exports.gg_rocks_ptr(this.outLenPtr));
    return (JSON.parse(json) as { rocks: RockInfo[] }).rocks;
  }

  /** Creates (or replaces) the simulation from flat parameter values. */
  create(values: Float64Array): void {
    const count = values.length;
    const ptr = this.exports.gg_alloc(count * 8);
    if (ptr === 0) {
      throw new EngineError("engine allocation failed");
    }
    try {
      new Float64Array(this.exports.memory.buffer, ptr, count).set(values);
      const code = this.exports.gg_create(ptr, count);
      if (code !== 0) {
        throw new EngineError(this.lastError() || `gg_create failed with code ${code}`);
      }
    } finally {
      this.exports.gg_free(ptr, count * 8);
    }
  }

  /**
   * Creates (or replaces) the simulation with optional user-defined geology
   * JSON documents (`RockSet` / `StrataSpec`, see `examples/`).
   */
  createWithGeology(values: Float64Array, rocksJson?: string, strataJson?: string): void {
    const count = values.length;
    const ptr = this.exports.gg_alloc(count * 8);
    if (ptr === 0) {
      throw new EngineError("engine allocation failed");
    }
    const allocateText = (text?: string): { ptr: Ptr; len: number } => {
      if (text === undefined || text.trim().length === 0) return { ptr: 0, len: 0 };
      const bytes = new TextEncoder().encode(text);
      const textPtr = this.exports.gg_alloc(bytes.length);
      if (textPtr === 0) throw new EngineError("engine allocation failed");
      new Uint8Array(this.exports.memory.buffer, textPtr, bytes.length).set(bytes);
      return { ptr: textPtr, len: bytes.length };
    };
    let rocks = { ptr: 0, len: 0 };
    let strata = { ptr: 0, len: 0 };
    try {
      new Float64Array(this.exports.memory.buffer, ptr, count).set(values);
      rocks = allocateText(rocksJson);
      strata = allocateText(strataJson);
      const code = this.exports.gg_create_with_geology(
        ptr,
        count,
        rocks.ptr,
        rocks.len,
        strata.ptr,
        strata.len,
      );
      if (code !== 0) {
        throw new EngineError(this.lastError() || `gg_create_with_geology failed with code ${code}`);
      }
    } finally {
      if (rocks.len > 0) this.exports.gg_free(rocks.ptr, rocks.len);
      if (strata.len > 0) this.exports.gg_free(strata.ptr, strata.len);
      this.exports.gg_free(ptr, count * 8);
    }
  }

  /** JSON of the user-defined geology config, or "null" when defaults are used. */
  geologyJson(): string {
    return this.readString(() => this.exports.gg_geology_json_ptr(this.outLenPtr));
  }

  /** Advances by `steps` base steps and returns how many ran. */
  step(steps: number): number {
    return this.exports.gg_step(Math.max(0, Math.floor(steps)));
  }

  /** Runs to the configured end. */
  run(): number {
    return this.exports.gg_run();
  }

  /** Rebuilds the world from the current parameters, preserving timeline frames. */
  rewind(): void {
    const code = this.exports.gg_rewind();
    if (code !== 0) {
      throw new EngineError(this.lastError() || `gg_rewind failed with code ${code}`);
    }
  }

  /** Exact seek by replay. Prefer the worker's chunked seek for UI use. */
  seek(year: number): void {
    const code = this.exports.gg_seek(year);
    if (code < 0) {
      throw new EngineError(this.lastError() || `gg_seek failed with code ${code}`);
    }
  }

  /**
   * Generates a low-resolution preview of `values` without touching the live
   * simulation. The returned terrain is a renderable frame at reduced size.
   */
  preview(values: Float64Array, grid: number): TerrainData {
    const count = values.length;
    const ptr = this.exports.gg_alloc(count * 8);
    if (ptr === 0) {
      throw new EngineError("engine allocation failed");
    }
    let info: { width: number; height: number; seaLevel: number; time: number };
    try {
      new Float64Array(this.exports.memory.buffer, ptr, count).set(values);
      const code = this.exports.gg_preview(ptr, count, Math.max(32, Math.round(grid)));
      if (code !== 0) {
        throw new EngineError(this.lastError() || `gg_preview failed with code ${code}`);
      }
      const json = this.readString(() => this.exports.gg_preview_info_ptr(this.outLenPtr));
      info = JSON.parse(json) as { width: number; height: number; seaLevel: number; time: number };
    } finally {
      this.exports.gg_free(ptr, count * 8);
    }
    const bytes = this.readBuffer(() => this.exports.gg_preview_ptr(this.outLenPtr));
    return {
      width: info.width,
      height: info.height,
      elevation: toFloat32(bytes),
      seaLevel: info.seaLevel,
      time: info.time,
    };
  }

  /** Simulation state summary. */
  state(): EngineState {
    const json = this.readString(() => this.exports.gg_state_ptr(this.outLenPtr));
    return JSON.parse(json) as EngineState;
  }

  /** Full statistics report. */
  stats(): EngineStats {
    const json = this.readString(() => this.exports.gg_stats_ptr(this.outLenPtr));
    return JSON.parse(json) as EngineStats;
  }

  /** Copies a display field (f32) for a colour mode. */
  field(mode: ColorMode): Float32Array {
    const kind = FIELD_IDS[mode];
    const bytes = this.readBuffer(() => this.exports.gg_field_ptr(kind, this.outLenPtr));
    return toFloat32(bytes);
  }

  /** Copies a timeline frame's elevation (f32). */
  historyFrame(index: number): Float32Array {
    const bytes = this.readBuffer(() => this.exports.gg_history_frame_ptr(index, this.outLenPtr));
    return toFloat32(bytes);
  }

  /** Samples a cross-section. */
  profile(line: ProfileLine, samples: number): ProfileSample[] {
    const json = this.readString(() =>
      this.exports.gg_profile_ptr(
        line.a[0],
        line.a[1],
        line.b[0],
        line.b[1],
        Math.max(2, Math.round(samples)),
        this.outLenPtr,
      ),
    );
    const parsed = JSON.parse(json) as { samples: Array<[number, number, number, number, number]> };
    return parsed.samples.map(([distance_m, elevation_m, water_depth_m, rock, biome]) => ({
      distance_m,
      elevation_m,
      water_depth_m,
      rock,
      biome,
    }));
  }

  /** OBJ mesh text. */
  exportObj(): string {
    return this.readString(() => this.exports.gg_obj_ptr(this.outLenPtr));
  }

  /** XYZ point cloud text. */
  exportXyz(step: number = 2): string {
    return this.readString(() => this.exports.gg_xyz_ptr(step, this.outLenPtr));
  }

  /**
   * Exports a binary asset for the current simulation state.
   * `kind`: 0 = 16-bit heightmap PNG, 1 = normal-map PNG, 2 = glTF,
   * 3 = cross-section PNG, 4 = OpenEXR heightmap, 5 = NetCDF fields.
   */
  exportBytes(kind: number): Uint8Array {
    const bytes = this.readBuffer(() => this.exports.gg_export_ptr(kind, this.outLenPtr));
    if (bytes.length === 0) {
      throw new EngineError(this.lastError() || `export kind ${kind} failed`);
    }
    return bytes;
  }

  /** Mineral catalogue + deposit map summary + inventory for the world. */
  mineralsReport(): MineralsReport {
    const json = this.readString(() => this.exports.gg_minerals_json_ptr(this.outLenPtr));
    if (json.length === 0) {
      throw new EngineError(this.lastError() || "minerals report is unavailable");
    }
    return JSON.parse(json) as MineralsReport;
  }

  /** Last engine error message. */
  lastError(): string {
    return this.readString(() => this.exports.gg_last_error_ptr(this.outLenPtr));
  }

  /** Copies a returned buffer into a JS-owned Uint8Array and frees the original. */
  private readBuffer(call: () => Ptr): Uint8Array {
    const ptr = call();
    if (ptr === 0) {
      return new Uint8Array(0);
    }
    // Re-read the length slot after the call: the call may have grown memory.
    const len = new Uint32Array(this.exports.memory.buffer, this.outLenPtr, 1)[0] ?? 0;
    if (len === 0) {
      return new Uint8Array(0);
    }
    const bytes = new Uint8Array(this.exports.memory.buffer, ptr, len).slice();
    this.exports.gg_free(ptr, len);
    return bytes;
  }

  /** Copies a returned UTF-8 string. */
  private readString(call: () => Ptr): string {
    const bytes = this.readBuffer(call);
    return new TextDecoder().decode(bytes);
  }

  /** Writes a number into the permanent scratch slot (kept for future ABI calls). */
  protected writeScratch(offset: number, value: number): void {
    new DataView(this.exports.memory.buffer).setFloat64(this.scratchPtr + offset, value, true);
  }
}

function toFloat32(bytes: Uint8Array): Float32Array {
  const out = new Float32Array(bytes.byteLength / 4);
  new Uint8Array(out.buffer).set(bytes);
  return out;
}
