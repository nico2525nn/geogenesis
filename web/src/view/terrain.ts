// GeoGenesis 3D viewport (three.js).
//
// The renderer is built around a *static* terrain grid: the vertex shader
// displaces it from a floating-point height texture, so a simulation frame
// costs one small texture upload instead of rebuilding 150k vertices on the
// CPU. Smooth normals, curvature ambient occlusion and the hypsometric/overlay
// albedo are all evaluated on the GPU from linear-light palette lookup tables
// that are shared with the 2D minimap (see `palettes.ts`), so the 3D view, the
// minimap and the legend can never disagree.
//
// The public API is the contract used by `main.ts`:
//   constructor(host, { minimap }), show, setMode, setField, setProfileLine,
//   onProfileChange, setExaggeration, resetCamera, screenshot, resize, dispose
// plus two additive helpers (getMode, getExaggeration, setCatalogs).

import * as THREE from "three";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
import { legendFor } from "../colors";
import type {
  BiomeInfo,
  ColorMode,
  MineralInfo,
  ProfileLine,
  RockInfo,
  TerrainData,
} from "../types";
import {
  categoricalCount,
  categoricalPalette,
  continuousPalette,
  FALLBACK_BIOMES,
  FALLBACK_ROCKS,
  LAND_LUT,
  LAND_LUT_SIZE,
  modeRange,
  OCEAN_LUT,
  OCEAN_LUT_SIZE,
  type CategoricalMode,
  type ContinuousMode,
  type PaletteLut,
} from "./palettes";
import {
  SKIRT_FRAG,
  SKIRT_VERT,
  SKY_FRAG,
  SKY_VERT,
  TERRAIN_FRAG,
  TERRAIN_VERT,
  WATER_FRAG,
  WATER_VERT,
} from "./shaders";

export interface TerrainViewOptions {
  minimap?: HTMLCanvasElement;
  exaggeration?: number;
}

/** Horizontal world size in three.js units (map width is normalised). */
const WORLD_WIDTH = 100;
/** Vertical-exaggeration factor that maps the robust relief to ~22% of the map width. */
const DEFAULT_EXAGGERATION = 60;
const MIN_EXAGGERATION = 1;
const MAX_EXAGGERATION = 160;
/** Fraction of the world width used by the full robust elevation range at neutrality. */
const RELIEF_FRACTION = 0.12;
const MAX_PIXEL_RATIO = 2;

const DEFAULT_YAW = -0.52;
const DEFAULT_PITCH = 0.30;
const DEFAULT_DISTANCE = 1.7;
const CAMERA_TWEEN_MS = 720;

const MODE_TERRAIN = 0;
const MODE_CATEGORICAL = 1;
const MODE_CONTINUOUS = 2;

const MINIMAP_MAX_PIXELS = 384;

type DragMode = "a" | "b" | "new" | "move" | null;

interface ViewUniforms {
  uHeight: THREE.IUniform<THREE.Texture>;
  uGrid: THREE.IUniform<THREE.Vector2>;
  uWorldWidth: THREE.IUniform<number>;
  uWorldDepth: THREE.IUniform<number>;
  uUnitsPerMetre: THREE.IUniform<number>;
  uSeaLevelM: THREE.IUniform<number>;
  uAOScale: THREE.IUniform<number>;
  uLandLut: THREE.IUniform<THREE.Texture>;
  uOceanLut: THREE.IUniform<THREE.Texture>;
  uModeLut: THREE.IUniform<THREE.Texture>;
  uField: THREE.IUniform<THREE.Texture>;
  uHasField: THREE.IUniform<number>;
  uModeKind: THREE.IUniform<number>;
  uCatCount: THREE.IUniform<number>;
  uLutSize: THREE.IUniform<THREE.Vector2>;
  uSunDir: THREE.IUniform<THREE.Vector3>;
  uSunColour: THREE.IUniform<THREE.Color>;
  uSkyColour: THREE.IUniform<THREE.Color>;
  uGroundColour: THREE.IUniform<THREE.Color>;
  uHazeCool: THREE.IUniform<THREE.Color>;
  uHazeWarm: THREE.IUniform<THREE.Color>;
  uFogDensity: THREE.IUniform<number>;
  uTime: THREE.IUniform<number>;
  uReliefShade: THREE.IUniform<number>;
}

interface WaterUniforms {
  uHeight: THREE.IUniform<THREE.Texture>;
  uGrid: THREE.IUniform<THREE.Vector2>;
  uWorldWidth: THREE.IUniform<number>;
  uWorldDepth: THREE.IUniform<number>;
  uSeaLevelM: THREE.IUniform<number>;
  uSunDir: THREE.IUniform<THREE.Vector3>;
  uSunColour: THREE.IUniform<THREE.Color>;
  uSkyTop: THREE.IUniform<THREE.Color>;
  uSkyHorizon: THREE.IUniform<THREE.Color>;
  uDeep: THREE.IUniform<THREE.Color>;
  uShallow: THREE.IUniform<THREE.Color>;
  uFoam: THREE.IUniform<THREE.Color>;
  uHazeCool: THREE.IUniform<THREE.Color>;
  uHazeWarm: THREE.IUniform<THREE.Color>;
  uFogDensity: THREE.IUniform<number>;
  uTime: THREE.IUniform<number>;
  uOpacity: THREE.IUniform<number>;
}

function modeKindOf(mode: ColorMode): number {
  if (mode === "terrain") {
    return MODE_TERRAIN;
  }
  if (isCategorical(mode)) {
    return MODE_CATEGORICAL;
  }
  return MODE_CONTINUOUS;
}

function isCategorical(mode: ColorMode): mode is CategoricalMode {
  return (
    mode === "biome" ||
    mode === "rock" ||
    mode === "river" ||
    mode === "minerals" ||
    mode === "mineral_deposits"
  );
}

function isContinuous(mode: ColorMode): mode is ContinuousMode {
  return mode !== "terrain" && !isCategorical(mode);
}

function clamp(value: number, lo: number, hi: number): number {
  return value < lo ? lo : value > hi ? hi : value;
}

function smoothstep(edge0: number, edge1: number, value: number): number {
  const t = clamp((value - edge0) / Math.max(edge1 - edge0, 1e-9), 0, 1);
  return t * t * (3 - 2 * t);
}

function easeInOutCubic(t: number): number {
  return t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2;
}

function formatMetres(value: number): string {
  if (!Number.isFinite(value)) {
    return "—";
  }
  const abs = Math.abs(value);
  if (abs >= 1000) {
    return `${(value / 1000).toFixed(2)} km`;
  }
  return `${Math.round(value)} m`;
}

function formatTime(seconds: number): string {
  const abs = Math.abs(seconds);
  if (abs >= 1e6) {
    return `${(seconds / 1e6).toFixed(2)} Myr`;
  }
  if (abs >= 1e3) {
    return `${(seconds / 1e3).toFixed(1)} kyr`;
  }
  return `${Math.round(seconds)} yr`;
}

/** Typed-array view cast for three's DataTexture constructor typings. */
function bufferSource(data: Float32Array | Uint8Array): BufferSource {
  return data as unknown as BufferSource;
}

function createLutTexture(lut: PaletteLut): THREE.DataTexture {
  const texture = new THREE.DataTexture(
    bufferSource(lut.linearRgba),
    lut.size,
    1,
    THREE.RGBAFormat,
    THREE.FloatType,
  );
  texture.minFilter = THREE.NearestFilter;
  texture.magFilter = THREE.NearestFilter;
  texture.wrapS = THREE.ClampToEdgeWrapping;
  texture.wrapT = THREE.ClampToEdgeWrapping;
  texture.generateMipmaps = false;
  texture.needsUpdate = true;
  return texture;
}

function createDummyTexture(): THREE.DataTexture {
  const texture = new THREE.DataTexture(
    bufferSource(new Float32Array([0, 0, 0, 0])),
    1,
    1,
    THREE.RedFormat,
    THREE.FloatType,
  );
  texture.needsUpdate = true;
  return texture;
}

export class TerrainView {
  private readonly host: HTMLElement;
  private readonly minimap: HTMLCanvasElement | null;
  private readonly renderer: THREE.WebGLRenderer;
  private readonly scene: THREE.Scene;
  private readonly camera: THREE.PerspectiveCamera;
  private readonly controls: OrbitControls;
  private readonly clock = new THREE.Clock();

  // Shared atmosphere colours (linear light; three handles sRGB conversion).
  private readonly sunDir = new THREE.Vector3(0.86, 0.42, 0.08).normalize();
  private readonly sunColour = new THREE.Color(0xffd3a3).multiplyScalar(2.7);
  private readonly skyColour = new THREE.Color(0x8fb2e0).multiplyScalar(0.52);
  private readonly groundColour = new THREE.Color(0x5a4632).multiplyScalar(0.5);
  private readonly hazeCool = new THREE.Color(0x49688a);
  private readonly hazeWarm = new THREE.Color(0xc08a5f);

  private readonly landLutTexture: THREE.DataTexture;
  private readonly oceanLutTexture: THREE.DataTexture;
  private readonly dummyTexture: THREE.DataTexture;
  private readonly categoricalTextures = new Map<CategoricalMode, THREE.DataTexture>();
  private readonly continuousTextures = new Map<ContinuousMode, THREE.DataTexture>();
  private modeLutTexture: THREE.DataTexture;

