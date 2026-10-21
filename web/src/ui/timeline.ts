// Timeline: the authoritative simulation-time control.
//
// Provides play/pause, stepping, a run-to-end action, speed selection,
// arbitrary-time jumps ("500k", "2.5Myr"), captured-frame markers, a hover
// readout, drag scrubbing with cheap preview frames and full keyboard/ARIA
// slider semantics.

import type { EngineState } from "../types";
import { el, icon } from "./dom";
import { formatDuration, formatDurationTick, formatSigned, niceStep, parseDuration } from "./format";

export interface TimelineCallbacks {
  onStep(): void;
  onStepBack(): void;
  onRun(): void;
  onRewind(): void;
  onPlayToggle(playing: boolean): void;
  onSeek(year: number): void;
  /** Cheap preview during a drag; must not start an exact seek. */
  onScrubPreview(year: number): void;
  onSpeedChange(stepsPerTick: number): void;
}

const SPEEDS: Array<{ label: string; steps: number }> = [
  { label: "0.5×", steps: 1 },
  { label: "1×", steps: 2 },
  { label: "2×", steps: 4 },
  { label: "4×", steps: 8 },
];

const KEYBOARD_SEEK_DELAY = 220;

export class Timeline {
  private readonly callbacks: TimelineCallbacks;
  private readonly host: HTMLElement;
  private readonly playButton: HTMLButtonElement;
  private readonly stepBackButton: HTMLButtonElement;
  private readonly stepButton: HTMLButtonElement;
  private readonly runButton: HTMLButtonElement;
  private readonly rewindButton: HTMLButtonElement;
  private readonly track: HTMLElement;
  private readonly rail: HTMLElement;
  private readonly progress: HTMLElement;
  private readonly ticks: HTMLElement;
  private readonly frameTicks: HTMLElement;
  private readonly tickLabels: HTMLElement;
  private readonly handle: HTMLElement;
  private readonly hoverLine: HTMLElement;
  private readonly hoverPill: HTMLElement;
  private readonly timeLabel: HTMLElement;
  private readonly durationLabel: HTMLElement;
  private readonly chips: HTMLElement;
  private readonly activity: HTMLElement;
  private readonly speedSelect: HTMLSelectElement;
  private readonly jumpInput: HTMLInputElement;
  private state: EngineState | null = null;
  private playing = false;
  private busy = false;
  private scrubbing = false;
  private previewYear: number | null = null;
  private renderWidth = 0;
  private tickKey = "";
  private keyboardTimer = 0;

