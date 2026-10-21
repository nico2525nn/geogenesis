// Small DOM helpers shared by the UI modules: element construction, inline
// SVG icons and a single delegated tooltip layer.
//
// Keeping these here avoids each panel inventing its own element builder and
// guarantees consistent accessibility attributes and icon rendering.

/** Attribute values accepted by `el`. `true` renders a bare (boolean) attribute. */
export type AttrValue = string | number | boolean | null | undefined;

/** Create an element with attributes and children in one expression. */
export function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  attrs: Record<string, AttrValue> = {},
  ...children: Array<Node | string | number | null | undefined>
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) {
    if (value === null || value === undefined || value === false) continue;
    if (key === "class") {
      node.className = String(value);
    } else if (key === "text") {
      node.textContent = String(value);
    } else if (key === "html") {
      node.innerHTML = String(value);
    } else {
      node.setAttribute(key, value === true ? "" : String(value));
    }
  }
  append(node, ...children);
  return node;
}

/** Append children, ignoring null/undefined; strings become text nodes. */
export function append(parent: Node, ...children: Array<Node | string | number | null | undefined>): void {
  for (const child of children) {
    if (child === null || child === undefined) continue;
    parent.appendChild(typeof child === "string" || typeof child === "number" ? document.createTextNode(String(child)) : child);
  }
}

/** Remove every child of an element. */
export function clear(node: Node): void {
  while (node.firstChild !== null) {
    node.removeChild(node.firstChild);
  }
}

/** Set text content only when it changed (keeps DOM churn low during playback). */
export function setText(node: Node, text: string): void {
  if (node.textContent !== text) {
    node.textContent = text;
  }
}

/** Toggle a class and return the resulting state. */
export function toggleClass(node: Element, name: string, force?: boolean): boolean {
  return node.classList.toggle(name, force);
}

// ---------------------------------------------------------------------------
// Icons
// ---------------------------------------------------------------------------

export type IconName =
  | "play"
  | "pause"
  | "step-forward"
  | "step-back"
  | "run"
  | "rewind"
  | "dice"
  | "search"
  | "close"
  | "download"
  | "camera"
  | "reset"
  | "chevron-right"
  | "help"
  | "keyboard"
  | "sliders"
  | "chart"
  | "plus"
  | "minus"
  | "target"
  | "layers"
  | "check"
  | "alert"
  | "info"
  | "map"
  | "expand"
  | "collapse"
  | "clock"
  | "arrow-left"
  | "arrow-right"
  | "sparkles";

