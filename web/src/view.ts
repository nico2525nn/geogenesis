import * as THREE from "three";
import type { ColorMode } from "./types";

/** Hypsometric tint ramp: deep ocean to snow. */
const RAMP: Array<[number, [number, number, number]]> = [
  [-1.0, [13, 42, 82]],
  [-0.35, [23, 78, 132]],
  [-0.02, [64, 130, 170]],
  [0.0, [194, 178, 128]],
  [0.08, [106, 148, 74]],
  [0.3, [74, 110, 62]],
  [0.55, [122, 106, 80]],
  [0.78, [168, 156, 134]],
  [1.0, [245, 245, 245]],
];

const BIOME_COLORS: Array<[number, number, number]> = [
  [34, 110, 60], // tropical rainforest
  [74, 140, 70], // temperate forest
  [46, 100, 90], // boreal
  [140, 160, 80], // grassland
  [210, 180, 110], // desert
  [150, 160, 150], // tundra
  [240, 244, 250], // ice
  [160, 150, 90], // mediterranean
  [170, 150, 80], // savanna
];

function rampColor(t: number): [number, number, number] {
  const x = Math.min(1, Math.max(-1, t));
  for (let i = 1; i < RAMP.length; i += 1) {
    const upper = RAMP[i];
    const lower = RAMP[i - 1];
    if (upper === undefined || lower === undefined) continue;
    if (x <= upper[0]) {
      const f = (x - lower[0]) / (upper[0] - lower[0]);
      return [
        lower[1][0] + (upper[1][0] - lower[1][0]) * f,
        lower[1][1] + (upper[1][1] - lower[1][1]) * f,
        lower[1][2] + (upper[1][2] - lower[1][2]) * f,
      ];
    }
  }
  const last = RAMP[RAMP.length - 1];
  return last === undefined ? [128, 128, 128] : last[1];
}

export interface TerrainData {
  width: number;
  height: number;
  elevation: Float64Array;
  seaLevel: number;
  biomes: Uint8Array | null;
}

export class TerrainView {
  private renderer: THREE.WebGLRenderer;
  private scene = new THREE.Scene();
  private camera: THREE.PerspectiveCamera;
  private terrain: THREE.Mesh | null = null;
  private sea: THREE.Mesh | null = null;
  private mode: ColorMode = "relief";
  private exaggeration = 1.0;

  constructor(private canvas: HTMLCanvasElement) {
    this.renderer = new THREE.WebGLRenderer({ canvas, antialias: true });
    this.renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
    this.camera = new THREE.PerspectiveCamera(45, 1, 0.1, 1e7);
    this.camera.position.set(0, -1.4, 0.9);
    this.camera.up.set(0, 0, 1);
    this.camera.lookAt(0, 0, 0);
    this.scene.add(new THREE.HemisphereLight(0xbfd4ff, 0x3a3227, 0.9));
    const sun = new THREE.DirectionalLight(0xfff2dd, 1.6);
    sun.position.set(-0.6, -0.8, 1.0);
    this.scene.add(sun);
    new ResizeObserver(() => this.resize()).observe(canvas.parentElement ?? canvas);
    this.resize();
  }

  setMode(mode: ColorMode): void {
    this.mode = mode;
  }

  setExaggeration(value: number): void {
    this.exaggeration = value;
  }

  private resize(): void {
    const parent = this.canvas.parentElement;
    const w = parent?.clientWidth ?? 800;
    const h = Math.max(200, (parent?.clientHeight ?? 600) - 140);
    this.renderer.setSize(w, h, false);
    this.camera.aspect = w / h;
    this.camera.updateProjectionMatrix();
  }

