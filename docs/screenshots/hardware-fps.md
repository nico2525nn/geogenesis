# Real-GPU rendering and frame-rate measurement

Measured **real hardware GPU** rendering (not SwiftShader) for the GeoGenesis
Web UI, plus visual QA screenshots. This document records the methodology, the
WebGL renderer string, and the measured frame times, including the honest
caveats about the shared machine used for the run.

Status against SPEC §13 ("WebGL 100k triangles at 60 fps"): **met and
exceeded** — the default 384² view is ~294k triangles (147k vertices) and holds
a 60 fps vsync against the iGPU, both idle and while the simulation streams
terrain frames into the viewport.

## Environment

| item | value |
| --- | --- |
| GPU | AMD Radeon Graphics iGPU (AMD Barceló / Renoir, PCI `1002:15E7`), `amdgpu` + Mesa `radeonsi`/ACO, Vulkan RADV available |
| Chrome | Google Chrome 155.0.8059.39 (`--headless=new`) |
| OS | Linux, 16 logical CPUs, 1600×1000 headless window (devicePixelRatio 1) |
| Page | production `vite build`, served over local HTTP |
| Viewport | 1600×1000 page → stage canvas ≈ 916×715 CSS px (DPR 1; the renderer caps DPR at 2) |

### Hardware GL in headless Chrome

Headless Chrome defaults to **SwiftShader** (software) unless GPU flags are
given. Hardware rendering *is* reachable headless on this machine; no `xvfb`
is needed (Xvfb would fall back to software llvmpipe anyway):

```bash
google-chrome --headless=new --enable-gpu \
  --use-gl=angle --use-angle=gl \
  --no-sandbox --disable-dev-shm-usage \
  --remote-debugging-port=9222 http://127.0.0.1:4001/
```

Renderer strings read from the page via CDP (`gl.getParameter` with
`WEBGL_debug_renderer_info`):

| flags | `UNMASKED_RENDERER_WEBGL` | verdict |
| --- | --- | --- |
| none (default headless) | `ANGLE (Google, Vulkan 1.3.0 (SwiftShader Device (Subzero) (0x0000C0DE)), SwiftShader driver)` | software |
| `--enable-gpu --use-gl=angle --use-angle=gl` | `ANGLE (AMD, AMD Radeon Graphics (radeonsi renoir ACO), OpenGL ES 3.2)` | **hardware (numbers below)** |
| `--enable-gpu --use-angle=vulkan --enable-features=Vulkan` | `ANGLE (AMD, Vulkan 1.4.335 (AMD Radeon Graphics (RADV RENOIR) (0x000015E7)), radv)` | hardware (Vulkan backend) |
| `--enable-gpu --use-gl=egl` | *(no WebGL context)* | unusable headless |

Both the ANGLE-GL (Mesa radeonsi) and ANGLE-Vulkan (RADV) paths work; the
measurements below use the ANGLE-GL path. `WebGL2` reports max texture size
16384 and 8× MSAA on both.

## Methodology

* The current source tree was built with `vite build` and served from a local
  HTTP server. Grid size (384/512/1024) was set through the parameter panel
  (`width`/`height` = N, then **Generate**) and confirmed from the stats header
  (e.g. `1024×1024 · 10 km/cell`).
* A Chrome DevTools Protocol (CDP) session injected a `requestAnimationFrame`
  interval probe into the live page. It records every frame delta for 3 s
  (idle) or 4 s (playback) and reports fps, P50, P95 and max frame time. This
  matches the compositor cadence the user actually gets; the built-in HUD FPS
  meter (`#hud-fps`, from `web/src/ui/fps.ts`) was read as a cross-check.
* **Playback** was started with the timeline Play transport, so the engine
  worker streams a new `TerrainData` every few steps (~110 ms cadence) and
  `TerrainView.show()` runs once per streamed frame — the realistic stress
  case, unlike a static idle scene.
* **GPU headroom** was additionally measured by relaunching Chrome with
  `--disable-gpu-vsync --disable-frame-rate-limit` (uncapped rAF).
* **`show()` cost** was measured in two ways: (a) a dev harness that
  instantiates `TerrainView` directly on synthetic 384/512/1024 grids and times
  8 consecutive `show()` calls with `performance.now()`; (b) a CDP CPU profile
  of the real app during playback, aggregating self time by function name.