const ICONS: Record<IconName, string> = {
  play: '<path d="M7 4.5v15l13-7.5z" fill="currentColor" stroke="none"/>',
  pause: '<path d="M7.5 4.5h3.2v15H7.5zM13.3 4.5h3.2v15h-3.2z" fill="currentColor" stroke="none"/>',
  "step-forward":
    '<path d="M6 5v14l10-7z" fill="currentColor" stroke="none"/><path d="M18 5v14"/>',
  "step-back":
    '<path d="M18 5v14L8 12z" fill="currentColor" stroke="none"/><path d="M6 5v14"/>',
  run: '<path d="M3 5.5v13L12 12z" fill="currentColor" stroke="none"/><path d="M12 5.5v13L21 12z" fill="currentColor" stroke="none"/>',
  rewind: '<path d="M21 5.5v13L12 12z" fill="currentColor" stroke="none"/><path d="M12 5.5v13L3 12z" fill="currentColor" stroke="none"/><path d="M3 4.5v15"/>',
  dice:
    '<rect x="4" y="4" width="16" height="16" rx="3.5"/><circle cx="8.6" cy="8.6" r="1.25" fill="currentColor" stroke="none"/><circle cx="15.4" cy="15.4" r="1.25" fill="currentColor" stroke="none"/><circle cx="12" cy="12" r="1.25" fill="currentColor" stroke="none"/>',
  search: '<circle cx="11" cy="11" r="6.5"/><path d="M16 16l4.5 4.5"/>',
  close: '<path d="M6 6l12 12M18 6L6 18"/>',
  download: '<path d="M12 3v12"/><path d="M7.5 10.5L12 15l4.5-4.5"/><path d="M4.5 19.5h15"/>',
  camera:
    '<path d="M3.5 8.5h3.2l1.6-2.3h7.4l1.6 2.3h3.2v10.3h-17z"/><circle cx="12" cy="13.2" r="3.3"/>',
  reset: '<path d="M4.8 12a7.2 7.2 0 1 0 2.2-5.2"/><path d="M4.5 4.5v4h4"/>',
  "chevron-right": '<path d="M9.5 5.5L16 12l-6.5 6.5"/>',
  help: '<circle cx="12" cy="12" r="8.6"/><path d="M9.6 9.2a2.5 2.5 0 1 1 3.6 2.3c-.75.4-1.1 1-1.1 1.9"/><circle cx="12" cy="16.7" r="1" fill="currentColor" stroke="none"/>',
  keyboard:
    '<rect x="2.5" y="6" width="19" height="12" rx="2"/><path d="M6 10h1M9.5 10h1M13 10h1M16.5 10h1M6 13.5h1M9.5 13.5h1M13 13.5h1M16.5 13.5h1M8 16.5h8"/>',
  sliders: '<path d="M4 7h9M17 7h3M4 12h3M11 12h9M4 17h11M19 17h1"/><circle cx="15" cy="7" r="1.9"/><circle cx="9" cy="12" r="1.9"/><circle cx="17" cy="17" r="1.9"/>',
  chart: '<path d="M4 20V4"/><path d="M4 20h16"/><path d="M8 16v-5M12.5 16V7M17 16v-8"/>',
  plus: '<path d="M12 5v14M5 12h14"/>',
  minus: '<path d="M5 12h14"/>',
  target: '<circle cx="12" cy="12" r="7.5"/><circle cx="12" cy="12" r="2" fill="currentColor" stroke="none"/><path d="M12 2.5v3M12 18.5v3M2.5 12h3M18.5 12h3"/>',
  layers: '<path d="M12 3.5l8.5 4.6L12 12.7 3.5 8.1z"/><path d="M3.5 12.4L12 17l8.5-4.6"/><path d="M3.5 16.7L12 21.3l8.5-4.6"/>',
  check: '<path d="M5 12.5l4.5 4.5L19 7.5"/>',
  alert: '<path d="M12 3.5L21.5 20h-19z"/><path d="M12 9.5v4.5"/><circle cx="12" cy="17" r="1" fill="currentColor" stroke="none"/>',
  info: '<circle cx="12" cy="12" r="8.6"/><path d="M12 11v5.5"/><circle cx="12" cy="7.7" r="1" fill="currentColor" stroke="none"/>',
  map: '<path d="M3.5 6.5l5-2 7 2.5 5-2v13l-5 2-7-2.5-5 2z"/><path d="M8.5 4.5v13M15.5 7v13"/>',
  expand: '<path d="M9 4.5H4.5V9M15 4.5h4.5V9M9 19.5H4.5V15M15 19.5h4.5V15"/>',
  collapse: '<path d="M4.5 9H9V4.5M19.5 9H15V4.5M4.5 15H9v4.5M19.5 15H15v4.5"/>',
  clock: '<circle cx="12" cy="12" r="8.6"/><path d="M12 6.8V12l3.6 2.2"/>',
  "arrow-left": '<path d="M19 12H5"/><path d="M11 6l-6 6 6 6"/>',
  "arrow-right": '<path d="M5 12h14"/><path d="M13 6l6 6-6 6"/>',
  sparkles:
    '<path d="M11 3.5l1.7 4.4 4.4 1.7-4.4 1.7L11 15.7l-1.7-4.4L4.9 9.6l4.4-1.7z"/><path d="M18.3 14.2l.9 2.2 2.2.9-2.2.9-.9 2.2-.9-2.2-2.2-.9 2.2-.9z"/>',
};

