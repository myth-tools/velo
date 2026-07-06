import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { currentMonitor } from "@tauri-apps/api/window";

/**
 * OrbController  —  industry-grade morph engine.
 *
 * Manages the orb ↔ app-shell transition with spring-physics-inspired
 * frame-by-frame interpolation, plus CSS squish/pop animations.
 *
 * States:
 *   "orb"  → 68×68 messenger-ball at saved position, always-on-top, skip-taskbar
 *   "app"  → 720×H panel at screen-center-top, resizable, show in taskbar
 *
 * Launch: always boots in "app" state (full desktop app feel).
 */

// ── Layout constants ─────────────────────────────────────────────────────────
const ORB_W = 140;
const ORB_H = 140;
const APP_W = 720;
const APP_H_MIN = 72; // command-bar only
const APP_Y = 48;

// ── Drag threshold (px, in screen coordinates) ────────────────────────────────────
const DRAG_THRESHOLD = 6; // px in screen-space; larger = less accidental drags

// ── Morph timing ─────────────────────────────────────────────────────────────
const MORPH_TO_APP_MS = 560;
const MORPH_TO_ORB_MS = 440;

// ── Spring easing ─────────────────────────────────────────────────────────────
/** Cubic-bezier approximation of a damped spring with overshoot. */
function easeSpring(t: number): number {
  // Approximates cubic-bezier(0.34, 1.56, 0.64, 1) in JS for window morph.
  // Uses a custom blend: ease-out quint + gentle overshoot.
  const c1 = 1.70158;
  const c2 = c1 * 1.525;
  return t < 0.5
    ? ((2 * t) ** 2 * ((c2 + 1) * 2 * t - c2)) / 2
    : ((2 * t - 2) ** 2 * ((c2 + 1) * (t * 2 - 2) + c2) + 2) / 2;
}

/** Ease-out quint — used for the window-size half of morphToOrb. */
function easeOutQuint(t: number): number {
  return 1 - (1 - t) ** 5;
}

export class OrbController {
  private orb = document.getElementById("orb") as HTMLDivElement;
  private body = document.body;

  private state: "orb" | "app" = "app"; // starts in app mode
  private morphing = false;

  /* Saved orb position — default to bottom-right area */
  private orbX = 48;
  private orbY = 48;

  /* Drag state */
  private dragged = false;
  private pointerDownX = 0;
  private pointerDownY = 0;
  private pointerId = -1;
  private moved = false;

  constructor() {
    this.restorePosition();
    this.bindDrag();
    this.bindKeyboard();
    // Ensure shell shows app on first render
    this.showShell("app");
  }

  // ── Public API ────────────────────────────────────────────────────────────

  getState() {
    return this.state;
  }

  async morphToApp() {
    if (this.state === "app" || this.morphing) return;
    this.state = "app";
    this.morphing = true;

    /* 1. Brief pop animation on the orb before window expands */
    this.orb.classList.remove("squish", "pop");
    void this.orb.offsetWidth; // reflow
    this.orb.classList.add("pop");

    /* Wait for CSS pop animation to reach peak before morphing window */
    await this.delay(160);

    const pos = await this.getGeometry();
    const monitor = await this.getCurrentMonitor();
    const centerX = Math.max(0, Math.round((monitor.size.width - APP_W) / 2));

    /* 2. Switch shell (shows #app, hides #orb) */
    this.showShell("app");
    await this.setResizable(true);
    await this.setSkipTaskbar(false);

    /* 3. Animate window from orb position → app position */
    await this.animateMorph(
      pos.x,
      pos.y,
      pos.width,
      pos.height,
      centerX,
      APP_Y,
      APP_W,
      APP_H_MIN,
      MORPH_TO_APP_MS,
      easeSpring,
    );

    this.morphing = false;
    this.focusInput();

    /* Clean up pop class */
    this.orb.classList.remove("pop");
  }

  async morphToOrb() {
    if (this.state === "orb" || this.morphing) return;
    this.state = "orb";
    this.morphing = true;

    const pos = await this.getGeometry();

    /* 1. Switch shell immediately so orb CSS appears */
    this.showShell("orb");
    await this.setResizable(false);
    await this.setSkipTaskbar(true);

    /* 2. Animate window from app → orb size/position */
    await this.animateMorph(
      pos.x,
      pos.y,
      pos.width,
      pos.height,
      this.orbX,
      this.orbY,
      ORB_W,
      ORB_H,
      MORPH_TO_ORB_MS,
      easeOutQuint,
    );

    /* 3. Play squish animation after window lands */
    this.orb.classList.remove("squish");
    void this.orb.offsetWidth;
    this.orb.classList.add("squish");

    /* Clean up squish class after animation */
    setTimeout(() => this.orb.classList.remove("squish"), 520);

    this.morphing = false;
  }

  onMinimize() {
    this.morphToOrb();
  }

  onEscape() {
    if (this.state === "app") this.morphToOrb();
  }

  // ── Unified pointer drag + click ──────────────────────────────────────────

