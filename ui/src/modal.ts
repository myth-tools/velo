/**
 * Modal — destructive action confirmation dialog.
 * Shows when the Rust backend intercepts a high-risk tool call.
 */

import { invoke } from "@tauri-apps/api/core";
import type { DestructiveActionRequest } from "./types";

export class Modal {
  private overlay = document.getElementById("modal-overlay")! as HTMLDivElement;
  private description = document.getElementById("modal-description")! as HTMLParagraphElement;
  private details = document.getElementById("modal-action-details")! as HTMLDivElement;
  private confirmBtn = document.getElementById("modal-confirm-btn")! as HTMLButtonElement;
  private cancelBtn = document.getElementById("modal-cancel-btn")! as HTMLButtonElement;

  private currentActionId: string | null = null;

  constructor() {
    this.confirmBtn.addEventListener("click", () => this.confirm());
    this.cancelBtn.addEventListener("click", () => this.cancel());

    // Close on backdrop click
    this.overlay.addEventListener("click", (e) => {
      if (e.target === this.overlay) this.cancel();
    });

    // Close on Escape
    document.addEventListener("keydown", (e) => {
      if (e.key === "Escape" && !this.overlay.hidden) this.cancel();
    });
  }

  show(request: DestructiveActionRequest) {
    this.currentActionId = request.action_id;

    this.description.textContent = request.description;
    this.details.textContent = formatArgs(request.tool_name, request.tool_args);

    // Colour the border by risk level
    const riskColour = riskColor(request.risk_level);
    const card = document.getElementById("modal-card")!;
    card.style.borderColor = riskColour.border;

    const icon = document.getElementById("modal-icon")!;
    icon.textContent = riskIcon(request.risk_level);
    icon.style.filter = `drop-shadow(0 0 12px ${riskColour.shadow})`;

    this.overlay.hidden = false;
    this.confirmBtn.focus();
  }

  private async confirm() {
    if (!this.currentActionId) return;

    this.confirmBtn.textContent = "Executing…";
    this.confirmBtn.disabled = true;

    try {
      await invoke("approve_destructive_action", { actionId: this.currentActionId });
    } catch (err) {
      console.error("approve_destructive_action error:", err);
    } finally {
      this.hide();
    }
  }

  private async cancel() {
    if (!this.currentActionId) return;

    try {
      await invoke("reject_destructive_action", { actionId: this.currentActionId });
    } catch (err) {
      console.error("reject_destructive_action error:", err);
    } finally {
      this.hide();
    }
  }

  private hide() {
    this.overlay.hidden = true;
    this.currentActionId = null;
    this.confirmBtn.textContent = "Confirm";
    this.confirmBtn.disabled = false;
  }
}

// ── Helpers ────────────────────────────────────────────────────────────────────

function formatArgs(toolName: string, args: Record<string, unknown>): string {
  try {
    const key = Object.keys(args)[0];
    if (key && typeof args[key] === "string") {
      return `${toolName}(${key}="${args[key]}")`;
    }
    return `${toolName}(${JSON.stringify(args, null, 2)})`;
  } catch {
    return toolName;
  }
}

function riskColor(level: string) {
  switch (level) {
    case "CRITICAL":
      return { border: "rgba(239,68,68,0.5)", shadow: "rgba(239,68,68,0.55)" };
    case "HIGH":
      return { border: "rgba(251,146,60,0.45)", shadow: "rgba(251,146,60,0.45)" };
    case "MEDIUM":
      return { border: "rgba(251,191,36,0.35)", shadow: "rgba(251,191,36,0.35)" };
    default:
      return { border: "rgba(100,220,255,0.25)", shadow: "rgba(100,220,255,0.25)" };
  }
}

function riskIcon(level: string): string {
  switch (level) {
    case "CRITICAL":
      return "🚨";
    case "HIGH":
      return "⚠️";
    case "MEDIUM":
      return "⚡";
    default:
      return "❓";
  }
}