/** Build an inline SVG icon that inherits `currentColor`. */
export function icon(name: IconName, size = 16): SVGSVGElement {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("width", String(size));
  svg.setAttribute("height", String(size));
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", "1.8");
  svg.setAttribute("stroke-linecap", "round");
  svg.setAttribute("stroke-linejoin", "round");
  svg.setAttribute("aria-hidden", "true");
  svg.setAttribute("focusable", "false");
  svg.classList.add("icon");
  svg.innerHTML = ICONS[name];
  return svg;
}

/** Build a button with an icon and an accessible label. */
export function iconButton(
  name: IconName,
  label: string,
  attrs: Record<string, AttrValue> = {},
): HTMLButtonElement {
  const button = el("button", {
    type: "button",
    class: "icon-button",
    "aria-label": label,
    "data-tip": label,
    ...attrs,
  });
  button.appendChild(icon(name));
  return button;
}

// ---------------------------------------------------------------------------
// Tooltip layer
// ---------------------------------------------------------------------------

let tooltip: HTMLDivElement | null = null;
let tooltipOwner: HTMLElement | null = null;

function ensureTooltip(): HTMLDivElement {
  if (tooltip !== null) return tooltip;
  tooltip = el("div", { class: "gg-tooltip", role: "tooltip", id: "gg-tooltip" });
  tooltip.hidden = true;
  document.body.appendChild(tooltip);
  return tooltip;
}

function showTooltip(owner: HTMLElement): void {
  const text = owner.dataset.tip;
  if (text === undefined || text.length === 0) return;
  const tip = ensureTooltip();
  tip.textContent = text;
  tip.hidden = false;
  tooltipOwner = owner;
  const ownerRect = owner.getBoundingClientRect();
  const tipRect = tip.getBoundingClientRect();
  const margin = 9;
  let left = ownerRect.left + ownerRect.width / 2 - tipRect.width / 2;
  left = Math.max(8, Math.min(left, window.innerWidth - tipRect.width - 8));
  let top = ownerRect.bottom + margin;
  if (top + tipRect.height > window.innerHeight - 8) {
    top = Math.max(8, ownerRect.top - tipRect.height - margin);
  }
  tip.style.left = `${Math.round(left)}px`;
  tip.style.top = `${Math.round(top)}px`;
}

function hideTooltip(): void {
  if (tooltip !== null) tooltip.hidden = true;
  tooltipOwner = null;
}

function handleEnter(target: EventTarget | null): void {
  if (!(target instanceof HTMLElement)) return;
  const owner = target.closest<HTMLElement>("[data-tip]");
  if (owner === null || owner === tooltipOwner) return;
  showTooltip(owner);
}

function handleLeave(target: EventTarget | null, related: EventTarget | null): void {
  if (!(target instanceof HTMLElement)) return;
  const owner = target.closest<HTMLElement>("[data-tip]");
  if (owner === null) return;
  if (related instanceof Node && owner.contains(related)) return;
  hideTooltip();
}

/** Installs the delegated tooltip listeners once. */
export function initTooltips(): void {
  document.addEventListener("pointerover", (event) => handleEnter(event.target));
  document.addEventListener("pointerout", (event) => handleLeave(event.target, event.relatedTarget));
  document.addEventListener("focusin", (event) => handleEnter(event.target));
  document.addEventListener("focusout", (event) => handleLeave(event.target, event.relatedTarget));
  window.addEventListener("scroll", hideTooltip, true);
  window.addEventListener("resize", hideTooltip);
}

/** Tiny confirm-free inline notice helper: returns a span with a dot. */
export function dot(className: string): HTMLSpanElement {
  return el("span", { class: `dot ${className}` });
}