  private bindDrag() {
    this.orb.addEventListener("pointerdown", (e) => {
      if (this.state !== "orb") return;
      this.moved = false;
      this.dragged = false;

      // We only use clientX/Y to check the initial movement threshold.
      this.pointerDownX = e.clientX;
      this.pointerDownY = e.clientY;
      this.pointerId = e.pointerId;
      this.orb.setPointerCapture(e.pointerId);
      this.orb.classList.add("dragging");
    });

    this.orb.addEventListener("pointermove", async (e) => {
      if (this.state !== "orb") return;
      if (e.pointerId !== this.pointerId) return;
      if (this.moved) return; // OS is handling the drag

      const dx = e.clientX - this.pointerDownX;
      const dy = e.clientY - this.pointerDownY;

      if (Math.abs(dx) > DRAG_THRESHOLD || Math.abs(dy) > DRAG_THRESHOLD) {
        this.moved = true;
        this.dragged = true;

        // Release pointer capture so Tauri/OS can handle the native drag
        this.orb.releasePointerCapture(e.pointerId);

        try {
          // Native drag: incredibly robust on all operating systems
          await getCurrentWebviewWindow().startDragging();
        } catch (err) {
          console.error("Native drag failed:", err);
        }

        // After drag finishes, read final position from OS
        const pos = await this.getGeometry();
        this.orbX = pos.x;
        this.orbY = pos.y;
        this.savePosition();

        this.orb.classList.remove("dragging");
        this.pointerId = -1;
      }
    });

    this.orb.addEventListener("pointerup", (e) => {
      if (this.state !== "orb") return;
      if (e.pointerId === this.pointerId) {
        this.orb.releasePointerCapture(e.pointerId);
      }
      this.orb.classList.remove("dragging");

      /* Click (no drag movement) → expand */
      if (!this.dragged) {
        this.ripple();
        this.morphToApp();
      }

      this.dragged = false;
      this.moved = false;
      this.pointerId = -1;
    });

    this.orb.addEventListener("pointercancel", () => {
      if (this.state !== "orb") return;
      this.orb.classList.remove("dragging");

      this.dragged = false;
      this.moved = false;
      this.pointerId = -1;
    });
  }

  // ── Keyboard ──────────────────────────────────────────────────────────────

  private bindKeyboard() {
    this.orb.addEventListener("keydown", (e) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        this.morphToApp();
      }
    });
  }

  // ── Shell visibility ──────────────────────────────────────────────────────

  private showShell(shell: "orb" | "app") {
    this.body.dataset.shell = shell;
    if (shell === "app") {
      // Stop any ongoing orb animations
      this.orb.style.animation = "none";
      setTimeout(() => {
        this.orb.style.animation = "";
      }, 50);
    }
  }

  // ── Window morph engine ──────────────────────────────────────────────────

  private animateMorph(
    sx: number,
    sy: number,
    sw: number,
    sh: number,
    ex: number,
    ey: number,
    ew: number,
    eh: number,
    durationMs: number,
    easeFn: (t: number) => number,
  ): Promise<void> {
    return new Promise((resolve) => {
      const startTime = performance.now();

      const tick = (now: number) => {
        const elapsed = now - startTime;
        const rawT = Math.min(elapsed / durationMs, 1);
        const p = easeFn(rawT);

        const x = Math.round(sx + (ex - sx) * p);
        const y = Math.round(sy + (ey - sy) * p);
        const w = Math.round(sw + (ew - sw) * p);
        const h = Math.round(sh + (eh - sh) * p);

        invoke("morph_window", { width: w, height: h, x, y });

        if (rawT >= 1) {
          /* Snap to exact final values */
          invoke("morph_window", { width: ew, height: eh, x: ex, y: ey });
          resolve();
        } else {
          requestAnimationFrame(tick);
        }
      };

      requestAnimationFrame(tick);
    });
  }

  // ── Ripple effect ─────────────────────────────────────────────────────────

  private ripple() {
    this.orb.classList.remove("rip");
    void this.orb.offsetWidth;
    this.orb.classList.add("rip");
    setTimeout(() => this.orb.classList.remove("rip"), 750);
  }

  // ── Tauri window helpers ──────────────────────────────────────────────────

  private async getGeometry() {
    return invoke<{ x: number; y: number; width: number; height: number }>("get_window_geometry");
  }

  private async getCurrentMonitor() {
    const mon = await currentMonitor();
    return mon!;
  }

  private async setResizable(v: boolean) {
    const w = getCurrentWebviewWindow();
    await w.setResizable(v);
  }

  private async setSkipTaskbar(v: boolean) {
    await invoke("set_skip_taskbar", { skip: v });
  }

  private focusInput() {
    const input = document.getElementById("cmd-input") as HTMLInputElement | null;
    if (input) setTimeout(() => input.focus(), 360);
  }

  // ── Helpers ───────────────────────────────────────────────────────────────

  private delay(ms: number): Promise<void> {
    return new Promise((r) => setTimeout(r, ms));
  }

  // ── Position persistence ──────────────────────────────────────────────────

  private savePosition() {
    try {
      localStorage.setItem("velo_orb_x", String(this.orbX));
      localStorage.setItem("velo_orb_y", String(this.orbY));
    } catch {
      /* noop */
    }
  }

  private restorePosition() {
    try {
      const x = localStorage.getItem("velo_orb_x");
      const y = localStorage.getItem("velo_orb_y");
      if (x !== null && !Number.isNaN(+x)) this.orbX = Number(x);
      if (y !== null && !Number.isNaN(+y)) this.orbY = Number(y);
    } catch {
      /* noop */
    }
  }
}
