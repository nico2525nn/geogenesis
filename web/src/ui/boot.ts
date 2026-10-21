// Boot / error overlay for the stage.
//
// The overlay is present in the markup so the first paint shows a real loading
// card instead of a blank canvas; the controller only swaps text/classes.

import { el } from "./dom";

export class BootOverlay {
  private readonly host: HTMLElement;
  private readonly card: HTMLElement;
  private readonly title: HTMLElement;
  private readonly detail: HTMLElement;
  private readonly bar: HTMLElement;
  private readonly fill: HTMLElement;
  private readonly retry: HTMLButtonElement;
  private onRetry: (() => void) | null = null;

  constructor(host: HTMLElement, retryButton: HTMLButtonElement) {
    this.host = host;
    this.title = host.querySelector<HTMLElement>(".boot-title") ?? el("h2", { class: "boot-title" });
    this.detail = host.querySelector<HTMLElement>(".boot-detail") ?? el("p", { class: "boot-detail" });
    this.bar = host.querySelector<HTMLElement>(".boot-bar") ?? el("div", { class: "boot-bar" });
    this.fill = host.querySelector<HTMLElement>(".boot-fill") ?? el("span", { class: "boot-fill" });
    if (!this.bar.contains(this.fill)) this.bar.appendChild(this.fill);
    this.card = host.querySelector<HTMLElement>(".boot-card") ?? el("div", { class: "boot-card" });
    this.retry = retryButton;
    this.retry.addEventListener("click", () => {
      this.onRetry?.();
    });
    this.show("Booting GeoGenesis", "loading the WebAssembly simulation engine…");
  }

  /** Shows the overlay with a title/detail pair. */
  show(title: string, detail: string, progress: number | null = null): void {
    this.host.hidden = false;
    this.host.classList.remove("is-error");
    this.title.textContent = title;
    this.detail.textContent = detail;
    this.retry.hidden = true;
    this.setProgress(progress);
  }

  /** progress 0..1, or null for an indeterminate shimmer. */
  setProgress(progress: number | null): void {
    this.host.classList.toggle("is-indeterminate", progress === null);
    if (progress !== null) {
      this.fill.style.width = `${Math.max(0, Math.min(1, progress)) * 100}%`;
    } else {
      this.fill.style.width = "";
    }
  }

  /** Shows an error state with a retry action. */
  showError(message: string, onRetry: () => void): void {
    this.host.hidden = false;
    this.host.classList.add("is-error");
    this.host.classList.remove("is-indeterminate");
    this.onRetry = onRetry;
    this.title.textContent = "Could not start the engine";
    this.detail.textContent = message;
    this.retry.hidden = false;
    this.fill.style.width = "0%";
  }

  /** Fades the overlay out of the way. */
  hide(): void {
    this.host.classList.add("is-hidden");
    window.setTimeout(() => {
      this.host.hidden = true;
      this.host.classList.remove("is-hidden");
    }, 280);
  }
}
