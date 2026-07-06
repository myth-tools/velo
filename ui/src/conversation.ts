/**
 * Conversation — persistent chat transcript between user and Velo.
 *
 * Renders streamed assistant tokens into a markdown-sanitized bubble,
 * keeping the full history visible (no more vanishing responses).
 *
 * Responsibilities:
 *  - Append user / assistant message bubbles to the transcript
 *  - Stream tokens into the active assistant bubble with a live cursor
 *  - Render markdown (`marked`) and sanitize HTML (`dompurify`) safely
 *  - Auto-scroll to the newest content (respects manual scroll-up)
 *  - Coordinate window height so the panel grows with the conversation
 */

import { invoke } from "@tauri-apps/api/core";
import DOMPurify from "dompurify";
import type { Config as PurifyConfig } from "dompurify";
import { marked } from "marked";

// ── Constants ────────────────────────────────────────────────────────────────

/** Command bar height in px — matches CSS #command-bar { height: 72px }. */
const COMMAND_BAR_H = 72;
/** Dashboard height in px when expanded — matches CommandBar.EXPANDED_H. */
const DASHBOARD_H = 480;
/** Min/max transcript panel height in px. */
const TRANSCRIPT_MIN_H = 0;
const TRANSCRIPT_MAX_H = 420;
/** Top+bottom vertical padding of #transcript-scroll (2 × var(--sp-2)). */
const TRANSCRIPT_PAD = 16;
/** Animation duration for window resize. */
const ANIM_MS = 280;
/** Minimum gap (ms) between streamed re-renders of the active bubble. */
const RENDER_INTERVAL_MS = 32;
/** If user is within this many px of the bottom, auto-scroll on new tokens. */
const SCROLL_SNAP_THRESHOLD = 120;
/** Max message bubbles before the oldest are pruned. */
const MAX_MESSAGES = 200;
/** Prune down to this many when the limit is hit. */
const PRUNE_TO = 150;

// ── marked config ────────────────────────────────────────────────────────────

marked.setOptions({ gfm: true, breaks: true });

/**
 * Hardened DOMPurify config: allow a curated set of tags for rich markdown
 * but strip anything that can execute code or exfiltrate data.
 */
const PURIFY_CONFIG: PurifyConfig = {
  USE_PROFILES: { html: true },
  ALLOWED_TAGS: [
    "p",
    "br",
    "strong",
    "em",
    "del",
    "code",
    "pre",
    "ul",
    "ol",
    "li",
    "blockquote",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "a",
    "hr",
    "span",
    "table",
    "thead",
    "tbody",
    "tr",
    "th",
    "td",
  ],
  ALLOWED_ATTR: ["href", "title", "target", "rel"],
  FORBID_ATTR: ["style", "class"],
  FORBID_TAGS: ["script", "iframe", "object", "embed", "form", "input", "style"],
};

DOMPurify.addHook("afterSanitizeAttributes", (node: Element) => {
  if (node.tagName === "A") {
    node.setAttribute("target", "_blank");
    node.setAttribute("rel", "noopener noreferrer nofollow");
  }
});

// ── Helpers ──────────────────────────────────────────────────────────────────

function renderMarkdown(src: string): string {
  const raw = marked.parse(src, { async: false }) as string;
  const clean = DOMPurify.sanitize(raw, PURIFY_CONFIG);
  return clean as unknown as string;
}

function easeOutCubic(t: number): number {
  return 1 - (1 - t) ** 3;
}

/** Force a CSS animation to replay on an element (modern browser API). */
function replayAnimation(el: Element): void {
  const anims = el.getAnimations();
  for (const a of anims) {
    a.cancel();
    a.play();
  }
}

// ── Conversation class ───────────────────────────────────────────────────────

class Conversation {
  private container = document.getElementById("conversation") as HTMLDivElement;
  private transcript = document.getElementById("transcript") as HTMLDivElement;
  private scrollArea = document.getElementById("transcript-scroll") as HTMLDivElement;

