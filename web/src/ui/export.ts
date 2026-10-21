// Export menu, download helpers and reproducible project metadata.
//
// The menu is keyboard accessible (Escape closes, arrow keys move, focus
// returns to the trigger) and every download gets a descriptive file name.

export interface ExportHandlers {
  onScreenshot(): void;
  onObj(): void;
  onXyz(): void;
  onRaw(): void;
  onParams(): void;
  onStats(): void;
  onSaveProject(): void;
  onLoadProject(): void;
  onHeight16(): void;
  onNormals(): void;
  onGltf(): void;
  onProfile(): void;
  onExr(): void;
  onNetcdf(): void;
}

const ITEMS: Array<{ key: keyof ExportHandlers; label: string; hint: string; format: string }> = [
  { key: "onSaveProject", label: "Save project", hint: "parameters + time + view mode", format: "json" },
  { key: "onLoadProject", label: "Load project…", hint: "restore a saved world", format: "json" },
  { key: "onScreenshot", label: "Viewport PNG", hint: "current 3D view", format: "png" },
  { key: "onObj", label: "Terrain OBJ", hint: "mesh for Blender / GIS", format: "obj" },
  { key: "onXyz", label: "Point cloud XYZ", hint: "text point cloud", format: "xyz" },
  { key: "onRaw", label: "Heightmap F32", hint: "raw little-endian f32", format: "f32" },
  { key: "onHeight16", label: "16-bit heightmap (PNG)", hint: "GIS-ready grayscale PNG", format: "png" },
  { key: "onNormals", label: "Normal map (PNG)", hint: "tangent-space surface normals", format: "png" },
  { key: "onGltf", label: "3D mesh (glTF)", hint: "self-contained glTF 2.0", format: "gltf" },
  { key: "onProfile", label: "Cross-section (PNG)", hint: "mid-line section image", format: "png" },
  { key: "onExr", label: "OpenEXR heightmap", hint: "32-bit float EXR heightfield", format: "exr" },
  { key: "onNetcdf", label: "NetCDF fields", hint: "classic NetCDF-3 field set", format: "nc" },
  { key: "onParams", label: "Parameters JSON", hint: "reproducible project", format: "json" },
  { key: "onStats", label: "Statistics JSON", hint: "full report", format: "json" },
];

/** Wires an export dropdown; returns a cleanup function. */
export function setupExportMenu(
  button: HTMLButtonElement,
  menu: HTMLElement,
  handlers: ExportHandlers,
): () => void {
  menu.replaceChildren();
  menu.setAttribute("role", "menu");
  menu.setAttribute("aria-label", "Export data");
  const entries: HTMLButtonElement[] = [];
  for (const item of ITEMS) {
    const entry = document.createElement("button");
    entry.type = "button";
    entry.className = "menu-item";
    entry.setAttribute("role", "menuitem");
    entry.tabIndex = -1;
    const label = document.createElement("span");
    label.className = "menu-item-label";
    label.textContent = item.label;
    const hint = document.createElement("small");
    hint.textContent = item.hint;
    entry.append(label, hint);
    entry.addEventListener("click", () => {
      closeMenu(false);
      handlers[item.key]();
    });
    entry.addEventListener("keydown", (event) => {
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        const index = entries.indexOf(entry);
        const delta = event.key === "ArrowDown" ? 1 : -1;
        const next = entries[(index + delta + entries.length) % entries.length];
        next?.focus();
      }
    });
    menu.appendChild(entry);
    entries.push(entry);
  }

  let open = false;

  const openMenu = (): void => {
    open = true;
    menu.hidden = false;
    button.setAttribute("aria-expanded", "true");
    entries[0]?.focus();
  };

  const closeMenu = (returnFocus: boolean): void => {
    if (!open) {
      menu.hidden = true;
      return;
    }
    open = false;
    menu.hidden = true;
    button.setAttribute("aria-expanded", "false");
    if (returnFocus) button.focus();
  };

  button.setAttribute("aria-haspopup", "menu");
  button.setAttribute("aria-expanded", "false");
  const toggle = (event: MouseEvent): void => {
    event.stopPropagation();
    if (open) closeMenu(false);
    else openMenu();
  };
  const onDocumentClick = (): void => closeMenu(false);
  const onDocumentKeyDown = (event: KeyboardEvent): void => {
    if (event.key === "Escape" && open) {
      event.stopPropagation();
      closeMenu(true);
    }
  };
  const onMenuBlur = (event: FocusEvent): void => {
    if (!open) return;
    const next = event.relatedTarget;
    if (next instanceof Node && (menu.contains(next) || button.contains(next))) return;
    closeMenu(false);
  };
  button.addEventListener("click", toggle);
  document.addEventListener("click", onDocumentClick);
  document.addEventListener("keydown", onDocumentKeyDown);
  menu.addEventListener("focusout", onMenuBlur);
  return () => {
    button.removeEventListener("click", toggle);
    document.removeEventListener("click", onDocumentClick);
    document.removeEventListener("keydown", onDocumentKeyDown);
    menu.removeEventListener("focusout", onMenuBlur);
  };
}