  constructor(host: HTMLElement, callbacks: TimelineCallbacks) {
    this.callbacks = callbacks;
    this.host = host;
    host.replaceChildren();
    host.classList.add("timeline-ready");

    const inner = el("div", { class: "timeline-inner" });

    // --- transport ----------------------------------------------------------
    const transport = el("div", { class: "timeline-transport", role: "group", "aria-label": "Time transport" });
    this.rewindButton = this.transportButton("rewind", "Jump to the start (Home)", () => this.callbacks.onRewind());
    this.stepBackButton = this.transportButton("step-back", "Step back to the previous captured frame (←)", () =>
      this.callbacks.onStepBack(),
    );
    this.playButton = el("button", {
      type: "button",
      class: "transport-play",
      "aria-label": "Play simulation (Space)",
      "aria-pressed": "false",
      "data-tip": "Play / pause (Space)",
    });
    this.playButton.appendChild(icon("play", 18));
    this.playButton.addEventListener("click", () => this.callbacks.onPlayToggle(!this.playing));
    this.stepButton = this.transportButton("step-forward", "Step forward (→)", () => this.callbacks.onStep());
    this.runButton = el("button", {
      type: "button",
      class: "timeline-run",
      "data-tip": "Run to the end of the configured duration",
    });
    this.runButton.append(icon("run", 14), el("span", { text: "Run" }));
    this.runButton.addEventListener("click", () => this.callbacks.onRun());
    transport.append(
      this.rewindButton,
      this.stepBackButton,
      this.playButton,
      this.stepButton,
      this.runButton,
    );

    // --- centre: headline + ruler -------------------------------------------
    const centre = el("div", { class: "timeline-centre" });
    const headline = el("div", { class: "timeline-headline" });
    this.timeLabel = el("span", { class: "timeline-time", text: "0 yr" });
    this.durationLabel = el("span", { class: "timeline-duration" });
    this.chips = el("span", { class: "timeline-chips" });
    this.activity = el("span", { class: "timeline-activity", hidden: true });
    headline.append(this.timeLabel, this.durationLabel, this.chips, this.activity);

    this.track = el("div", {
      class: "timeline-track",
      role: "slider",
      tabindex: "0",
      "aria-label": "Simulation time",
      "aria-valuemin": "0",
      "aria-valuenow": "0",
      "aria-valuetext": "0 yr",
      "data-tip": "Drag to scrub · ← → to step · Home / End",
    });
    this.rail = el("div", { class: "timeline-rail" });
    this.progress = el("div", { class: "timeline-progress" });
    this.ticks = el("div", { class: "timeline-ticks" });
    this.frameTicks = el("div", { class: "timeline-frame-ticks" });
    this.tickLabels = el("div", { class: "timeline-tick-labels" });
    this.handle = el("div", { class: "timeline-handle" });
    this.hoverLine = el("div", { class: "timeline-hover-line", hidden: true });
    this.hoverPill = el("div", { class: "timeline-hover-pill", hidden: true });
    this.track.append(
      this.rail,
      this.progress,
      this.frameTicks,
      this.ticks,
      this.tickLabels,
      this.handle,
      this.hoverLine,
      this.hoverPill,
    );
    centre.append(headline, this.track);

    this.track.addEventListener("pointerdown", this.onPointerDown);
    this.track.addEventListener("pointermove", this.onPointerMove);
    this.track.addEventListener("pointerup", this.onPointerUp);
    this.track.addEventListener("pointercancel", this.onPointerUp);
    this.track.addEventListener("pointerleave", this.onPointerLeave);
    this.track.addEventListener("keydown", this.onKeyDown);

    const resizeObserver = new ResizeObserver(() => {
      const width = this.track.clientWidth;
      if (Math.abs(width - this.renderWidth) > 2) {
        this.renderWidth = width;
        this.tickKey = "";
        if (this.state !== null) this.update(this.state);
      }
    });
    resizeObserver.observe(this.track);

    // --- right: speed + jump ------------------------------------------------
    const side = el("div", { class: "timeline-side" });
    const speedWrap = el("label", { class: "timeline-speed-wrap", "data-tip": "Simulation speed" });
    speedWrap.append(el("span", { class: "visually-hidden", text: "Simulation speed" }));
    this.speedSelect = el("select", { class: "timeline-speed", "aria-label": "Simulation speed" });
    for (const speed of SPEEDS) {
      this.speedSelect.appendChild(el("option", { value: String(speed.steps), text: speed.label }));
    }
    this.speedSelect.value = "2";
    this.speedSelect.addEventListener("change", () => {
      this.callbacks.onSpeedChange(Number(this.speedSelect.value) || 2);
    });
    speedWrap.appendChild(this.speedSelect);

    const jump = el("label", { class: "timeline-jump-wrap", "data-tip": "Jump to a time: 500k, 2.5Myr, 1.2e6" });
    jump.appendChild(icon("clock", 13));
    this.jumpInput = el("input", {
      type: "text",
      class: "timeline-jump",
      placeholder: "jump: 500k",
      autocomplete: "off",
      spellcheck: "false",
      "aria-label": "Jump to simulation time",
    });
    this.jumpInput.addEventListener("keydown", (event) => {
      if (event.key === "Escape") {
        event.stopPropagation();
        this.jumpInput.value = "";
        this.jumpInput.classList.remove("invalid");
        this.jumpInput.blur();
        return;
      }
      if (event.key !== "Enter") return;
      const year = parseDuration(this.jumpInput.value);
      if (year === null) {
        this.jumpInput.classList.add("invalid");
        this.jumpInput.setAttribute("aria-invalid", "true");
        return;
      }
      this.jumpInput.classList.remove("invalid");
      this.jumpInput.removeAttribute("aria-invalid");
      this.callbacks.onSeek(Math.max(0, year));
      this.jumpInput.value = "";
      this.jumpInput.blur();
    });
    jump.appendChild(this.jumpInput);
    side.append(speedWrap, jump);

    inner.append(transport, centre, side);
    host.appendChild(inner);
    this.updateLabels(0, 0);
    this.syncButtons();
  }