  private activeMsg: HTMLDivElement | null = null;
  private activeText = "";
  private renderScheduled = false;
  private lastRender = 0;
  private resizeAnim = 0;
  private userScrolledUp = false;
  private visible = false;
  private dashOpen = false;

  constructor() {
    this.bindScrollWatcher();
    this.bindDashboardWatcher();
  }

  // ── Public API ──────────────────────────────────────────────────────────────

  pushUser(text: string): void {
    this.ensureVisible();
    this.pruneExcess();

    const msg = document.createElement("div");
    msg.className = "msg msg--user";
    msg.setAttribute("role", "article");

    const body = document.createElement("div");
    body.className = "msg__body";
    body.textContent = text;
    msg.appendChild(body);

    this.transcript.appendChild(msg);
    this.snapToBottom();
    this.scheduleResize();
  }

  beginAssistant(): void {
    this.ensureVisible();
    this.pruneExcess();

    this.activeText = "";
    this.cancelPendingRender();

    const msg = this.createAssistantNode();
    // Show the streaming caret immediately so the user sees activity
    // even during network latency.
    const body = msg.querySelector(".msg__body")!;
    body.innerHTML = '<span class="stream-caret"></span>';
    this.transcript.appendChild(msg);
    this.activeMsg = msg;

    this.snapToBottom();
    this.scheduleResize();
  }

  appendToken(delta: string): void {
    if (!this.activeMsg) this.beginAssistant();
    this.activeText += delta;
    this.scheduleRender();
  }

  endAssistant(): void {
    this.cancelPendingRender();
    if (this.activeMsg) {
      this.renderActiveNow();
      this.activeMsg.classList.remove("msg--streaming");
      this.activeMsg = null;
    }
    this.activeText = "";
    this.snapToBottom();
    this.scheduleResize();
  }

  showError(message: string): void {
    this.cancelPendingRender();
    if (this.activeMsg) {
      this.activeMsg.classList.remove("msg--streaming");
      this.activeMsg.remove();
      this.activeMsg = null;
    }
    this.activeText = "";

    const msg = document.createElement("div");
    msg.className = "msg msg--error";
    msg.setAttribute("role", "alert");
    const body = document.createElement("div");
    body.className = "msg__body";
    body.textContent = `⚠ ${message}`;
    msg.appendChild(body);
    this.transcript.appendChild(msg);
    this.snapToBottom();
    this.scheduleResize();
  }

  clear(): void {
    this.transcript.innerHTML = "";
    this.activeMsg = null;
    this.activeText = "";
    this.visible = false;
    this.container.hidden = true;
    this.scheduleResize();
  }

  /**
   * Called by CommandBar when the dashboard toggles.
   * Toggles the local dashOpen flag and triggers a window-height recalculation.
   * This avoids the race condition with the async MutationObserver.
   */
  onDashboardToggle(): void {
    this.dashOpen = !this.dashOpen;
    this.scheduleResize();
  }

  // ── Internal: prune ────────────────────────────────────────────────────────

  private pruneExcess(): void {
    if (this.transcript.children.length <= MAX_MESSAGES) return;
    while (this.transcript.children.length > PRUNE_TO) {
      this.transcript.firstElementChild?.remove();
    }
  }

  // ── Internal: node construction ─────────────────────────────────────────────

  private createAssistantNode(): HTMLDivElement {
    const msg = document.createElement("div");
    msg.className = "msg msg--assistant msg--streaming";
    msg.setAttribute("role", "article");

    const avatar = document.createElement("span");
    avatar.className = "msg__avatar";
    avatar.textContent = "⚡";
    msg.appendChild(avatar);

    const body = document.createElement("div");
    body.className = "msg__body";
    msg.appendChild(body);

    return msg;
  }

  // ── Internal: visibility & window resize ───────────────────────────────────

  private ensureVisible(): void {
    if (this.visible) return;
    this.visible = true;
    this.container.hidden = false;
    replayAnimation(this.container);
  }

