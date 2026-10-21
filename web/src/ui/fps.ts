// Rolling requestAnimationFrame frame-rate meter for the HUD.
//
// Measures the real compositor frame cadence over a ~1 s sliding window and
// refreshes the readout four times per second. The loop is suspended while the
// tab is hidden so a backgrounded simulation does not report bogus numbers.
// The label reserves a fixed width in CSS so updates never shift the HUD.

const WINDOW_MS = 1000;
const UPDATE_MS = 250;

export class FpsMeter {
  private readonly element: HTMLElement;
  private frames: number[] = [];
  private rafId = 0;
  private active = false;
  private windowStart = 0;
  private lastUpdate = 0;
  private lastFrameTime = 0;
  private readonly onVisibilityChange = (): void => {
    if (document.hidden) {
      this.stop();
      this.element.textContent = "— fps";
      this.element.classList.remove("is-good", "is-warn", "is-bad");
      this.element.classList.add("is-idle");
      this.element.setAttribute("aria-label", "Render frame rate paused (tab hidden)");
    } else {
      this.start();
    }
  };

  constructor(element: HTMLElement) {
    this.element = element;
    this.element.textContent = "— fps";
    this.element.classList.add("is-idle");
    this.element.setAttribute("aria-label", "Render frame rate");
    document.addEventListener("visibilitychange", this.onVisibilityChange);
    if (!document.hidden) this.start();
  }

  /** Stops the measurement loop (page teardown). */
  dispose(): void {
    document.removeEventListener("visibilitychange", this.onVisibilityChange);
    this.stop();
  }

  private start(): void {
    if (this.active) return;
    this.active = true;
    this.frames = [];
    this.windowStart = 0;
    this.lastUpdate = 0;
    this.lastFrameTime = 0;
    this.rafId = requestAnimationFrame(this.tick);
  }

  private stop(): void {
    this.active = false;
    if (this.rafId !== 0) cancelAnimationFrame(this.rafId);
    this.rafId = 0;
    this.frames = [];
  }

  private readonly tick = (now: number): void => {
    if (!this.active) return;
    if (this.windowStart === 0) this.windowStart = now;
    this.frames.push(now);
    const cutoff = now - WINDOW_MS;
    while (this.frames.length > 0 && (this.frames[0] ?? Infinity) <= cutoff) {
      this.frames.shift();
    }
    // Normally several frames fit in the rolling window. Extremely slow frames
    // (software GL, throttled tabs) leave fewer than two samples, in which case
    // fall back to the last inter-frame interval so the HUD never sticks on
    // "— fps" while the page is actually rendering.
    const enoughFrames = this.frames.length >= 2;
    const stalledLongEnough = now - this.windowStart >= WINDOW_MS * 1.5;
    if (now - this.lastUpdate >= UPDATE_MS && (enoughFrames || stalledLongEnough)) {
      let fps: number;
      if (enoughFrames) {
        const first = this.frames[0] ?? now;
        const span = now - first;
        fps = span > 0 ? ((this.frames.length - 1) * 1000) / span : 0;
      } else {
        const interval = this.lastFrameTime > 0 ? now - this.lastFrameTime : 0;
        fps = interval > 0 ? 1000 / interval : 0;
      }
      this.render(fps);
      this.lastUpdate = now;
    }
    this.lastFrameTime = now;
    this.rafId = requestAnimationFrame(this.tick);
  };

  private render(fps: number): void {
    const rounded = Math.max(0, Math.min(999, Math.round(fps)));
    this.element.textContent = `${rounded} fps`;
    this.element.classList.remove("is-idle", "is-good", "is-warn", "is-bad");
    if (rounded >= 55) this.element.classList.add("is-good");
    else if (rounded >= 30) this.element.classList.add("is-warn");
    else this.element.classList.add("is-bad");
    this.element.setAttribute("aria-label", `Render frame rate ${rounded} frames per second`);
    this.element.title = `Render frame rate: ${rounded} fps (rolling ${WINDOW_MS / 1000} s window)`;
  }
}