* The rAF interval probe injected over CDP is short enough to inline:

  ```js
  await new Promise((resolve) => {
    const intervals = []; let last = 0; const start = performance.now();
    const tick = (now) => {
      if (last) intervals.push(now - last);
      last = now;
      if (now - start < 3000) requestAnimationFrame(tick);
      else {
        const sorted = intervals.sort((a, b) => a - b);
        const pct = (q) => sorted[Math.floor(q * (sorted.length - 1))];
        resolve({ fps: (sorted.length * 1000) / (now - start), p50: pct(0.5), p95: pct(0.95) });
      }
    };
    requestAnimationFrame(tick);
  });
  ```

* Caveat: this machine is shared with other concurrent workstreams (Rust
  test/clippy/rustc and JVM jobs). System load average was logged before and
  after each run and ranged from ~2 to ~16 on 16 CPUs; contention shows up as
  occasional 33–50 ms frames. The numbers below report the best observed run
  plus the loaded-run ranges. In the heaviest burst observed (load average ~15)
  playback fell to ≈52 fps at 384², ≈39 fps at 512² and ≈33 fps idle at 1024² —
  that is host CPU starvation (the GPU process and the main thread compete for
  cores), not the renderer's steady-state cost, so it is quoted separately
  rather than mixed into the table.

## Measured frame times (capped to the 60 Hz vsync)

Render target ≈ 916×715 CSS px, DPR 1, Khronos neutral tone mapping, 60 fps cap.
"best" = the cleanest run observed (the earliest in the session, before the
concurrent build/test jobs peaked — load average was not instrumented on that
run); "loaded" = the same scene during concurrent Rust/JVM jobs (load average
8–16, where logged), reported as a conservative floor.

| grid | scene | fps (best → loaded) | P50 | P95 (best → loaded) | max |
| --- | --- | --- | --- | --- | --- |
| 384² (147k verts / 294k tris) | idle | **59.9** → 58.9 | 16.7 ms | 16.7 → 16.8 ms | 16.8 ms (clean) |
| 384² | playback | **59.8** → 56.4 | 16.7 ms | 16.7 → 33.3 ms | 16.8 ms (clean) |
| 512² (262k verts / 523k tris) | idle | **59.0** → 50.9 | 16.7 ms | 16.7 → 33.4 ms | 50 ms (one spike) |
| 512² | playback | **57.6** → 47.3 | 16.7 ms | 16.8 → 33.4 ms | 33.4 ms |
| 1024² (1.05M verts / 2.09M tris) | idle | ~58 (loaded only) | 16.7 ms | 33.4 ms | 66.7 ms |
| 1024² | playback | 44–49 (loaded only) | 16.7 ms | 33.4 ms | 50–186 ms |

Reading the table:

* **384² is a locked 60 fps** in the clean run, both idle and while the worker
  streams a new terrain frame roughly every 110 ms (the realistic playback
  stress case). At 294k triangles this already exceeds SPEC §13's 100k-triangle
  60 fps requirement by ~3×.
* **512² is at 57.6 fps during playback in the clean run** (P50 16.7 ms, P95
  16.8 ms — i.e. essentially every frame on time, with one 33 ms frame over a
  4 s window). Under heavy host load it degrades to the mid-40s/50s.
* **1024² is not a locked-60 interactive size on this iGPU** with the current
  build: the scene is valid (1.05 M vertices / 2.09 M triangles) and renders at
  ~100 fps idle uncapped, but streaming terrain updates costs enough CPU + fill
  rate to drop to ~57 fps uncapped / ~45–58 capped under load. Use 384² or
  512² for interactive work.

HUD cross-check: the in-app `#hud-fps` meter (1 s sliding window) showed
`60 fps` for 384²/512² idle in the clean run and `55–60 fps` during playback;
the hardware screenshots above were taken with it reading 60 fps (terrain,
biomes, temperature) and 55 fps during the 1.28 Myr playback shot.

## GPU headroom (uncapped, vsync disabled)

Indicative only — with vsync off the headless compositor's scheduling gets
jittery, and concurrent load contaminates long frames.

| grid | idle fps | playback fps |
| --- | --- | --- |
| 384² | ~247 | (noisy, contaminated by a 5 s external stall) |
| 512² | ~198 | ~146 |
| 1024² | ~100 | ~57 |

So the static scene is comfortably above 60 fps at every supported grid size;
the 1024² playback case is the one where CPU-side per-frame work plus the
raster pass approaches the 16.7 ms budget.

## `show()` update cost

