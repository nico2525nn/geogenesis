// Toast notifications.
//
// A single `#toast` container hosts a small stack of transient messages. The
// container is hidden whenever the stack is empty so status probes (and screen
// readers) only see it when there is something to say.

import { el, icon } from "./dom";

export type ToastKind = "info" | "success" | "error";

const DEFAULT_TIMEOUT = 4200;
const ERROR_TIMEOUT = 8000;
const MAX_TOASTS = 3;

export class Toasts {
  private readonly host: HTMLElement;
  private readonly timers = new Map<HTMLElement, number>();

  constructor(host: HTMLElement) {
    this.host = host;
    host.replaceChildren();
    host.hidden = true;
  }

  info(message: string): void {
    this.show(message, "info");
  }

  success(message: string): void {
    this.show(message, "success");
  }

  error(message: string): void {
    this.show(message, "error");
  }

  show(message: string, kind: ToastKind = "info", timeout?: number): void {
    while (this.host.children.length >= MAX_TOASTS) {
      const oldest = this.host.firstElementChild;
      if (oldest instanceof HTMLElement) this.dismiss(oldest);
      else break;
    }
    const toast = el("div", {
      class: `toast toast-${kind}`,
      role: kind === "error" ? "alert" : "status",
    });
    const iconName = kind === "error" ? "alert" : kind === "success" ? "check" : "info";
    toast.appendChild(icon(iconName, 15));
    const text = el("span", { class: "toast-message", text: message });
    const close = el("button", {
      type: "button",
      class: "toast-close",
      "aria-label": "Dismiss notification",
    });
    close.appendChild(icon("close", 12));
    close.addEventListener("click", () => this.dismiss(toast));
    const bar = el("span", { class: "toast-bar", "aria-hidden": "true" });
    toast.append(text, close, bar);
    const ms = timeout ?? (kind === "error" ? ERROR_TIMEOUT : DEFAULT_TIMEOUT);
    toast.style.setProperty("--toast-ms", `${ms}ms`);
    bar.style.animationDuration = `${ms}ms`;
    this.host.appendChild(toast);
    this.host.hidden = false;
    const timer = window.setTimeout(() => this.dismiss(toast), ms);
    this.timers.set(toast, timer);
    requestAnimationFrame(() => toast.classList.add("is-visible"));
  }

  clear(): void {
    for (const child of Array.from(this.host.children)) {
      if (child instanceof HTMLElement) this.dismiss(child);
    }
  }

  private dismiss(toast: HTMLElement): void {
    const timer = this.timers.get(toast);
    if (timer !== undefined) window.clearTimeout(timer);
    this.timers.delete(toast);
    toast.classList.remove("is-visible");
    toast.classList.add("is-leaving");
    const remove = (): void => {
      toast.remove();
      if (this.host.children.length === 0) this.host.hidden = true;
    };
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) remove();
    else window.setTimeout(remove, 180);
  }
}