  private desiredTranscriptHeight(): number {
    if (!this.visible) return TRANSCRIPT_MIN_H;
    // scrollHeight includes the vertical padding (2 × 8px = 16px).
    // Subtract TRANSCRIPT_PAD (the actual vertical padding) once.
    const contentH = this.scrollArea.scrollHeight - TRANSCRIPT_PAD;
    return Math.max(TRANSCRIPT_MIN_H, Math.min(contentH, TRANSCRIPT_MAX_H));
  }

  /** Publicly callable — scheduled window-height recalculation (debounced). */
  scheduleResize(): void {
    const transcriptH = this.desiredTranscriptHeight();
    const dashH = this.dashOpen ? DASHBOARD_H : 0;
    const targetWindowH = COMMAND_BAR_H + transcriptH + dashH;

    cancelAnimationFrame(this.resizeAnim);

    const startH = window.outerHeight;
    if (startH === targetWindowH) {
      this.applyTranscriptHeight(transcriptH);
      return;
    }

    const startTime = performance.now();
    const tick = (now: number): void => {
      const progress = Math.min((now - startTime) / ANIM_MS, 1);
      const eased = easeOutCubic(progress);
      const current = Math.round(startH + (targetWindowH - startH) * eased);

      invoke("morph_window", {
        width: window.outerWidth,
        height: current,
        x: window.screenX,
        y: window.screenY,
      }).catch(() => {});

      if (progress < 1) {
        this.resizeAnim = requestAnimationFrame(tick);
      } else {
        invoke("morph_window", {
          width: window.outerWidth,
          height: targetWindowH,
          x: window.screenX,
          y: window.screenY,
        }).catch(() => {});
        this.applyTranscriptHeight(transcriptH);
      }
    };
    this.resizeAnim = requestAnimationFrame(tick);
  }

  private applyTranscriptHeight(h: number): void {
    this.scrollArea.style.height = `${h}px`;
  }

  // ── Internal: streaming render ─────────────────────────────────────────────

  private scheduleRender(): void {
    if (this.renderScheduled) return;
    const elapsed = performance.now() - this.lastRender;
    const delay = Math.max(0, RENDER_INTERVAL_MS - elapsed);
    window.setTimeout(() => {
      this.renderScheduled = false;
      this.renderActiveNow();
    }, delay);
    this.renderScheduled = true;
  }

  private cancelPendingRender(): void {
    this.renderScheduled = false;
  }

  private renderActiveNow(): void {
    if (!this.activeMsg) return;
    this.lastRender = performance.now();
    const body = this.activeMsg.querySelector<HTMLElement>(".msg__body");
    if (!body) return;
    const cursor = '<span class="stream-caret"></span>';
    body.innerHTML = renderMarkdown(this.activeText) + cursor;
    this.maybeSnapToBottom();
  }

  // ── Internal: scroll coordination ──────────────────────────────────────────

  private bindScrollWatcher(): void {
    this.scrollArea.addEventListener(
      "scroll",
      () => {
        const { scrollTop, scrollHeight, clientHeight } = this.scrollArea;
        const distFromBottom = scrollHeight - scrollTop - clientHeight;
        this.userScrolledUp = distFromBottom > SCROLL_SNAP_THRESHOLD;
      },
      { passive: true },
    );
  }

  private snapToBottom(): void {
    this.scrollArea.scrollTo({ top: this.scrollArea.scrollHeight, behavior: "auto" });
    this.userScrolledUp = false;
  }

  private maybeSnapToBottom(): void {
    if (this.userScrolledUp) return;
    this.scrollArea.scrollTop = this.scrollArea.scrollHeight;
  }

  // ── Internal: dashboard state tracking ─────────────────────────────────────

  private bindDashboardWatcher(): void {
    const dash = document.getElementById("dashboard");
    if (!dash) return;
    const observer = new MutationObserver(() => {
      this.dashOpen = !dash.hidden;
      // Replay the entrance animation on every open.
      if (!dash.hidden) replayAnimation(dash);
    });
    observer.observe(dash, { attributes: true, attributeFilter: ["hidden"] });
    this.dashOpen = !dash.hidden;
  }
}

export const conversation = new Conversation();