  show(data: TerrainData): void {
    const { width, height, elevation } = data;
    if (this.terrain !== null) {
      this.scene.remove(this.terrain);
      this.terrain.geometry.dispose();
      (this.terrain.material as THREE.Material).dispose();
      this.terrain = null;
    }
    if (this.sea !== null) {
      this.scene.remove(this.sea);
      this.sea.geometry.dispose();
      (this.sea.material as THREE.Material).dispose();
      this.sea = null;
    }

    const positions = new Float32Array(width * height * 3);
    const colors = new Float32Array(width * height * 3);
    const indices: number[] = [];
    let min = Infinity;
    let max = -Infinity;
    for (const v of elevation) {
      if (Number.isFinite(v)) {
        if (v < min) min = v;
        if (v > max) max = v;
      }
    }
    if (!Number.isFinite(min)) {
      min = 0;
      max = 1;
    }
    const span = Math.max(1e-9, max - min);
    const world = 2.0;
    const stepX = world / Math.max(1, width - 1);
    const stepY = world / Math.max(1, height - 1);

    for (let y = 0; y < height; y += 1) {
      for (let x = 0; x < width; x += 1) {
        const i = y * width + x;
        const z = elevation[i] ?? 0;
        positions[i * 3] = x * stepX - world / 2;
        positions[i * 3 + 1] = y * stepY - world / 2;
        positions[i * 3 + 2] = ((z - min) / span - 0.35) * 0.5 * this.exaggeration;
        const [r, g, b] = this.colorFor(data, x, y, z, min, span);
        colors[i * 3] = r / 255;
        colors[i * 3 + 1] = g / 255;
        colors[i * 3 + 2] = b / 255;
      }
    }
    for (let y = 0; y < height - 1; y += 1) {
      for (let x = 0; x < width - 1; x += 1) {
        const a = y * width + x;
        const b2 = a + 1;
        const c = a + width;
        const d = c + 1;
        indices.push(a, c, b2, b2, c, d);
      }
    }
    const geometry = new THREE.BufferGeometry();
    geometry.setAttribute("position", new THREE.BufferAttribute(positions, 3));
    geometry.setAttribute("color", new THREE.BufferAttribute(colors, 3));
    geometry.setIndex(indices);
    geometry.computeVertexNormals();
    const material = new THREE.MeshStandardMaterial({ vertexColors: true, roughness: 0.95, metalness: 0 });
    this.terrain = new THREE.Mesh(geometry, material);
    this.scene.add(this.terrain);

    // Sea plane at the datum.
    const seaZ = ((data.seaLevel - min) / span - 0.35) * 0.5 * this.exaggeration;
    const seaGeo = new THREE.PlaneGeometry(world * 1.2, world * 1.2);
    const seaMat = new THREE.MeshStandardMaterial({
      color: 0x1d4e79,
      transparent: true,
      opacity: 0.72,
      roughness: 0.25,
      metalness: 0.1,
    });
    this.sea = new THREE.Mesh(seaGeo, seaMat);
    this.sea.position.set(0, 0, seaZ);
    this.scene.add(this.sea);
    this.render();
  }

  private colorFor(
    data: TerrainData,
    x: number,
    y: number,
    z: number,
    min: number,
    span: number,
  ): [number, number, number] {
    const t = ((z - min) / span) * 2 - 1;
    switch (this.mode) {
      case "hypsometric":
        return rampColor(t);
      case "slope": {
        const s = this.slopeAt(data, x, y);
        const k = Math.min(1, s / 0.6);
        return [Math.round(40 + 200 * k), Math.round(60 + 40 * (1 - k)), Math.round(80 + 40 * (1 - k))];
      }
      case "biome": {
        const b = data.biomes?.[y * data.width + x] ?? 6;
        return BIOME_COLORS[b] ?? [128, 128, 128];
      }
      case "relief":
      default: {
        // Hillshade from the gradient plus hypsometric tint.
        const g = this.gradientAt(data, x, y);
        const shade = Math.max(0.25, 1 - (g.x * -0.5 + g.y * -0.7));
        const [r, gg, b2] = rampColor(t);
        return [Math.round(r * shade), Math.round(gg * shade), Math.round(b2 * shade)];
      }
    }
  }

  private gradientAt(data: TerrainData, x: number, y: number): { x: number; y: number } {
    const { width, height, elevation } = data;
    const at = (xx: number, yy: number): number => {
      const cx = Math.min(width - 1, Math.max(0, xx));
      const cy = Math.min(height - 1, Math.max(0, yy));
      return elevation[cy * width + cx] ?? 0;
    };
    return { x: (at(x + 1, y) - at(x - 1, y)) / 2, y: (at(x, y + 1) - at(x, y - 1)) / 2 };
  }

  private slopeAt(data: TerrainData, x: number, y: number): number {
    const g = this.gradientAt(data, x, y);
    return Math.hypot(g.x, g.y) / 1000;
  }

  render(): void {
    this.renderer.render(this.scene, this.camera);
  }
}