  private biomes: readonly BiomeInfo[] = FALLBACK_BIOMES;
  private rocks: readonly RockInfo[] = FALLBACK_ROCKS;
  private minerals: readonly MineralInfo[] = [];

  private readonly terrainUniforms: ViewUniforms;
  private readonly terrainMaterial: THREE.ShaderMaterial;
  private readonly overlayMaterial: THREE.ShaderMaterial;
  private readonly terrain: THREE.Mesh;
  private terrainGeometry: THREE.BufferGeometry | null = null;

  private heightTexture: THREE.DataTexture;
  private fieldTexture: THREE.DataTexture | null = null;
  private fieldBytes: Uint8Array | null = null;

  private readonly water: THREE.Mesh;
  private readonly waterUniforms: WaterUniforms;

  private readonly skirtMaterial: THREE.ShaderMaterial;
  private readonly skirt: THREE.Mesh;
  private skirtGeometry: THREE.BufferGeometry | null = null;
  private skirtTopIndices: Int32Array | null = null;
  private skirtX: Float32Array | null = null;
  private skirtZ: Float32Array | null = null;

  private readonly sky: THREE.Mesh;
  private readonly skyMaterial: THREE.ShaderMaterial;

  private readonly profileLineObject: THREE.Line;
  private readonly profileGeometry: THREE.BufferGeometry;
  private readonly profileArray: Float32Array;
  private readonly profileMaterial: THREE.LineBasicMaterial;
  private readonly profileMarkers: [THREE.Mesh, THREE.Mesh];

  private data: TerrainData | null = null;
  private mode: ColorMode = "terrain";
  private exaggeration: number;
  private profileLine: ProfileLine | null = null;
  private profileCallback: ((line: ProfileLine) => void) | null = null;

  private gridWidth = 0;
  private gridHeight = 0;
  private worldDepth = WORLD_WIDTH;
  private reliefM = 8000;
  private unitsPerMetre = 0.0025;
  private reliefWorld = 20;
  private baseY = -12;

  private fieldValues: Float32Array | null = null;
  private fieldMode: ColorMode | null = null;
  private fieldUploaded = false;

  private readonly resizeObserver: ResizeObserver;
  private frameHandle = 0;
  private disposed = false;
  private userInteracted = false;
  private firstWorld = true;
  private waterOpacity = 1;
  private waterOpacityTarget = 1;
  private reliefShade = 1;
  private reliefShadeTarget = 1;

  private cameraTween: {
    fromPosition: THREE.Vector3;
    fromTarget: THREE.Vector3;
    toPosition: THREE.Vector3;
    toTarget: THREE.Vector3;
    elapsed: number;
    duration: number;
  } | null = null;

  // Minimap state.
  private minimapContext: CanvasRenderingContext2D | null = null;
  private minimapBase: HTMLCanvasElement | null = null;
  private minimapBaseWidth = 0;
  private minimapBaseHeight = 0;
  private minimapImage: ImageData | null = null;
  private minimapDirty = true;
  private minimapBackingWidth = 0;
  private minimapBackingHeight = 0;
  private readonly tooltip: HTMLDivElement | null;
  private dragging: DragMode = null;
  private dragOffset: [number, number] = [0, 0];
  private hoverCell: [number, number] | null = null;

