// Headless smoke test for the GeoGenesis WASM ABI (Node >= 20).
//
// Loads web/public/engine.wasm, drives the C ABI exactly like the Web Worker
// does, and checks determinism, buffers, history, seeking and every preset.
// Run: node scripts/smoke-wasm.mjs

import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const wasmPath = join(root, "web/public/engine.wasm");

const failures = [];
function check(name, condition, detail = "") {
  if (condition) {
    console.log(`  ok   ${name}`);
  } else {
    failures.push(`${name} ${detail}`);
    console.log(`  FAIL ${name} ${detail}`);
  }
}

const { instance } = await WebAssembly.instantiate(await readFile(wasmPath), {});
const ex = instance.exports;
const memory = ex.memory;
const outLenPtr = ex.gg_alloc(4);

function u32(ptr) {
  return new DataView(memory.buffer).getUint32(ptr, true);
}
function take(ptr) {
  const len = u32(outLenPtr);
  const bytes = new Uint8Array(memory.buffer, ptr, len).slice();
  ex.gg_free(ptr, len);
  return bytes;
}
function takeString(call) {
  return new TextDecoder().decode(take(call()));
}
function takeF32(ptr) {
  const bytes = take(ptr);
  return new Float32Array(bytes.buffer, 0, bytes.byteLength / 4).slice();
}
function lastError() {
  return takeString(() => ex.gg_last_error_ptr(outLenPtr));
}
function create(values) {
  const ptr = ex.gg_alloc(values.length * 8);
  new Float64Array(memory.buffer, ptr, values.length).set(values);
  const code = ex.gg_create(ptr, values.length);
  ex.gg_free(ptr, values.length * 8);
  return code;
}
function state() {
  return JSON.parse(takeString(() => ex.gg_state_ptr(outLenPtr)));
}
function elevation() {
  return takeF32(ex.gg_field_ptr(0, outLenPtr));
}

console.log(`GeoGenesis engine ${takeString(() => ex.gg_version_ptr(outLenPtr))}`);
console.log("ABI smoke test");

const schema = JSON.parse(takeString(() => ex.gg_schema_ptr(outLenPtr)));
const presets = JSON.parse(takeString(() => ex.gg_presets_ptr(outLenPtr))).presets;
const biomes = JSON.parse(takeString(() => ex.gg_biomes_ptr(outLenPtr))).biomes;
const rocks = JSON.parse(takeString(() => ex.gg_rocks_ptr(outLenPtr))).rocks;
check("schema has parameters", schema.params.length > 80, `(${schema.params.length})`);
check("biomes catalogue", biomes.length >= 12, `(${biomes.length})`);
check("rocks catalogue", rocks.length === 14, `(${rocks.length})`);
check("presets authored", presets.length >= 6, `(${presets.length})`);

const keyIndex = new Map(schema.params.map((p, i) => [p.key, i]));
const defaults = Float64Array.from(schema.params.map((p) => p.default));
function applyOverrides(values, overrides) {
  for (const [key, value] of Object.entries(overrides)) {
    const index = keyIndex.get(key);
    if (index === undefined) {
      throw new Error(`unknown preset key ${key}`);
    }
    values[index] = value;
  }
}
function small(values, size = 96) {
  values[keyIndex.get("width")] = size;
  values[keyIndex.get("height")] = size;
  return values;
}

// --- basic lifecycle -------------------------------------------------------
let params = small(Float64Array.from(defaults));
params[keyIndex.get("total_yr")] = 30_000;
params[keyIndex.get("step_yr")] = 10_000;
params[keyIndex.get("history_frames")] = 8;
params[keyIndex.get("seed")] = 42;
const t0 = performance.now();
let code = create(params);
const createMs = performance.now() - t0;
check("create default world", code === 0, lastError());
let s = state();
check("grid matches parameters", s.grid.width === 96 && s.grid.height === 96);
check("initial frame captured", s.history.count >= 1, `(${s.history.count})`);
const elev0 = elevation();
check("elevation buffer size", elev0.length === 96 * 96, `(${elev0.length})`);
check("elevation finite", elev0.every(Number.isFinite));
const land = elev0.filter((z) => z > s.sea_level_m).length / elev0.length;
check("world has land and ocean", land > 0.05 && land < 0.95, `(land ${land.toFixed(2)})`);