/** Downloads a text file. */
export function downloadText(filename: string, text: string, mime = "text/plain"): void {
  downloadBlob(filename, new Blob([text], { type: `${mime};charset=utf-8` }));
}

/** Downloads a binary file with an optional MIME type (e.g. image/png). */
export function downloadBytes(
  filename: string,
  bytes: Uint8Array,
  mime = "application/octet-stream",
): void {
  const copy = new Uint8Array(bytes.length);
  copy.set(bytes);
  downloadBlob(filename, new Blob([copy], { type: mime }));
}

/** Downloads a data URL (for canvas screenshots). */
export function downloadDataUrl(filename: string, dataUrl: string): void {
  const link = document.createElement("a");
  link.href = dataUrl;
  link.download = filename;
  document.body.appendChild(link);
  link.click();
  link.remove();
}

function downloadBlob(filename: string, blob: Blob): void {
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = filename;
  document.body.appendChild(link);
  link.click();
  link.remove();
  window.setTimeout(() => URL.revokeObjectURL(url), 4000);
}

/** Metadata attached to exported JSON files. */
export interface ExportMeta {
  appVersion: string;
  preset: string | null;
  seed: number | null;
  mode: string;
  elapsedYear: number;
  totalYear: number;
  stepYear: number;
  grid: { width: number; height: number; cellKm: number } | null;
}

/** Splits an ISO timestamp into a filesystem-friendly UTC stamp. */
export function fileStamp(date = new Date()): string {
  return date.toISOString().replace(/[:.]/g, "-").slice(0, 19);
}

/** A safe slug fragment. */
export function slug(text: string, fallback = "world"): string {
  const cleaned = text
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return cleaned.length > 0 ? cleaned : fallback;
}

/** Builds `geogenesis-<preset>-s<seed>-t<time>-<what>.<ext>`. */
export function exportFilename(meta: ExportMeta, what: string, extension: string): string {
  const preset = meta.preset === null ? "custom" : slug(meta.preset);
  const seed = meta.seed === null ? "noseed" : `s${meta.seed}`;
  const time = `t${Math.round(meta.elapsedYear)}yr`;
  return `geogenesis-${preset}-${seed}-${time}-${slug(what)}.${extension}`;
}

/** Enriches the worker's flat `{version, values}` params JSON with names/meta. */
export function enrichParamsJson(raw: string, keys: readonly string[], meta: ExportMeta): string {
  let values: number[] = [];
  let engineVersion = "unknown";
  try {
    const parsed = JSON.parse(raw) as { version?: number | string; values?: number[] };
    if (Array.isArray(parsed.values)) values = parsed.values;
    if (parsed.version !== undefined) engineVersion = String(parsed.version);
  } catch {
    // Fall back to a minimal file rather than failing the download.
  }
  return JSON.stringify(
    {
      format: "geogenesis-project",
      version: 2,
      engine: engineVersion,
      app: meta.appVersion,
      exported_at: new Date().toISOString(),
      preset: meta.preset,
      seed: meta.seed,
      display_mode: meta.mode,
      time: { elapsed_yr: meta.elapsedYear, total_yr: meta.totalYear, step_yr: meta.stepYear },
      grid: meta.grid,
      parameters: Object.fromEntries(keys.map((key, index) => [key, values[index] ?? null])),
      // Raw flat values keep the file round-trippable through `gg_create`.
      values,
    },
    null,
    2,
  );
}

/** Adds export metadata around the engine statistics JSON. */
export function enrichStatsJson(raw: string, meta: ExportMeta): string {
  let stats: unknown = null;
  try {
    stats = JSON.parse(raw);
  } catch {
    stats = null;
  }
  return JSON.stringify(
    {
      format: "geogenesis-stats",
      version: 2,
      engine: "geogenesis-core",
      app: meta.appVersion,
      exported_at: new Date().toISOString(),
      preset: meta.preset,
      seed: meta.seed,
      display_mode: meta.mode,
      time: { elapsed_yr: meta.elapsedYear, total_yr: meta.totalYear, step_yr: meta.stepYear },
      grid: meta.grid,
      stats,
    },
    null,
    2,
  );
}