  /** Updates the timeline from a simulation state. */
  update(state: EngineState): void {
    this.state = state;
    if (this.previewYear === null) {
      this.setPosition(state.elapsed_yr);
    }
    this.updateLabels(state.elapsed_yr, state.total_yr);
    this.updateChips(state);
    this.renderTicks(state);
    this.syncButtons();
  }

  /** Enables/disables controls while an engine operation runs. */
  setBusy(busy: boolean): void {
    this.busy = busy;
    this.host.classList.toggle("is-busy", busy);
    this.syncButtons();
  }

  /** Shows a short activity label in the headline ("running · 42%"). */
  setActivity(text: string | null): void {
    const runLabel = this.runButton.querySelector("span");
    if (text === null) {
      this.activity.hidden = true;
      this.activity.textContent = "";
      this.host.classList.remove("is-active");
      this.runButton.classList.remove("is-running");
      if (runLabel !== null) runLabel.textContent = "Run";
    } else {
      this.activity.hidden = false;
      this.activity.textContent = text;
      this.host.classList.add("is-active");
      if (text.startsWith("running")) {
        this.runButton.classList.add("is-running");
        if (runLabel !== null) runLabel.textContent = "Running…";
      }
    }
  }

  /** Reflects the playback state. */
  setPlaying(playing: boolean): void {
    this.playing = playing;
    this.playButton.replaceChildren(icon(playing ? "pause" : "play", 18));
    this.playButton.setAttribute("aria-pressed", playing ? "true" : "false");
    this.playButton.setAttribute("aria-label", playing ? "Pause simulation (Space)" : "Play simulation (Space)");
    this.playButton.dataset.tip = playing ? "Pause (Space)" : "Play (Space)";
    this.host.classList.toggle("is-playing", playing);
    this.syncButtons();
  }

  /** Currently selected speed in steps per tick. */
  getSpeed(): number {
    return Number(this.speedSelect.value) || 2;
  }

  /** Current simulated time (including a pending preview). */
  getTime(): number {
    return this.previewYear ?? this.state?.elapsed_yr ?? 0;
  }

  /** Clears a pending preview and re-syncs the handle to the real time. */
  clearPreview(): void {
    this.previewYear = null;
    if (this.state !== null) {
      this.setPosition(this.state.elapsed_yr);
      this.updateLabels(this.state.elapsed_yr, this.state.total_yr);
    }
  }

  // --- internals ------------------------------------------------------------

  private transportButton(name: "rewind" | "step-back" | "step-forward", tip: string, action: () => void): HTMLButtonElement {
    const button = el("button", {
      type: "button",
      class: "transport-button",
      "aria-label": tip,
      "data-tip": tip,
    });
    button.appendChild(icon(name, 15));
    button.addEventListener("click", action);
    return button;
  }

  private setPosition(year: number): void {
    const total = Math.max(this.state?.total_yr ?? 0, 1);
    const fraction = Math.max(0, Math.min(year / total, 1));
    this.progress.style.width = `${(fraction * 100).toFixed(3)}%`;
    this.handle.style.left = `${(fraction * 100).toFixed(3)}%`;
    this.track.setAttribute("aria-valuenow", String(Math.round(year)));
    this.track.setAttribute("aria-valuetext", `${formatDuration(year)} of ${formatDuration(total)}`);
  }