// --- binary export ABI -----------------------------------------------------
const exportKinds = {
  height16: 0,
  normals: 1,
  gltf: 2,
  profile: 3,
  exr: 4,
  netcdf: 5,
};
for (const [label, kind] of Object.entries(exportKinds)) {
  const bytes = take(ex.gg_export_ptr(kind, outLenPtr));
  check(`export ${label} produces bytes`, bytes.length > 0, `(${bytes.length})`);
  if (kind === 0) {
    check("export height16 is PNG", bytes[0] === 0x89 && bytes[1] === 0x50);
    // IHDR bit depth at offset 24 must be 16 for a 16-bit grayscale image.
    check("export height16 is 16-bit", bytes[24] === 16 && bytes[25] === 0);
  } else if (kind === 1) {
    check("export normals is PNG", bytes[0] === 0x89 && bytes[1] === 0x50);
  } else if (kind === 2) {
    const text = new TextDecoder().decode(bytes.slice(0, 64));
    check("export gltf is JSON", text.startsWith('{"asset"'));
  } else if (kind === 3) {
    check("export profile is PNG", bytes[0] === 0x89 && bytes[1] === 0x50);
  } else if (kind === 4) {
    check(
      "export exr has OpenEXR magic",
      bytes.length > 16 &&
        bytes[0] === 0x76 &&
        bytes[1] === 0x2f &&
        bytes[2] === 0x31 &&
        bytes[3] === 0x01,
    );
  } else {
    const magic = new TextDecoder().decode(bytes.slice(0, 4));
    check("export netcdf is CDF-1", magic === "CDF\x01");
  }
}

// --- low-resolution parameter preview -------------------------------------
const previewPtr = ex.gg_alloc(params.length * 8);
new Float64Array(memory.buffer, previewPtr, params.length).set(params);
const previewCode = ex.gg_preview(previewPtr, params.length, 96);
ex.gg_free(previewPtr, params.length * 8);
check("preview generation", previewCode === 0, lastError());
const previewInfo = JSON.parse(takeString(() => ex.gg_preview_info_ptr(outLenPtr)));
check(
  "preview info matches grid",
  previewInfo.width === 96 && previewInfo.height === 96,
  `(${previewInfo.width}x${previewInfo.height})`,
);
const previewElev = takeF32(ex.gg_preview_ptr(outLenPtr));
check(
  "preview elevation buffer size",
  previewElev.length === previewInfo.width * previewInfo.height,
  `(${previewElev.length})`,
);
check("preview elevation finite", previewElev.every(Number.isFinite));

// --- minerals report -------------------------------------------------------
const mineralsJson = takeString(() => ex.gg_minerals_json_ptr(outLenPtr));
const minerals = JSON.parse(mineralsJson);
check("minerals catalogue", minerals.minerals.length === 14, `(${minerals.minerals.length})`);
check(
  "minerals inventory has entries",
  minerals.inventory.entries.length === minerals.minerals.length,
);
check(
  "minerals map dimensions",
  minerals.map.width === 96 && minerals.map.height === 96,
  `(${minerals.map.width}x${minerals.map.height})`,
);

const steps = ex.gg_step(3);
check("steps completed", steps === 3, `(${steps})`);
s = state();
check("elapsed advanced", Math.abs(s.elapsed_yr - 30_000) < 1e-9, `(${s.elapsed_yr})`);
check("ledger present", Number.isFinite(s.ledger.erosion_m3));
const stats = JSON.parse(takeString(() => ex.gg_stats_ptr(outLenPtr)));
check("stats grid", stats.grid.width === 96);
check("stats elevation range", stats.elevation.max >= stats.elevation.min);
check("stats biomes", Array.isArray(stats.biomes) && stats.biomes.length > 0);
const slope = takeF32(ex.gg_field_ptr(8, outLenPtr));
check("slope field", slope.length === 96 * 96 && slope.every(Number.isFinite));

// --- determinism -----------------------------------------------------------
create(params);
ex.gg_step(3);
const elevA = elevation();
create(params);
ex.gg_step(3);
const elevB = elevation();
check("deterministic replay", elevA.every((v, i) => v === elevB[i]));

// --- history, profile, exports ---------------------------------------------
const frame0 = takeF32(ex.gg_history_frame_ptr(0, outLenPtr));
check("history frame buffer", frame0.length === 96 * 96);
const profileJson = JSON.parse(
  takeString(() => ex.gg_profile_ptr(0, 0, 95, 95, 64, outLenPtr)),
);
check("profile samples", profileJson.samples.length === 64);
const obj = takeString(() => ex.gg_obj_ptr(outLenPtr));
check("obj export", obj.startsWith("# GeoGenesis") && obj.includes("\nf "));
const xyz = takeString(() => ex.gg_xyz_ptr(4, outLenPtr));
check("xyz export", xyz.split("\n").length > 100);
const raw = take(ex.gg_field_ptr(0, outLenPtr));
check("raw elevation bytes", raw.length === 96 * 96 * 4);