`TerrainView.show()` is designed to be cheap: geometry is static, the height
data is one `R32F` texture upload, colours are evaluated on the GPU from a
linear-light LUT, and only the minimap raster + a small profile/skirt update
run on the CPU.

| grid | `show()` median | max |
| --- | --- | --- |
| 384² | **7.2 ms** | 16.2 ms¹ |
| 512² | **8.4 ms** | 16.8 ms¹ |
| 1024² | **10.3 ms** | 12.1 ms |

¹ occasional GC/background spike. Measured with a dev harness on the hardware
GL path (no other page load), so these are CPU-side costs excluding the actual
GPU raster pass.

CDP CPU profile of the real app during 384²/512² playback: the main thread is
idle 79–85 % of sampled time (waiting on the worker/compositor), the minimap
raster (`renderMinimapBase`) is 1.6–3.2 % and `drawMinimap` ~1.4 %, garbage
collector ~0.2 %, and there are no long-task (>50 ms) entries after page load.
The minimap rebuild (~4–10 ms per simulation frame) is the largest remaining
per-update CPU cost; it is intentionally capped at 384² pixels regardless of
grid size, and the base image is cached between frames (dragging the
cross-section only redraws the line overlay).

## Visual QA (hardware screenshots)

Hardware-rendered screenshots (1600×1000 headless window, CDP capture, DPR 1;
the PNGs are 1600×857 because headless Chrome's window metrics leave a little
vertical chrome) at DPR 1, replacing the earlier software-GL images:

| screenshot | scene |
| --- | --- |
| [`terrain.png`](terrain.png) | 384² Earth-like world, terrain mode, 60 fps HUD |
| [`biomes.png`](biomes.png) | biome overlay (categorical palette) |
| [`temperature.png`](temperature.png) | temperature overlay (continuous ramp), 60 fps HUD |
| [`run-1.3myr.png`](run-1.3myr.png) | simulation running (~1.3 Myr), terrain streamed into the viewport |

The screenshots were taken from a fresh production build served locally
(`vite build`, hardware GL, CDP `Page.captureScreenshot`) and the same source
tree passes `tsc --noEmit && vite build`. While inspecting them at 1600×1000,
one defect was fixed: the long ocean
swells used for water normals aliased into fabric-like streaks at grazing
view angles, so their amplitude now fades with camera distance (the fine chop
is kept for close-up sparkle). No other visible regressions were found: colours
match the minimap/legend, the sky/atmosphere gradient is smooth, the sea reads
blue at depth and turquoise at the shelf, and coastlines/cliff skirts are
crisp.

## Limitations / notes

* **Shared machine.** These numbers were taken while other workstreams ran
  `cargo test`/`rustc`/clippy jobs on the same 16-core host; load average was
  logged before/after each run and is quoted with the results. Anything that
  looks like a random 33–50 ms frame correlates with those bursts. The "best"
  figures are the cleanest observed runs; the "loaded" figures are a
  conservative floor, not the renderer's steady-state cost.
* **DPR 1.** Headless Chrome runs at devicePixelRatio 1. The renderer caps DPR
  at 2, so on HiDPI displays the raster count can be up to 4× higher (≈2.6 M
  px at 916×715 CSS × DPR 2). That configuration was not measured on this
  iGPU — expect 384² to be safe and 512² to sit close to the 60 fps budget.
* 1024² works but is not a 60 fps interactive target (see above); 384²/512² are
  recommended. The engine's own large-grid path is tiled generation, which is a
  different (offline) mode.
* Frame times are compositor-capped rAF intervals, i.e. what the user sees.
  GPU-internal cost is not separately instrumented; the uncapped runs and the
  `show()` timings bracket it.
* Hardware GL in headless Chrome is flag-dependent (`--enable-gpu
  --use-gl=angle --use-angle=gl`); without those flags Chrome silently falls
  back to SwiftShader, and any "60 fps" measurement in that mode is not a
  hardware measurement.

## Reproduce

```bash
# terminal 1: production build + static server
cd web && ./node_modules/.bin/tsc --noEmit && ./node_modules/.bin/vite build
cd dist && python3 -m http.server 4001

# terminal 2: hardware-GL headless Chrome with CDP
google-chrome --headless=new --enable-gpu --use-gl=angle --use-angle=gl \
  --no-sandbox --disable-dev-shm-usage --window-size=1600,1000 \
  --remote-debugging-port=9222 http://127.0.0.1:4001/

# then drive it via the CDP helper used for these numbers
# (inject a requestAnimationFrame interval probe and read #hud-fps)
```