  private updateLabels(elapsed: number, total: number): void {
    this.timeLabel.textContent = formatDuration(elapsed);
    this.durationLabel.textContent = total > 0 ? `/ ${formatDuration(total)}` : "";
  }

  private updateChips(state: EngineState): void {
    const frames = state.history.frames.length;
    const parts = [
      `sea ${formatSigned(state.sea_level_m, 0)} m`,
      `step ${formatDuration(state.step_yr)}`,
      `${frames} frame${frames === 1 ? "" : "s"}`,
    ];
    const text = parts.join(" · ");
    if (this.chips.textContent !== text) this.chips.textContent = text;
  }

  private renderTicks(state: EngineState): void {
    const width = this.track.clientWidth;
    if (width <= 0) return;
    const total = Math.max(state.total_yr, 1);
    const frames = state.history.frames;
    const frameCount = frames.length;
    const key = `${width}|${Math.round(total)}|${frameCount}|${Math.round(state.step_yr)}`;
    if (key === this.tickKey) return;
    this.tickKey = key;

    // Captured frames (subsampled to avoid overdraw).
    this.frameTicks.replaceChildren();
    const frameStride = Math.max(1, Math.ceil(frameCount / 80));
    for (let i = 0; i < frameCount; i += frameStride) {
      const frame = frames[i];
      if (frame === undefined) continue;
      const tick = el("span", {
        class: "timeline-frame-tick",
        title: `Captured frame · ${formatDuration(frame.time)}`,
      });
      tick.style.left = `${Math.min((frame.time / total) * 100, 100).toFixed(3)}%`;
      this.frameTicks.appendChild(tick);
    }

    // Major time ticks with non-overlapping labels.
    this.ticks.replaceChildren();
    this.tickLabels.replaceChildren();
    const step = niceStep(total, Math.max(3, Math.min(8, Math.floor(width / 110))));
    const count = Math.floor(total / step);
    let lastLabelRight = -Infinity;
    for (let i = 0; i <= count; i += 1) {
      const year = i * step;
      const fraction = year / total;
      const tick = el("span", { class: "timeline-tick" });
      tick.style.left = `${(fraction * 100).toFixed(3)}%`;
      this.ticks.appendChild(tick);
      const text = formatDurationTick(year);
      const estimatedWidth = text.length * 5.4;
      const centre = fraction * width;
      if (centre - estimatedWidth / 2 < lastLabelRight + 8) continue;
      if (centre + estimatedWidth / 2 > width + 1) continue;
      const labelEl = el("span", { class: "timeline-tick-label", text });
      labelEl.style.left = `${fraction * 100}%`;
      this.tickLabels.appendChild(labelEl);
      lastLabelRight = centre + estimatedWidth / 2;
    }
    // Always label the end point when it fits.
    if (total > 0) {
      const endLabel = el("span", { class: "timeline-tick-label is-end", text: formatDurationTick(total) });
      endLabel.style.left = "100%";
      this.tickLabels.appendChild(endLabel);
    }
  }

  private syncButtons(): void {
    const locked = this.busy;
    const scrubLocked = locked;
    this.playButton.disabled = locked;
    this.stepBackButton.disabled = locked || this.playing;
    this.stepButton.disabled = locked || this.playing;
    this.runButton.disabled = locked || this.playing;
    this.rewindButton.disabled = locked || this.playing;
    this.speedSelect.disabled = locked;
    this.jumpInput.disabled = locked;
    this.track.setAttribute("aria-disabled", scrubLocked ? "true" : "false");
    this.track.classList.toggle("is-locked", scrubLocked);
    this.runButton.classList.toggle("is-running", locked && this.activity.hidden === false);
  }

  private fractionFromEvent(event: PointerEvent): number {
    const rect = this.track.getBoundingClientRect();
    return Math.max(0, Math.min(1, (event.clientX - rect.left) / Math.max(rect.width, 1)));
  }

  private yearFromFraction(fraction: number): number {
    return Math.round(fraction * (this.state?.total_yr ?? 0));
  }