// --- seeking ---------------------------------------------------------------
const seekTarget = 20_000;
const seekSteps = ex.gg_seek(seekTarget);
s = state();
check("seek reached target", s.elapsed_yr >= seekTarget - 1e-6, `(${s.elapsed_yr})`);
check("seek reported steps", seekSteps >= 0);
ex.gg_rewind();
s = state();
check("rewind preserved history", s.history.count >= 1, `(${s.history.count})`);

// --- presets ---------------------------------------------------------------
let presetsOk = 0;
for (const preset of presets) {
  const values = small(Float64Array.from(defaults));
  values[keyIndex.get("total_yr")] = 20_000;
  values[keyIndex.get("step_yr")] = 10_000;
  values[keyIndex.get("history_frames")] = 4;
  applyOverrides(values, preset.overrides);
  const presetCode = create(values);
  if (presetCode !== 0) {
    console.log(`  FAIL preset ${preset.key}: ${lastError()}`);
    failures.push(`preset ${preset.key}`);
    continue;
  }
  ex.gg_step(1);
  const e = elevation();
  const ok = e.every(Number.isFinite);
  if (!ok) {
    failures.push(`preset ${preset.key} produced non-finite terrain`);
  } else {
    presetsOk += 1;
  }
}
check("all presets build", presetsOk === presets.length, `(${presetsOk}/${presets.length})`);

// --- custom geology JSON (RockSet + StrataSpec ABI) -----------------------
const rocksJson = await readFile(join(root, "examples/custom-rocks.json"), "utf8");
const strataJson = await readFile(join(root, "examples/strata.json"), "utf8");
const geologyValues = small(Float64Array.from(defaults));
const paramPtr = ex.gg_alloc(geologyValues.length * 8);
new Float64Array(memory.buffer, paramPtr, geologyValues.length).set(geologyValues);
const rocksBytes = new TextEncoder().encode(rocksJson);
const strataBytes = new TextEncoder().encode(strataJson);
const rocksPtr = ex.gg_alloc(rocksBytes.length);
const strataPtr = ex.gg_alloc(strataBytes.length);
new Uint8Array(memory.buffer, rocksPtr, rocksBytes.length).set(rocksBytes);
new Uint8Array(memory.buffer, strataPtr, strataBytes.length).set(strataBytes);
const geologyCode = ex.gg_create_with_geology(
  paramPtr,
  geologyValues.length,
  rocksPtr,
  rocksBytes.length,
  strataPtr,
  strataBytes.length,
);
check("custom geology create", geologyCode === 0, lastError());
if (geologyCode === 0) {
  const config = JSON.parse(takeString(() => ex.gg_geology_json_ptr(outLenPtr)));
  check(
    "geology JSON round-trip",
    config.rocks?.custom_rocks?.length === 3 && config.strata?.layers?.length === 6,
  );
  ex.gg_step(2);
  check("custom geology steps finite", elevation().every(Number.isFinite));
}
ex.gg_free(rocksPtr, rocksBytes.length);
ex.gg_free(strataPtr, strataBytes.length);
// Invalid geology JSON is rejected with an error message (valid parameters).
const badJson = new TextEncoder().encode('{"version":1,"custom_rocks":[{"name":"x"}]}');
const badPtr = ex.gg_alloc(badJson.length);
new Uint8Array(memory.buffer, badPtr, badJson.length).set(badJson);
const badCode = ex.gg_create_with_geology(
  paramPtr,
  geologyValues.length,
  badPtr,
  badJson.length,
  0,
  0,
);
ex.gg_free(badPtr, badJson.length);
check("invalid geology JSON rejected", badCode !== 0 && lastError().length > 0);
ex.gg_free(paramPtr, geologyValues.length * 8);

console.log(`\ncreate 96² in ${createMs.toFixed(1)} ms`);
if (failures.length > 0) {
  console.error(`\n${failures.length} failure(s):`);
  for (const failure of failures) {
    console.error(`  - ${failure}`);
  }
  process.exit(1);
}
console.log("\nALL ABI CHECKS PASSED");