  constructor(host: HTMLElement, options: TerrainViewOptions = {}) {
    this.host = host;
    this.minimap = options.minimap ?? null;
    this.exaggeration = clamp(
      options.exaggeration ?? DEFAULT_EXAGGERATION,
      MIN_EXAGGERATION,
      MAX_EXAGGERATION,
    );

    this.renderer = new THREE.WebGLRenderer({
      antialias: true,
      alpha: false,
      preserveDrawingBuffer: true,
      powerPreference: "high-performance",
    });
    this.renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, MAX_PIXEL_RATIO));
    this.renderer.setSize(host.clientWidth || 800, host.clientHeight || 600, false);
    this.renderer.outputColorSpace = THREE.SRGBColorSpace;
    this.renderer.toneMapping = THREE.NeutralToneMapping;
    this.renderer.toneMappingExposure = 1.02;
    this.renderer.setClearColor(0x05070b, 1);
    this.renderer.domElement.style.width = "100%";
    this.renderer.domElement.style.height = "100%";
    this.renderer.domElement.style.display = "block";
    this.renderer.domElement.style.touchAction = "none";
    host.replaceChildren(this.renderer.domElement);

    this.scene = new THREE.Scene();

    this.camera = new THREE.PerspectiveCamera(
      40,
      (host.clientWidth || 800) / Math.max(host.clientHeight || 600, 1),
      0.1,
      2400,
    );
    this.camera.position.set(-52, 68, 92);

    this.controls = new OrbitControls(this.camera, this.renderer.domElement);
    this.controls.target.set(0, 1.5, 0);
    this.controls.enableDamping = true;
    this.controls.dampingFactor = 0.075;
    this.controls.rotateSpeed = 0.68;
    this.controls.zoomSpeed = 0.72;
    this.controls.panSpeed = 0.6;
    this.controls.screenSpacePanning = false;
    this.controls.minDistance = 30;
    this.controls.maxDistance = 380;
    this.controls.maxPolarAngle = Math.PI * 0.492;
    this.controls.minPolarAngle = Math.PI * 0.06;
    this.controls.update();
    this.controls.addEventListener("start", this.onControlsStart);

    // LUTs (shared with the minimap) as data textures.
    this.landLutTexture = createLutTexture(LAND_LUT);
    this.oceanLutTexture = createLutTexture(OCEAN_LUT);
    this.dummyTexture = createDummyTexture();
    this.heightTexture = createDummyTexture();
    this.modeLutTexture = this.continuousOrCategoricalLutTexture(this.mode);

    this.terrainUniforms = {
      uHeight: { value: this.heightTexture },
      uGrid: { value: new THREE.Vector2(2, 2) },
      uWorldWidth: { value: WORLD_WIDTH },
      uWorldDepth: { value: WORLD_WIDTH },
      uUnitsPerMetre: { value: this.unitsPerMetre },
      uSeaLevelM: { value: 0 },
      uAOScale: { value: 240 },
      uLandLut: { value: this.landLutTexture },
      uOceanLut: { value: this.oceanLutTexture },
      uModeLut: { value: this.modeLutTexture },
      uField: { value: this.dummyTexture },
      uHasField: { value: 0 },
      uModeKind: { value: MODE_TERRAIN },
      uCatCount: { value: 1 },
      uLutSize: { value: new THREE.Vector2(LAND_LUT_SIZE, OCEAN_LUT_SIZE) },
      uSunDir: { value: this.sunDir },
      uSunColour: { value: this.sunColour },
      uSkyColour: { value: this.skyColour },
      uGroundColour: { value: this.groundColour },
      uHazeCool: { value: this.hazeCool },
      uHazeWarm: { value: this.hazeWarm },
      uFogDensity: { value: 0.0027 },
      uTime: { value: 0 },
      uReliefShade: { value: 1 },
    };

    this.terrainMaterial = new THREE.ShaderMaterial({
      uniforms: this.terrainUniforms as unknown as Record<string, THREE.IUniform>,
      vertexShader: TERRAIN_VERT,
      fragmentShader: TERRAIN_FRAG,
      side: THREE.FrontSide,
    });
    // Overlay modes skip the filmic tone curve so ramps match the legend
    // exactly; they share the same uniform objects as the terrain material.
    this.overlayMaterial = new THREE.ShaderMaterial({
      uniforms: this.terrainUniforms as unknown as Record<string, THREE.IUniform>,
      vertexShader: TERRAIN_VERT,
      fragmentShader: TERRAIN_FRAG,
      side: THREE.FrontSide,
      defines: { GG_DATA_MODE: "1" },
    });
    this.terrain = new THREE.Mesh(new THREE.BufferGeometry(), this.terrainMaterial);
    this.terrain.frustumCulled = false;
    this.scene.add(this.terrain);

    this.waterUniforms = {
      uHeight: { value: this.heightTexture },
      uGrid: { value: new THREE.Vector2(2, 2) },
      uWorldWidth: { value: WORLD_WIDTH },
      uWorldDepth: { value: WORLD_WIDTH },
      uSeaLevelM: { value: 0 },
      uSunDir: { value: this.sunDir },
      uSunColour: { value: this.sunColour },
      uSkyTop: { value: new THREE.Color(0x3a6d9e) },
      uSkyHorizon: { value: new THREE.Color(0xc09573) },
      uDeep: { value: new THREE.Color(0x0c4a77) },
      uShallow: { value: new THREE.Color(0x2fa3b2) },
      uFoam: { value: new THREE.Color(0xd8eaec) },
      uHazeCool: { value: this.hazeCool },
      uHazeWarm: { value: this.hazeWarm },
      uFogDensity: { value: 0.0027 },
      uTime: { value: 0 },
      uOpacity: { value: 1 },
    };
    this.water = new THREE.Mesh(
      new THREE.PlaneGeometry(WORLD_WIDTH, WORLD_WIDTH),
      new THREE.ShaderMaterial({
        uniforms: this.waterUniforms as unknown as Record<string, THREE.IUniform>,
        vertexShader: WATER_VERT,
        fragmentShader: WATER_FRAG,
        transparent: true,
        depthWrite: false,
        side: THREE.FrontSide,
      }),
    );
    this.water.renderOrder = 2;
    this.water.position.y = 0;
    this.water.frustumCulled = false;
    this.scene.add(this.water);

    this.skirtMaterial = new THREE.ShaderMaterial({
      uniforms: {
        uSunDir: { value: this.sunDir },
        uSunColour: { value: this.sunColour },
        uSkyColour: { value: this.skyColour },
        uHazeCool: { value: this.hazeCool },
        uHazeWarm: { value: this.hazeWarm },
        uFogDensity: { value: 0.0027 },
        uBaseY: { value: -12 },
        uGroundColour: { value: this.groundColour },
        uStrataA: { value: new THREE.Color(0xae946a) },
        uStrataB: { value: new THREE.Color(0x82766a) },
        uStrataC: { value: new THREE.Color(0x5f564d) },
      },
      vertexShader: SKIRT_VERT,
      fragmentShader: SKIRT_FRAG,
      side: THREE.DoubleSide,
    });
    this.skirt = new THREE.Mesh(new THREE.BufferGeometry(), this.skirtMaterial);
    this.skirt.frustumCulled = false;
    this.scene.add(this.skirt);

    this.skyMaterial = new THREE.ShaderMaterial({
      uniforms: {
        uSunDir: { value: this.sunDir },
        uZenith: { value: new THREE.Color(0x0a1424) },
        uMidSky: { value: new THREE.Color(0x243a55) },
        uHorizon: { value: new THREE.Color(0x5f7690) },
        uVoid: { value: new THREE.Color(0x04070d) },
        uSunGlow: { value: new THREE.Color(0xff9d54) },
        uTime: { value: 0 },
      },
      vertexShader: SKY_VERT,
      fragmentShader: SKY_FRAG,
      side: THREE.BackSide,
      depthWrite: false,
      depthTest: false,
    });
    this.sky = new THREE.Mesh(new THREE.SphereGeometry(500, 48, 24), this.skyMaterial);
    this.sky.renderOrder = -10;
    this.sky.frustumCulled = false;
    this.scene.add(this.sky);

    // Cross-section annotation: a line hovering just above the surface.
    this.profileArray = new Float32Array(512 * 3);
    this.profileGeometry = new THREE.BufferGeometry();
    this.profileGeometry.setAttribute(
      "position",
      new THREE.BufferAttribute(this.profileArray, 3),
    );
    this.profileGeometry.setDrawRange(0, 0);
    this.profileMaterial = new THREE.LineBasicMaterial({
      color: 0xffd27a,
      transparent: true,
      opacity: 0.96,
      depthTest: false,
      toneMapped: false,
    });
    this.profileLineObject = new THREE.Line(this.profileGeometry, this.profileMaterial);
    this.profileLineObject.renderOrder = 20;
    this.profileLineObject.frustumCulled = false;
    this.profileLineObject.visible = false;
    this.scene.add(this.profileLineObject);

    const markerGeometry = new THREE.SphereGeometry(0.85, 12, 8);
    const markerMaterial = new THREE.MeshBasicMaterial({
      color: 0xffe8ae,
      transparent: true,
      opacity: 0.95,
      depthTest: false,
    });
    this.profileMarkers = [
      new THREE.Mesh(markerGeometry, markerMaterial),
      new THREE.Mesh(markerGeometry, markerMaterial),
    ];
    for (const marker of this.profileMarkers) {
      marker.renderOrder = 21;
      marker.frustumCulled = false;
      marker.visible = false;
      this.scene.add(marker);
    }

    this.resizeObserver = new ResizeObserver(() => this.resize());
    this.resizeObserver.observe(host);

    if (this.minimap !== null) {
      this.minimapContext = this.minimap.getContext("2d");
      this.tooltip = this.createTooltip();
      this.minimap.addEventListener("pointerdown", this.onMinimapDown);
      this.minimap.addEventListener("pointermove", this.onMinimapMove);
      this.minimap.addEventListener("pointerup", this.onMinimapUp);
      this.minimap.addEventListener("pointercancel", this.onMinimapUp);
      this.minimap.addEventListener("pointerleave", this.onMinimapLeave);
    } else {
      this.tooltip = null;
    }

    this.applyModeVisuals();

    this.animate = this.animate.bind(this);
    this.frameHandle = requestAnimationFrame(this.animate);
  }

  // ------------------------------------------------------------------ public

  /** Replaces the displayed frame (geometry is only rebuilt on grid changes). */
  show(data: TerrainData): void {
    const gridChanged =
      this.data === null ||
      this.data.width !== data.width ||
      this.data.height !== data.height;
    this.data = data;
    if (gridChanged) {
      this.prepareWorld();
    }
    this.uploadHeightTexture();
    this.syncFieldForMode();
    this.updateVerticalUniforms();
    this.updateSkirt();
    this.updateProfileOverlay();
    this.drawMinimap(true);
  }

  /** Changes the overlay colour mode. */
  setMode(mode: ColorMode): void {
    if (this.mode === mode) {
      return;
    }
    this.mode = mode;
    this.applyModeVisuals();
    this.syncFieldForMode();
    this.drawMinimap(true);
  }

  /** Returns the current overlay mode. */
  getMode(): ColorMode {
    return this.mode;
  }

  /**
   * Supplies the authoritative biome/rock/mineral catalogues from the engine
   * schema. The mineral catalogue is optional: when it is missing (or shorter
   * than the field's indices) the mineral maps fall back to grey rather than
   * throwing.
   */
  setCatalogs(
    biomes: readonly BiomeInfo[],
    rocks: readonly RockInfo[],
    minerals?: readonly MineralInfo[],
  ): void {
    this.biomes = biomes.length > 0 ? biomes : FALLBACK_BIOMES;
    this.rocks = rocks.length > 0 ? rocks : FALLBACK_ROCKS;
    this.minerals = minerals ?? [];
    for (const texture of this.categoricalTextures.values()) {
      texture.dispose();
    }
    this.categoricalTextures.clear();
    this.modeLutTexture = this.continuousOrCategoricalLutTexture(this.mode);
    this.terrainUniforms.uModeLut.value = this.modeLutTexture;
    if (isCategorical(this.mode)) {
      this.terrainUniforms.uCatCount.value = categoricalCount(
        this.mode,
        this.biomes,
        this.rocks,
        this.minerals,
      );
    }
    this.drawMinimap(true);
  }

  /** Adds or replaces an extra overlay field (e.g. after an async fetch). */
  setField(mode: ColorMode, values: Float32Array): void {
    if (this.data === null || values.length !== this.gridWidth * this.gridHeight) {
      return;
    }
    this.fieldMode = mode;
    this.fieldValues = values;
    if (this.mode === mode) {
      this.uploadFieldTexture(values, mode);
      this.drawMinimap(true);
    } else {
      this.fieldUploaded = false;
    }
  }

  /** Sets the cross-section line drawn on the minimap and the terrain. */
  setProfileLine(line: ProfileLine | null): void {
    this.profileLine = line;
    this.updateProfileOverlay();
    this.drawMinimap(false);
  }

  /** Registers a callback fired when the user draws a new cross-section. */
  onProfileChange(callback: (line: ProfileLine) => void): void {
    this.profileCallback = callback;
  }

  /** Sets the vertical exaggeration factor (1..160, default 60). */
  setExaggeration(factor: number): void {
    const next = clamp(Number.isFinite(factor) ? factor : DEFAULT_EXAGGERATION, MIN_EXAGGERATION, MAX_EXAGGERATION);
    if (Math.abs(next - this.exaggeration) < 1e-6) {
      return;
    }
    this.exaggeration = next;
    if (this.data !== null) {
      this.updateVerticalUniforms();
      this.updateSkirt();
      this.updateProfileOverlay();
    }
  }

  /** Current vertical exaggeration. */
  getExaggeration(): number {
    return this.exaggeration;
  }

  /** Smoothly resets the camera to the default 3/4 aerial framing. */
  resetCamera(): void {
    const framing = this.computeFraming();
    this.cameraTween = {
      fromPosition: this.camera.position.clone(),
      fromTarget: this.controls.target.clone(),
      toPosition: framing.position,
      toTarget: framing.target,
      elapsed: 0,
      duration: CAMERA_TWEEN_MS,
    };
    this.controls.enabled = false;
  }

  /** Renders and returns a PNG data URL of the viewport. */
  screenshot(): string {
    this.renderer.render(this.scene, this.camera);
    return this.renderer.domElement.toDataURL("image/png");
  }

  /** Handles container resizing. */
  resize(): void {
    const width = this.host.clientWidth;
    const height = this.host.clientHeight;
    if (width === 0 || height === 0) {
      return;
    }
    const pixelRatio = Math.min(window.devicePixelRatio || 1, MAX_PIXEL_RATIO);
    this.renderer.setPixelRatio(pixelRatio);
    this.renderer.setSize(width, height, false);
    this.camera.aspect = width / height;
    this.camera.updateProjectionMatrix();
    this.drawMinimap(false);
  }

  /** Frees GPU resources and listeners. */
  dispose(): void {
    if (this.disposed) {
      return;
    }
    this.disposed = true;
    cancelAnimationFrame(this.frameHandle);
    this.resizeObserver.disconnect();
    this.controls.removeEventListener("start", this.onControlsStart);
    this.controls.dispose();
    if (this.minimap !== null) {
      this.minimap.removeEventListener("pointerdown", this.onMinimapDown);
      this.minimap.removeEventListener("pointermove", this.onMinimapMove);
      this.minimap.removeEventListener("pointerup", this.onMinimapUp);
      this.minimap.removeEventListener("pointercancel", this.onMinimapUp);
      this.minimap.removeEventListener("pointerleave", this.onMinimapLeave);
    }
    this.tooltip?.remove();
    this.terrainGeometry?.dispose();
    this.skirtGeometry?.dispose();
    this.profileGeometry.dispose();
    this.profileMaterial.dispose();
    for (const marker of this.profileMarkers) {
      (marker.material as THREE.Material).dispose();
      marker.geometry.dispose();
    }
    this.heightTexture.dispose();
    this.fieldTexture?.dispose();
    this.dummyTexture.dispose();
    this.landLutTexture.dispose();
    this.oceanLutTexture.dispose();
    for (const texture of this.categoricalTextures.values()) {
      texture.dispose();
    }
    for (const texture of this.continuousTextures.values()) {
      texture.dispose();
    }
    this.modeLutTexture.dispose();
    this.terrainMaterial.dispose();
    this.overlayMaterial.dispose();
    (this.water.material as THREE.Material).dispose();
    this.water.geometry.dispose();
    this.skirtMaterial.dispose();
    this.skyMaterial.dispose();
    this.sky.geometry.dispose();
    this.renderer.dispose();
    this.host.replaceChildren();
  }

  // ------------------------------------------------------------------- world

  private prepareWorld(): void {
    const data = this.data;
    if (data === null) {
      return;
    }
    this.gridWidth = data.width;
    this.gridHeight = data.height;
    this.worldDepth = (WORLD_WIDTH * data.height) / Math.max(data.width, 1);

    this.computeRelief();
    this.buildTerrainGeometry();
    this.rebuildHeightTexture();
    this.buildSkirt();

    this.water.geometry.dispose();
    const waterGeometry = new THREE.PlaneGeometry(WORLD_WIDTH, this.worldDepth);
    waterGeometry.rotateX(-Math.PI / 2);
    this.water.geometry = waterGeometry;
    this.waterUniforms.uGrid.value.set(this.gridWidth, this.gridHeight);
    this.waterUniforms.uWorldWidth.value = WORLD_WIDTH;
    this.waterUniforms.uWorldDepth.value = this.worldDepth;

    this.fieldValues = null;
    this.fieldMode = null;
    this.fieldUploaded = false;
    this.terrainUniforms.uHasField.value = 0;

    this.updateVerticalUniforms();
    this.updateSkirt();
    this.updateProfileOverlay();

    if (this.firstWorld) {
      this.firstWorld = false;
      this.applyDefaultCamera();
    } else if (!this.userInteracted) {
      this.resetCamera();
    }
  }

  /** Robust elevation spread (p2..p98) used to derive the vertical scale. */
  private computeRelief(): void {
    const data = this.data;
    if (data === null) {
      return;
    }
    const elevation = data.elevation;
    const count = elevation.length;
    const stride = Math.max(1, Math.floor(count / 4096));
    const samples: number[] = [];
    for (let i = 0; i < count; i += stride) {
      samples.push(elevation[i] ?? 0);
    }
    if (samples.length === 0) {
      this.reliefM = 8000;
      return;
    }
    samples.sort((a, b) => a - b);
    const percentile = (q: number): number => {
      const index = clamp(Math.round(q * (samples.length - 1)), 0, samples.length - 1);
      return samples[index] ?? 0;
    };
    const low = percentile(0.02);
    const high = percentile(0.98);
    this.reliefM = Math.max(high - low, 800);
  }

  private buildTerrainGeometry(): void {
    const width = this.gridWidth;
    const height = this.gridHeight;
    const depth = this.worldDepth;
    const vertexCount = width * height;
    const positions = new Float32Array(vertexCount * 3);
    const normals = new Float32Array(vertexCount * 3);
    const uvs = new Float32Array(vertexCount * 2);
    const indices = new Uint32Array((width - 1) * (height - 1) * 6);

    for (let y = 0; y < height; y += 1) {
      for (let x = 0; x < width; x += 1) {
        const i = y * width + x;
        positions[i * 3] = (x / Math.max(width - 1, 1) - 0.5) * WORLD_WIDTH;
        positions[i * 3 + 1] = 0;
        positions[i * 3 + 2] = (y / Math.max(height - 1, 1) - 0.5) * depth;
        normals[i * 3 + 1] = 1;
        uvs[i * 2] = (x + 0.5) / width;
        uvs[i * 2 + 1] = (y + 0.5) / height;
      }
    }
    let cursor = 0;
    for (let y = 0; y < height - 1; y += 1) {
      for (let x = 0; x < width - 1; x += 1) {
        const a = y * width + x;
        const b = a + 1;
        const c = a + width;
        const d = c + 1;
        indices[cursor++] = a;
        indices[cursor++] = c;
        indices[cursor++] = b;
        indices[cursor++] = b;
        indices[cursor++] = c;
        indices[cursor++] = d;
      }
    }

    const geometry = new THREE.BufferGeometry();
    geometry.setAttribute("position", new THREE.BufferAttribute(positions, 3));
    geometry.setAttribute("normal", new THREE.BufferAttribute(normals, 3));
    geometry.setAttribute("uv", new THREE.BufferAttribute(uvs, 2));
    geometry.setIndex(new THREE.BufferAttribute(indices, 1));
    geometry.boundingSphere = new THREE.Sphere(
      new THREE.Vector3(0, 0, 0),
      Math.hypot(WORLD_WIDTH, depth) * 0.5 + 40,
    );
    this.terrainGeometry?.dispose();
    this.terrainGeometry = geometry;
    this.terrain.geometry = geometry;

    this.terrainUniforms.uGrid.value.set(width, height);
    this.terrainUniforms.uWorldWidth.value = WORLD_WIDTH;
    this.terrainUniforms.uWorldDepth.value = depth;
  }

  private rebuildHeightTexture(): void {
    const data = this.data;
    if (data === null) {
      return;
    }
    this.heightTexture.dispose();
    const texture = new THREE.DataTexture(
      bufferSource(data.elevation),
      data.width,
      data.height,
      THREE.RedFormat,
      THREE.FloatType,
    );
    texture.minFilter = THREE.NearestFilter;
    texture.magFilter = THREE.NearestFilter;
    texture.wrapS = THREE.ClampToEdgeWrapping;
    texture.wrapT = THREE.ClampToEdgeWrapping;
    texture.generateMipmaps = false;
    texture.unpackAlignment = 1;
    texture.needsUpdate = true;
    this.heightTexture = texture;
    this.terrainUniforms.uHeight.value = texture;
    this.waterUniforms.uHeight.value = texture;
  }

  private uploadHeightTexture(): void {
    const data = this.data;
    if (data === null) {
      return;
    }
    const image = this.heightTexture.image as unknown as {
      data: Float32Array;
      width: number;
      height: number;
    };
    if (image.width !== data.width || image.height !== data.height) {
      this.rebuildHeightTexture();
      return;
    }
    if (image.data !== data.elevation) {
      image.data = data.elevation;
    }
    this.heightTexture.needsUpdate = true;
  }

  private updateVerticalUniforms(): void {
    const data = this.data;
    if (data === null) {
      return;
    }
    const nominal = WORLD_WIDTH * RELIEF_FRACTION;
    this.unitsPerMetre =
      (nominal * (this.exaggeration / DEFAULT_EXAGGERATION)) / Math.max(this.reliefM, 1);
    this.reliefWorld = this.reliefM * this.unitsPerMetre;

    this.terrainUniforms.uUnitsPerMetre.value = this.unitsPerMetre;
    this.terrainUniforms.uSeaLevelM.value = data.seaLevel;
    this.terrainUniforms.uAOScale.value = Math.max(this.reliefM * 0.022, 40);
    this.waterUniforms.uSeaLevelM.value = data.seaLevel;
    this.water.position.y = 0;
    this.sky.position.copy(this.camera.position);
  }

  private buildSkirt(): void {
    const width = this.gridWidth;
    const height = this.gridHeight;
    const depth = this.worldDepth;

    const edgeDefs: Array<{
      points: Array<[number, number]>;
      normal: THREE.Vector3;
    }> = [
      {
        points: Array.from({ length: width }, (_, i) => [i, 0] as [number, number]),
        normal: new THREE.Vector3(0, 0.3, -1).normalize(),
      },
      {
        points: Array.from({ length: height }, (_, i) => [width - 1, i] as [number, number]),
        normal: new THREE.Vector3(1, 0.3, 0).normalize(),
      },
      {
        points: Array.from({ length: width }, (_, i) => [width - 1 - i, height - 1] as [number, number]),
        normal: new THREE.Vector3(0, 0.3, 1).normalize(),
      },
      {
        points: Array.from({ length: height }, (_, i) => [0, height - 1 - i] as [number, number]),
        normal: new THREE.Vector3(-1, 0.3, 0).normalize(),
      },
    ];

    let vertexTotal = 0;
    for (const edge of edgeDefs) {
      vertexTotal += Math.max(edge.points.length - 1, 0) * 6;
    }
    const positions = new Float32Array(vertexTotal * 3);
    const normals = new Float32Array(vertexTotal * 3);
    const topIndices = new Int32Array(vertexTotal).fill(-1);
    const xs = new Float32Array(vertexTotal);
    const zs = new Float32Array(vertexTotal);

    let cursor = 0;
    const push = (
      gridIndex: number,
      gx: number,
      gy: number,
      normal: THREE.Vector3,
    ): void => {
      const worldX = (gx / Math.max(width - 1, 1) - 0.5) * WORLD_WIDTH;
      const worldZ = (gy / Math.max(height - 1, 1) - 0.5) * depth;
      positions[cursor * 3] = worldX;
      positions[cursor * 3 + 1] = 0;
      positions[cursor * 3 + 2] = worldZ;
      normals[cursor * 3] = normal.x;
      normals[cursor * 3 + 1] = normal.y;
      normals[cursor * 3 + 2] = normal.z;
      topIndices[cursor] = gridIndex;
      xs[cursor] = worldX;
      zs[cursor] = worldZ;
      cursor += 1;
    };

    for (const edge of edgeDefs) {
      const points = edge.points;
      for (let i = 0; i < points.length - 1; i += 1) {
        const a = points[i]!;
        const b = points[i + 1]!;
        const indexA = a[1] * width + a[0];
        const indexB = b[1] * width + b[0];
        // Two triangles per segment: top edge down to the base of the slab.
        push(indexA, a[0], a[1], edge.normal);
        push(indexB, b[0], b[1], edge.normal);
        push(-1, b[0], b[1], edge.normal);
        push(indexA, a[0], a[1], edge.normal);
        push(-1, b[0], b[1], edge.normal);
        push(-1, a[0], a[1], edge.normal);
      }
    }

    const geometry = new THREE.BufferGeometry();
    geometry.setAttribute("position", new THREE.BufferAttribute(positions, 3));
    geometry.setAttribute("normal", new THREE.BufferAttribute(normals, 3));
    this.skirtGeometry?.dispose();
    this.skirtGeometry = geometry;
    this.skirt.geometry = geometry;
    this.skirtTopIndices = topIndices;
    this.skirtX = xs;
    this.skirtZ = zs;
  }

  private updateSkirt(): void {
    const data = this.data;
    const geometry = this.skirtGeometry;
    const topIndices = this.skirtTopIndices;
    const xs = this.skirtX;
    const zs = this.skirtZ;
    if (data === null || geometry === null || topIndices === null || xs === null || zs === null) {
      return;
    }
    const attribute = geometry.getAttribute("position") as THREE.BufferAttribute;
    const positions = attribute.array as Float32Array;
    const elevation = data.elevation;
    const seaLevel = data.seaLevel;
    const units = this.unitsPerMetre;

    let minBorder = 0;
    for (let v = 0; v < topIndices.length; v += 1) {
      const gridIndex = topIndices[v]!;
      let y: number;
      if (gridIndex >= 0) {
        y = ((elevation[gridIndex] ?? 0) - seaLevel) * units;
        // Underwater borders are walled up to the waterline so the sea reads
        // as a contained cross-section instead of a floating glass sheet.
        if (y < 0) {
          y = 0;
        }
        if (y < minBorder) {
          minBorder = y;
        }
      } else {
        y = 0;
      }
      positions[v * 3] = xs[v] ?? 0;
      positions[v * 3 + 1] = y;
      positions[v * 3 + 2] = zs[v] ?? 0;
    }
    // Base goes below the lowest rim so the slab always reads as solid rock.
    this.baseY = Math.min(minBorder, 0) - 9;
    for (let v = 0; v < topIndices.length; v += 1) {
      if (topIndices[v] === -1) {
        positions[v * 3 + 1] = this.baseY;
      }
    }
    attribute.needsUpdate = true;
    this.skirtMaterial.uniforms.uBaseY!.value = this.baseY;
  }

  // ------------------------------------------------------------------ fields

  private continuousOrCategoricalLutTexture(mode: ColorMode): THREE.DataTexture {
    if (isCategorical(mode)) {
      let texture = this.categoricalTextures.get(mode);
      if (texture === undefined) {
        texture = createLutTexture(
          categoricalPalette(mode, this.biomes, this.rocks, this.minerals),
        );
        this.categoricalTextures.set(mode, texture);
      }
      return texture;
    }
    const continuousMode: ContinuousMode = isContinuous(mode) ? mode : "slope";
    let texture = this.continuousTextures.get(continuousMode);
    if (texture === undefined) {
      texture = createLutTexture(continuousPalette(continuousMode));
      this.continuousTextures.set(continuousMode, texture);
    }
    return texture;
  }

  private applyModeVisuals(): void {
    const kind = modeKindOf(this.mode);
    this.terrainUniforms.uModeKind.value = kind;
    this.modeLutTexture = this.continuousOrCategoricalLutTexture(this.mode);
    this.terrainUniforms.uModeLut.value = this.modeLutTexture;
    this.terrain.material = kind === MODE_TERRAIN ? this.terrainMaterial : this.overlayMaterial;
    if (kind === MODE_CATEGORICAL) {
      this.terrainUniforms.uCatCount.value = categoricalCount(
        this.mode as CategoricalMode,
        this.biomes,
        this.rocks,
        this.minerals,
      );
    }
    let water = 0.32;
    let relief = 0.36;
    switch (this.mode) {
      case "terrain":
        water = 1;
        relief = 1;
        break;
      case "biome":
        water = 0.28;
        relief = 0.55;
        break;
      case "rock":
        water = 0.22;
        relief = 0.52;
        break;
      case "minerals":
        // Mineral maps are read as data: keep the palette flat and the water
        // pane nearly invisible so the categorical colours stay true.
        water = 0.1;
        relief = 0.3;
        break;
      case "mineral_deposits":
        water = 0.08;
        relief = 0.22;
        break;
      case "river":
        water = 0.85;
        relief = 0.5;
        break;
      case "lake":
        water = 0.9;
        relief = 0.5;
        break;
      case "ice":
      case "snow":
        water = 0.45;
        relief = 0.5;
        break;
      default:
        water = 0.16;
        relief = 0.5;
        break;
    }
    this.waterOpacityTarget = water;
    this.reliefShadeTarget = relief;
  }

  private syncFieldForMode(): void {
    const data = this.data;
    if (data === null) {
      this.terrainUniforms.uHasField.value = 0;
      return;
    }
    if (this.mode === "terrain") {
      this.terrainUniforms.uHasField.value = 0;
      return;
    }
    const incoming = data.fields?.[this.mode];
    if (incoming !== undefined && incoming.length === this.gridWidth * this.gridHeight) {
      if (incoming !== this.fieldValues || this.fieldMode !== this.mode) {
        this.fieldValues = incoming;
        this.fieldMode = this.mode;
        this.uploadFieldTexture(incoming, this.mode);
      } else if (!this.fieldUploaded) {
        this.uploadFieldTexture(incoming, this.mode);
      }
      return;
    }
    // Simulation frames do not carry overlay fields; keep the most recent
    // field for the same mode instead of flashing back to terrain colours.
    if (
      this.fieldValues !== null &&
      this.fieldMode === this.mode &&
      this.fieldValues.length === this.gridWidth * this.gridHeight
    ) {
      if (!this.fieldUploaded) {
        this.uploadFieldTexture(this.fieldValues, this.mode);
      }
      return;
    }
    this.terrainUniforms.uHasField.value = 0;
  }

  private uploadFieldTexture(values: Float32Array, mode: ColorMode): void {
    const width = this.gridWidth;
    const height = this.gridHeight;
    if (width === 0 || height === 0) {
      return;
    }
    if (this.fieldBytes === null || this.fieldBytes.length !== values.length) {
      this.fieldBytes = new Uint8Array(values.length);
      this.fieldTexture?.dispose();
      this.fieldTexture = null;
    }
    if (
      this.fieldTexture !== null &&
      ((this.fieldTexture.image as unknown as { width: number }).width !== width ||
        (this.fieldTexture.image as unknown as { height: number }).height !== height)
    ) {
      this.fieldTexture.dispose();
      this.fieldTexture = null;
      this.fieldBytes = new Uint8Array(values.length);
    }
    const bytes = this.fieldBytes;
    if (bytes === null) {
      return;
    }
    const kind = modeKindOf(mode);
    if (kind === MODE_CATEGORICAL) {
      for (let i = 0; i < values.length; i += 1) {
        const id = Math.round(values[i] ?? 0);
        bytes[i] = id < 0 ? 0 : id > 255 ? 255 : id;
      }
    } else if (isContinuous(mode)) {
      const range = modeRange(mode);
      const scale = 255 / range.span;
      for (let i = 0; i < values.length; i += 1) {
        const encoded = Math.round(((values[i] ?? range.min) - range.min) * scale);
        bytes[i] = encoded < 0 ? 0 : encoded > 255 ? 255 : encoded;
      }
    }
    const filter = kind === MODE_CATEGORICAL ? THREE.NearestFilter : THREE.LinearFilter;
    if (this.fieldTexture === null) {
      const texture = new THREE.DataTexture(
        bufferSource(bytes),
        width,
        height,
        THREE.RedFormat,
        THREE.UnsignedByteType,
      );
      texture.minFilter = filter;
      texture.magFilter = filter;
      texture.wrapS = THREE.ClampToEdgeWrapping;
      texture.wrapT = THREE.ClampToEdgeWrapping;
      texture.generateMipmaps = false;
      texture.unpackAlignment = 1;
      texture.needsUpdate = true;
      this.fieldTexture = texture;
      this.terrainUniforms.uField.value = texture;
    } else {
      if (
        this.fieldTexture.magFilter !== filter ||
        this.fieldTexture.minFilter !== filter
      ) {
        this.fieldTexture.magFilter = filter;
        this.fieldTexture.minFilter = filter;
      }
      (this.fieldTexture.image as unknown as { data: Uint8Array }).data = bytes;
      this.fieldTexture.needsUpdate = true;
    }
    this.terrainUniforms.uHasField.value = 1;
    this.fieldUploaded = true;
  }

  // ------------------------------------------------------------------- camera

  private applyDefaultCamera(): void {
    const framing = this.computeFraming();
    this.camera.position.copy(framing.position);
    this.controls.target.copy(framing.target);
    this.cameraTween = null;
    this.controls.enabled = true;
    this.controls.update();
  }

  private computeFraming(): { position: THREE.Vector3; target: THREE.Vector3 } {
    const size = this.renderer.getSize(new THREE.Vector2());
    const aspect = size.x / Math.max(size.y, 1);
    const maxDimension = Math.max(WORLD_WIDTH, this.worldDepth);
    const aspectBoost = Math.max(1, 1.12 / Math.max(aspect, 0.5));
    const distance = maxDimension * DEFAULT_DISTANCE * aspectBoost;
    const target = new THREE.Vector3(0, this.reliefWorld * 0.02, 0);
    const horizontal = Math.cos(DEFAULT_PITCH) * distance;
    const position = new THREE.Vector3(
      Math.sin(DEFAULT_YAW) * horizontal,
      Math.sin(DEFAULT_PITCH) * distance + target.y,
      Math.cos(DEFAULT_YAW) * horizontal,
    );
    return { position, target };
  }

  private readonly onControlsStart = (): void => {
    this.userInteracted = true;
    if (this.cameraTween !== null) {
      this.cameraTween = null;
      this.controls.enabled = true;
    }
  };

  private clampTarget(): void {
    const target = this.controls.target;
    const limit = Math.max(WORLD_WIDTH, this.worldDepth) * 0.85;
    target.x = clamp(target.x, -limit, limit);
    target.z = clamp(target.z, -limit, limit);
    const vertical = Math.max(this.reliefWorld, 8) * 1.4;
    target.y = clamp(target.y, -vertical, vertical);
  }

  // ------------------------------------------------------------------ profile

  private updateProfileOverlay(): void {
    const data = this.data;
    const line = this.profileLine;
    if (data === null || line === null) {
      this.profileLineObject.visible = false;
      for (const marker of this.profileMarkers) {
        marker.visible = false;
      }
      return;
    }
    const samples = Math.min(512, Math.max(48, Math.round(Math.hypot(
      line.b[0] - line.a[0],
      line.b[1] - line.a[1],
    ) * 2)));
    const attribute = this.profileGeometry.getAttribute("position") as THREE.BufferAttribute;
    const array = attribute.array as Float32Array;
    const elevation = data.elevation;
    const width = this.gridWidth;
    const height = this.gridHeight;
    const seaLevel = data.seaLevel;
    const units = this.unitsPerMetre;
    const worldWidth = WORLD_WIDTH;
    const depth = this.worldDepth;

    for (let s = 0; s < samples; s += 1) {
      const t = samples === 1 ? 0 : s / (samples - 1);
      const gx = clamp(line.a[0] + (line.b[0] - line.a[0]) * t, 0, width - 1);
      const gy = clamp(line.a[1] + (line.b[1] - line.a[1]) * t, 0, height - 1);
      const elevationValue = this.sampleElevation(elevation, width, height, gx, gy);
      const worldX = (gx / Math.max(width - 1, 1) - 0.5) * worldWidth;
      const worldZ = (gy / Math.max(height - 1, 1) - 0.5) * depth;
      const y = (elevationValue - seaLevel) * units + 0.5;
      array[s * 3] = worldX;
      array[s * 3 + 1] = y;
      array[s * 3 + 2] = worldZ;
    }
    attribute.needsUpdate = true;
    this.profileGeometry.setDrawRange(0, samples);
    this.profileLineObject.visible = true;

    const endpoints: Array<[number, number]> = [line.a, line.b];
    for (let i = 0; i < 2; i += 1) {
      const marker = this.profileMarkers[i]!;
      const point = endpoints[i]!;
      const gx = clamp(point[0], 0, width - 1);
      const gy = clamp(point[1], 0, height - 1);
      const elevationValue = this.sampleElevation(elevation, width, height, gx, gy);
      marker.position.set(
        (gx / Math.max(width - 1, 1) - 0.5) * worldWidth,
        (elevationValue - seaLevel) * units + 0.7,
        (gy / Math.max(height - 1, 1) - 0.5) * depth,
      );
      marker.visible = true;
    }
  }

  private sampleElevation(
    elevation: Float32Array,
    width: number,
    height: number,
    gx: number,
    gy: number,
  ): number {
    const x0 = Math.floor(gx);
    const y0 = Math.floor(gy);
    const x1 = Math.min(x0 + 1, width - 1);
    const y1 = Math.min(y0 + 1, height - 1);
    const fx = gx - x0;
    const fy = gy - y0;
    const a = elevation[y0 * width + x0] ?? 0;
    const b = elevation[y0 * width + x1] ?? 0;
    const c = elevation[y1 * width + x0] ?? 0;
    const d = elevation[y1 * width + x1] ?? 0;
    return (a * (1 - fx) + b * fx) * (1 - fy) + (c * (1 - fx) + d * fx) * fy;
  }

  // ----------------------------------------------------------------- minimap

  private createTooltip(): HTMLDivElement | null {
    const parent = this.minimap?.parentElement;
    if (parent === null || parent === undefined) {
      return null;
    }
    if (window.getComputedStyle(parent).position === "static") {
      parent.style.position = "relative";
    }
    const tooltip = document.createElement("div");
    tooltip.className = "gg-minimap-tooltip";
    tooltip.style.cssText = [
      "position:absolute",
      "pointer-events:none",
      "z-index:8",
      "padding:4px 8px",
      "border-radius:6px",
      "border:1px solid rgba(120,140,170,0.35)",
      "background:rgba(8,11,16,0.92)",
      "color:#cfd9e6",
      "font:11px/1.45 ui-monospace,SFMono-Regular,Menlo,Consolas,monospace",
      "white-space:nowrap",
      "box-shadow:0 2px 10px rgba(0,0,0,0.45)",
      "transform:translate(-50%,-135%)",
      "display:none",
    ].join(";");
    parent.appendChild(tooltip);
    return tooltip;
  }

  private drawMinimap(force: boolean): void {
    if (force) {
      this.minimapDirty = true;
    }
    const canvas = this.minimap;
    const ctx = this.minimapContext;
    const data = this.data;
    if (canvas === null || ctx === null || data === null) {
      return;
    }
    const rect = canvas.getBoundingClientRect();
    const cssWidth = rect.width > 1 ? rect.width : 240;
    const cssHeight = rect.height > 1 ? rect.height : 240;
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    const backingWidth = Math.max(1, Math.round(cssWidth * dpr));
    const backingHeight = Math.max(1, Math.round(cssHeight * dpr));
    if (canvas.width !== backingWidth || canvas.height !== backingHeight) {
      canvas.width = backingWidth;
      canvas.height = backingHeight;
      this.minimapBackingWidth = backingWidth;
      this.minimapBackingHeight = backingHeight;
    }
    if (this.minimapDirty || this.minimapBase === null) {
      this.renderMinimapBase();
      this.minimapDirty = false;
    }
    ctx.clearRect(0, 0, backingWidth, backingHeight);
    if (this.minimapBase !== null) {
      ctx.imageSmoothingEnabled = true;
      ctx.imageSmoothingQuality = "high";
      ctx.drawImage(this.minimapBase, 0, 0, backingWidth, backingHeight);
    }
    this.drawMinimapProfile(ctx, backingWidth, backingHeight, cssWidth);
  }

  private renderMinimapBase(): void {
    const data = this.data;
    if (data === null) {
      return;
    }
    const width = this.gridWidth;
    const height = this.gridHeight;
    const drawWidth = Math.min(width, MINIMAP_MAX_PIXELS);
    const drawHeight = Math.max(
      1,
      Math.round((drawWidth * height) / Math.max(width, 1)),
    );
    if (
      this.minimapBase === null ||
      this.minimapBaseWidth !== drawWidth ||
      this.minimapBaseHeight !== drawHeight
    ) {
      const base = document.createElement("canvas");
      base.width = drawWidth;
      base.height = drawHeight;
      this.minimapBase = base;
      this.minimapBaseWidth = drawWidth;
      this.minimapBaseHeight = drawHeight;
      this.minimapImage = null;
    }
    const base = this.minimapBase;
    if (base === null) {
      return;
    }
    const ctx = base.getContext("2d");
    if (ctx === null) {
      return;
    }
    if (this.minimapImage === null || this.minimapImage.width !== drawWidth) {
      this.minimapImage = ctx.createImageData(drawWidth, drawHeight);
    }
    const image = this.minimapImage;
    const bytes = image.data;
    const elevation = data.elevation;
    const seaLevel = data.seaLevel;
    const mode = this.mode;
    const kind = modeKindOf(mode);
    const hasField = this.fieldValues !== null && this.fieldValues.length === width * height;
    const field = hasField ? this.fieldValues : null;

    const oceanSrgb = OCEAN_LUT.srgb;
    const landSrgb = LAND_LUT.srgb;
    const oceanSize = OCEAN_LUT.size;
    const landSize = LAND_LUT.size;
    let catSrgb: Uint8Array | null = null;
    let catSize = 0;
    let contSrgb: Uint8Array | null = null;
    let contSize = 0;
    let contMin = 0;
    let contSpan = 1;
    if (kind === MODE_CATEGORICAL) {
      const palette = categoricalPalette(
        mode as CategoricalMode,
        this.biomes,
        this.rocks,
        this.minerals,
      );
      catSrgb = palette.srgb;
      catSize = palette.size;
    } else if (kind === MODE_CONTINUOUS && isContinuous(mode)) {
      const palette = continuousPalette(mode);
      contSrgb = palette.srgb;
      contSize = palette.size;
      const range = modeRange(mode);
      contMin = range.min;
      contSpan = range.span;
    }

    const xDen = Math.max(drawWidth - 1, 1);
    const yDen = Math.max(drawHeight - 1, 1);
    for (let py = 0; py < drawHeight; py += 1) {
      const gy = Math.round((py / yDen) * (height - 1));
      for (let px = 0; px < drawWidth; px += 1) {
        const gx = Math.round((px / xDen) * (width - 1));
        const cell = gy * width + gx;
        const offset = (py * drawWidth + px) * 4;
        const elevationValue = elevation[cell] ?? 0;

        // Local hillshade from a north-west light; also gives bathymetry and
        // flat data overlays a readable sense of relief.
        const left = elevation[cell - (gx > 0 ? 1 : 0)] ?? elevationValue;
        const right = elevation[cell + (gx < width - 1 ? 1 : 0)] ?? elevationValue;
        const up = elevation[cell - (gy > 0 ? width : 0)] ?? elevationValue;
        const down = elevation[cell + (gy < height - 1 ? width : 0)] ?? elevationValue;
        const gradientX = (right - left) * 0.5;
        const gradientY = (down - up) * 0.5;
        const normalX = -gradientX / 10000;
        const normalY = -gradientY / 10000;
        const normalInverse = 1 / Math.sqrt(normalX * normalX + 1 + normalY * normalY);
        const lambert = Math.max(
          0,
          (normalX * -0.5 + 0.72 + normalY * -0.5) * normalInverse,
        );

        if (mode === "terrain" || field === null) {
          const isLand = elevationValue > seaLevel;
          const coast =
            (left > seaLevel) !== isLand ||
            (right > seaLevel) !== isLand ||
            (up > seaLevel) !== isLand ||
            (down > seaLevel) !== isLand;
          const coastShade = coast ? 0.62 : 1;
          if (elevationValue <= seaLevel) {
            const depth = (seaLevel - elevationValue) / 6000;
            const t = clamp(depth, 0, 1);
            const index = Math.round(t * (oceanSize - 1)) * 3;
            bytes[offset] = oceanSrgb[index] ?? 0;
            bytes[offset + 1] = oceanSrgb[index + 1] ?? 0;
            bytes[offset + 2] = oceanSrgb[index + 2] ?? 0;
          } else {
            const above = (elevationValue - seaLevel) / 2600;
            const t = clamp(above, 0, 1);
            const index = Math.round(t * (landSize - 1)) * 3;
            const shade = (0.62 + 0.55 * lambert) * coastShade;
            bytes[offset] = Math.min(255, (landSrgb[index] ?? 0) * shade);
            bytes[offset + 1] = Math.min(255, (landSrgb[index + 1] ?? 0) * shade);
            bytes[offset + 2] = Math.min(255, (landSrgb[index + 2] ?? 0) * shade);
          }
        } else if (kind === MODE_CATEGORICAL && catSrgb !== null) {
          const id = clamp(Math.round(field[cell] ?? 0), 0, catSize - 1);
          const shade = 0.95 + 0.1 * lambert;
          const index = id * 3;
          bytes[offset] = Math.min(255, (catSrgb[index] ?? 0) * shade);
          bytes[offset + 1] = Math.min(255, (catSrgb[index + 1] ?? 0) * shade);
          bytes[offset + 2] = Math.min(255, (catSrgb[index + 2] ?? 0) * shade);
        } else if (contSrgb !== null) {
          const t = clamp(((field[cell] ?? contMin) - contMin) / contSpan, 0, 1);
          const shade = 0.95 + 0.1 * lambert;
          const index = Math.round(t * (contSize - 1)) * 3;
          bytes[offset] = Math.min(255, (contSrgb[index] ?? 0) * shade);
          bytes[offset + 1] = Math.min(255, (contSrgb[index + 1] ?? 0) * shade);
          bytes[offset + 2] = Math.min(255, (contSrgb[index + 2] ?? 0) * shade);
        }
        bytes[offset + 3] = 255;
      }
    }
    ctx.putImageData(image, 0, 0);
  }

  private drawMinimapProfile(
    ctx: CanvasRenderingContext2D,
    backingWidth: number,
    backingHeight: number,
    cssWidth: number,
  ): void {
    const line = this.profileLine;
    if (line === null || this.data === null) {
      return;
    }
    const width = this.gridWidth;
    const height = this.gridHeight;
    const scale = backingWidth / Math.max(cssWidth, 1);
    const toX = (gx: number): number => (gx / Math.max(width - 1, 1)) * backingWidth;
    const toY = (gy: number): number => (gy / Math.max(height - 1, 1)) * backingHeight;
    const ax = toX(line.a[0]);
    const ay = toY(line.a[1]);
    const bx = toX(line.b[0]);
    const by = toY(line.b[1]);

    ctx.save();
    ctx.lineCap = "round";
    // Dark scrim so the pale section line survives bright biomes.
    ctx.strokeStyle = "rgba(6, 9, 14, 0.72)";
    ctx.lineWidth = 5 * scale;
    ctx.beginPath();
    ctx.moveTo(ax, ay);
    ctx.lineTo(bx, by);
    ctx.stroke();

    ctx.strokeStyle = "rgba(255, 223, 150, 0.96)";
    ctx.lineWidth = 2 * scale;
    ctx.setLineDash([7 * scale, 5 * scale]);
    ctx.beginPath();
    ctx.moveTo(ax, ay);
    ctx.lineTo(bx, by);
    ctx.stroke();
    ctx.setLineDash([]);

    const hover = this.hoverCell;
    const radiusNear = 12;
    for (const [index, point] of [line.a, line.b].entries()) {
      const px = toX(point[0]);
      const py = toY(point[1]);
      const near =
        hover !== null &&
        Math.hypot(hover[0] - point[0], hover[1] - point[1]) <
          (radiusNear / Math.max(cssWidth, 1)) * (width - 1);
      ctx.beginPath();
      ctx.arc(px, py, (near ? 6.5 : 5) * scale, 0, Math.PI * 2);
      ctx.fillStyle = near ? "rgba(255, 249, 230, 1)" : "rgba(255, 240, 205, 0.95)";
      ctx.fill();
      ctx.lineWidth = 2 * scale;
      ctx.strokeStyle = index === 0 ? "#ffca62" : "#ffdf9e";
      ctx.stroke();
    }
    ctx.restore();
  }

  // -------------------------------------------------------------- interaction

  private minimapCell(event: PointerEvent): [number, number] {
    const canvas = this.minimap;
    if (canvas === null || this.data === null) {
      return [0, 0];
    }
    const rect = canvas.getBoundingClientRect();
    const x = ((event.clientX - rect.left) / Math.max(rect.width, 1)) * (this.gridWidth - 1);
    const y = ((event.clientY - rect.top) / Math.max(rect.height, 1)) * (this.gridHeight - 1);
    return [clamp(x, 0, this.gridWidth - 1), clamp(y, 0, this.gridHeight - 1)];
  }

  private hitTest(cell: [number, number], event: PointerEvent): DragMode {
    const canvas = this.minimap;
    const line = this.profileLine;
    if (canvas === null || line === null || this.data === null) {
      return null;
    }
    const rect = canvas.getBoundingClientRect();
    const scaleX = (this.gridWidth - 1) / Math.max(rect.width, 1);
    const scaleY = (this.gridHeight - 1) / Math.max(rect.height, 1);
    const pixelDistance = 14;
    const dx = (p: [number, number]): number => (p[0] - cell[0]) * scaleX;
    const dy = (p: [number, number]): number => (p[1] - cell[1]) * scaleY;
    if (Math.hypot(dx(line.a), dy(line.a)) < pixelDistance) {
      return "a";
    }
    if (Math.hypot(dx(line.b), dy(line.b)) < pixelDistance) {
      return "b";
    }
    // Distance from the point to the segment, in screen pixels.
    const ax = rect.left + (line.a[0] / Math.max(this.gridWidth - 1, 1)) * rect.width;
    const ay = rect.top + (line.a[1] / Math.max(this.gridHeight - 1, 1)) * rect.height;
    const bx = rect.left + (line.b[0] / Math.max(this.gridWidth - 1, 1)) * rect.width;
    const by = rect.top + (line.b[1] / Math.max(this.gridHeight - 1, 1)) * rect.height;
    const vx = bx - ax;
    const vy = by - ay;
    const lengthSq = vx * vx + vy * vy;
    if (lengthSq < 1) {
      return null;
    }
    const t = clamp(((event.clientX - ax) * vx + (event.clientY - ay) * vy) / lengthSq, 0, 1);
    const distance = Math.hypot(event.clientX - (ax + vx * t), event.clientY - (ay + vy * t));
    return distance < 7 ? "move" : null;
  }

  private updateHover(cell: [number, number] | null, event: PointerEvent | null): void {
    this.hoverCell = cell;
    const tooltip = this.tooltip;
    if (cell === null || event === null || this.data === null || tooltip === null) {
      if (tooltip !== null) {
        tooltip.style.display = "none";
      }
      if (this.dragging === null) {
        this.drawMinimap(false);
      }
      return;
    }
    const [gx, gy] = cell;
    const width = this.gridWidth;
    const height = this.gridHeight;
    const cellIndex = Math.round(gy) * width + Math.round(gx);
    const elevationValue = this.data.elevation[cellIndex] ?? 0;
    const parts = [
      `x ${Math.round(gx)} · y ${Math.round(gy)}`,
      formatMetres(elevationValue),
    ];
    const field = this.fieldValues;
    if (field !== null && this.mode !== "terrain" && field.length === width * height) {
      const value = field[cellIndex] ?? 0;
      if (isCategorical(this.mode)) {
        const id = Math.round(value);
        if (this.mode === "minerals") {
          // 255 is the engine's "barren" sentinel; the catalogue itself is an
          // index-addressed list, so a missing entry falls back to a label.
          if (id === 255) {
            parts.push("barren");
          } else {
            parts.push(this.minerals[id]?.label ?? `mineral ${id}`);
          }
        } else if (this.mode === "mineral_deposits") {
          parts.push(id > 0 ? "deposit" : "none");
        } else if (this.mode === "biome" || this.mode === "rock") {
          const catalogue: readonly (BiomeInfo | RockInfo)[] =
            this.mode === "biome" ? this.biomes : this.rocks;
          const entry = catalogue.find((item) => item.id === id);
          parts.push(entry?.label ?? `class ${id}`);
        } else {
          parts.push(id >= 2 ? "major river" : id >= 1 ? "channel" : "hillslope");
        }
      } else if (isContinuous(this.mode)) {
        const range = modeRange(this.mode);
        const decimals = range.span < 2 ? 2 : range.span < 20 ? 1 : 0;
        parts.push(`${value.toFixed(decimals)} ${range.unit}`.trim());
      }
    }
    parts.push(formatTime(this.data.time));
    tooltip.textContent = parts.join("  ·  ");
    tooltip.style.display = "block";

    const rect = this.minimap!.getBoundingClientRect();
    const parent = this.minimap!.parentElement;
    if (parent !== null) {
      const parentRect = parent.getBoundingClientRect();
      const cssX = rect.left - parentRect.left + ((gx / Math.max(width - 1, 1)) * rect.width);
      const cssY = rect.top - parentRect.top + ((gy / Math.max(height - 1, 1)) * rect.height);
      tooltip.style.left = `${cssX}px`;
      tooltip.style.top = `${cssY}px`;
    }
    if (this.dragging === null) {
      this.drawMinimap(false);
    }
  }

  private readonly onMinimapDown = (event: PointerEvent): void => {
    if (this.data === null) {
      return;
    }
    event.preventDefault();
    const cell = this.minimapCell(event);
    const hit = this.hitTest(cell, event);
    this.minimap?.setPointerCapture(event.pointerId);
    if (hit === "a" || hit === "b") {
      this.dragging = hit;
      return;
    }
    if (hit === "move" && this.profileLine !== null) {
      this.dragging = "move";
      this.dragOffset = [this.profileLine.a[0] - cell[0], this.profileLine.a[1] - cell[1]];
      return;
    }
    this.dragging = "new";
    this.profileLine = { a: [cell[0], cell[1]], b: [cell[0], cell[1]] };
    this.updateProfileOverlay();
    this.drawMinimap(false);
  };

  private readonly onMinimapMove = (event: PointerEvent): void => {
    if (this.data === null) {
      return;
    }
    const cell = this.minimapCell(event);
    if (this.dragging === null) {
      this.updateHover(cell, event);
      const canvas = this.minimap;
      if (canvas !== null) {
        const hit = this.hitTest(cell, event);
        canvas.style.cursor = hit === "a" || hit === "b" ? "grab" : hit === "move" ? "move" : "crosshair";
      }
      return;
    }
    const line = this.profileLine;
    if (line === null) {
      return;
    }
    if (this.dragging === "a") {
      this.profileLine = { a: [cell[0], cell[1]], b: line.b };
    } else if (this.dragging === "b") {
      this.profileLine = { a: line.a, b: [cell[0], cell[1]] };
    } else if (this.dragging === "move") {
      const dx = cell[0] - (line.a[0] + this.dragOffset[0]);
      const dy = cell[1] - (line.a[1] + this.dragOffset[1]);
      const shift = (point: [number, number]): [number, number] => [
        clamp(point[0] + dx, 0, this.gridWidth - 1),
        clamp(point[1] + dy, 0, this.gridHeight - 1),
      ];
      this.profileLine = { a: shift(line.a), b: shift(line.b) };
    } else {
      this.profileLine = { a: line.a, b: [cell[0], cell[1]] };
    }
    this.updateProfileOverlay();
    this.drawMinimap(false);
    this.updateHover(cell, event);
  };

  private readonly onMinimapUp = (event: PointerEvent): void => {
    if (this.dragging === null) {
      return;
    }
    const wasNew = this.dragging === "new";
    this.dragging = null;
    if (this.minimap?.hasPointerCapture(event.pointerId) === true) {
      this.minimap.releasePointerCapture(event.pointerId);
    }
    let line = this.profileLine;
    if (line === null) {
      return;
    }
    const length = Math.hypot(line.b[0] - line.a[0], line.b[1] - line.a[1]);
    if (wasNew && length < 2) {
      const span = Math.max(8, Math.round(this.gridWidth * 0.18));
      const centre: [number, number] = [line.a[0], line.a[1]];
      line = {
        a: [clamp(centre[0] - span, 0, this.gridWidth - 1), centre[1]],
        b: [clamp(centre[0] + span, 0, this.gridWidth - 1), centre[1]],
      };
      this.profileLine = line;
      this.updateProfileOverlay();
    }
    this.drawMinimap(false);
    this.profileCallback?.(line);
  };

  private readonly onMinimapLeave = (): void => {
    if (this.dragging !== null) {
      return;
    }
    this.updateHover(null, null);
  };

  // ------------------------------------------------------------------- frame

  private animate(): void {
    if (this.disposed) {
      return;
    }
    this.frameHandle = requestAnimationFrame(this.animate);
    const delta = Math.min(this.clock.getDelta(), 0.1);
    const time = this.clock.elapsedTime;

    if (this.cameraTween !== null) {
      const tween = this.cameraTween;
      tween.elapsed += delta * 1000;
      const progress = clamp(tween.elapsed / tween.duration, 0, 1);
      const eased = easeInOutCubic(progress);
      this.camera.position.lerpVectors(tween.fromPosition, tween.toPosition, eased);
      this.controls.target.lerpVectors(tween.fromTarget, tween.toTarget, eased);
      this.camera.lookAt(this.controls.target);
      if (progress >= 1) {
        this.cameraTween = null;
        this.controls.enabled = true;
        this.controls.update();
      }
    } else {
      this.controls.update();
      this.clampTarget();
    }

    const smoothing = 1 - Math.exp(-delta * 9);
    this.waterOpacity += (this.waterOpacityTarget - this.waterOpacity) * smoothing;
    this.reliefShade += (this.reliefShadeTarget - this.reliefShade) * smoothing;
    this.terrainUniforms.uReliefShade.value = this.reliefShade;
    this.waterUniforms.uOpacity!.value = this.waterOpacity;
    this.terrainUniforms.uTime.value = time;
    this.waterUniforms.uTime!.value = time;
    this.skyMaterial.uniforms.uTime!.value = time;
    this.sky.position.copy(this.camera.position);

    this.renderer.render(this.scene, this.camera);
  }
}

// Keep the legend helper importable from the view module, as before.
export { legendFor };