  private previewAt(fraction: number, notify: boolean): void {
    if (this.state === null) return;
    const year = this.yearFromFraction(fraction);
    this.previewYear = year;
    this.setPosition(year);
    this.updateLabels(year, this.state.total_yr);
    this.showHover(fraction);
    if (notify) this.callbacks.onScrubPreview(year);
  }

  private showHover(fraction: number): void {
    if (this.state === null) return;
    const rect = this.track.getBoundingClientRect();
    const x = fraction * rect.width;
    this.hoverLine.hidden = false;
    this.hoverLine.style.left = `${(fraction * 100).toFixed(3)}%`;
    this.hoverPill.hidden = false;
    this.hoverPill.textContent = formatDuration(this.yearFromFraction(fraction));
    const pillWidth = this.hoverPill.offsetWidth || 70;
    const left = Math.max(0, Math.min(x - pillWidth / 2, rect.width - pillWidth));
    this.hoverPill.style.left = `${left.toFixed(1)}px`;
  }

  private hideHover(): void {
    this.hoverLine.hidden = true;
    this.hoverPill.hidden = true;
  }

  private readonly onPointerDown = (event: PointerEvent): void => {
    if (this.state === null || this.busy) return;
    if (event.button !== 0 && event.pointerType === "mouse") return;
    this.scrubbing = true;
    this.track.setPointerCapture(event.pointerId);
    this.track.classList.add("is-scrubbing");
    this.previewAt(this.fractionFromEvent(event), true);
    event.preventDefault();
  };

  private lastPreviewAt = 0;

  private readonly onPointerMove = (event: PointerEvent): void => {
    if (this.state === null) return;
    const fraction = this.fractionFromEvent(event);
    if (!this.scrubbing) {
      this.showHover(fraction);
      return;
    }
    const now = performance.now();
    const notify = now - this.lastPreviewAt > 70;
    if (notify) this.lastPreviewAt = now;
    this.previewAt(fraction, notify);
  };

  private readonly onPointerUp = (event: PointerEvent): void => {
    if (!this.scrubbing || this.state === null) return;
    this.scrubbing = false;
    this.track.classList.remove("is-scrubbing");
    if (this.track.hasPointerCapture(event.pointerId)) {
      this.track.releasePointerCapture(event.pointerId);
    }
    const fraction = this.fractionFromEvent(event);
    const year = this.yearFromFraction(fraction);
    this.previewYear = null;
    this.callbacks.onSeek(year);
    this.hideHover();
  };

  private readonly onPointerLeave = (): void => {
    if (!this.scrubbing) this.hideHover();
  };

  private readonly onKeyDown = (event: KeyboardEvent): void => {
    if (this.state === null || this.busy) return;
    const total = this.state.total_yr;
    const current = this.previewYear ?? this.state.elapsed_yr;
    const fine = Math.max(total / 50, this.state.step_yr);
    let target: number | null = null;
    switch (event.key) {
      case "ArrowRight":
      case "ArrowUp":
        target = current + (event.shiftKey ? fine * 5 : fine);
        break;
      case "ArrowLeft":
      case "ArrowDown":
        target = current - (event.shiftKey ? fine * 5 : fine);
        break;
      case "PageUp":
        target = current + total / 10;
        break;
      case "PageDown":
        target = current - total / 10;
        break;
      case "Home":
        target = 0;
        break;
      case "End":
        target = total;
        break;
      case "Enter":
        this.callbacks.onSeek(Math.round(current));
        this.clearPreview();
        return;
      case "Escape":
        this.clearPreview();
        return;
      default:
        return;
    }
    if (target === null) return;
    event.preventDefault();
    const clamped = Math.max(0, Math.min(target, total));
    this.previewYear = clamped;
    this.setPosition(clamped);
    this.updateLabels(clamped, total);
    window.clearTimeout(this.keyboardTimer);
    this.keyboardTimer = window.setTimeout(() => {
      if (this.previewYear === null) return;
      this.callbacks.onSeek(Math.round(this.previewYear));
    }, KEYBOARD_SEEK_DELAY);
  };
}
