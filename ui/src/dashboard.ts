/**
 * Dashboard — renders the task timeline with live status LEDs,
 * step details on hover, and the undo button.
 */

import { invoke } from "@tauri-apps/api/core";
import type { StepRecord, TaskRecord } from "./types";

export class Dashboard {
  private taskList = document.getElementById("task-list")! as HTMLDivElement;
  private undoBtn = document.getElementById("undo-btn")! as HTMLButtonElement;
  private tasks = new Map<string, HTMLElement>();

  constructor() {
    this.undoBtn.addEventListener("click", () => this.undo());
  }

  // ── Task upsert ────────────────────────────────────────────────────────────

  upsertTask(record: TaskRecord) {
    const existing = this.tasks.get(record.id);

    if (existing) {
      this.updateCard(existing, record);
    } else {
      const card = this.createCard(record);
      this.taskList.prepend(card);
      this.tasks.set(record.id, card);

      // Cap to 30 visible tasks
      while (this.taskList.children.length > 30) {
        const last = this.taskList.lastElementChild;
        if (last) this.taskList.removeChild(last);
      }
    }
  }

  // ── Card creation ──────────────────────────────────────────────────────────

  private createCard(record: TaskRecord): HTMLElement {
    const card = document.createElement("div");
    card.className = "task-card";
    card.setAttribute("role", "listitem");
    card.dataset.taskId = record.id;
    this.updateCard(card, record);
    return card;
  }

  private updateCard(card: HTMLElement, record: TaskRecord) {
    const statusClass = `task-card__led--${record.status}`;
    const time = formatTime(record.started_at);

    card.innerHTML = `
      <div class="task-card__header">
        <span class="task-card__led ${statusClass}" title="${record.status}"></span>
        <span class="task-card__desc" title="${escHtml(record.description)}">${escHtml(record.description)}</span>
        <span class="task-card__time">${time}</span>
      </div>
      <div class="task-card__steps">
        ${record.steps.slice(-8).map(renderStep).join("")}
      </div>
    `;
  }

  // ── Undo ───────────────────────────────────────────────────────────────────

  private async undo() {
    this.undoBtn.textContent = "↩ Undoing…";
    this.undoBtn.disabled = true;

    try {
      const result = await invoke<string>("undo_last_snapshot");
      showToast(`✓ ${result}`, "success");
    } catch (err) {
      showToast(`✗ Undo failed: ${err}`, "error");
    } finally {
      this.undoBtn.textContent = "↩ Undo";
      this.undoBtn.disabled = false;
    }
  }
}

// ── Helpers ────────────────────────────────────────────────────────────────────

function renderStep(step: StepRecord): string {
  const kindLabel: Record<string, string> = {
    thought: "💭 Think",
    tool_call: "🔧 Call",
    tool_result: "✓ Result",
    reflection: "🔄 Reflect",
    final_answer: "✅ Done",
  };
  const label = kindLabel[step.kind] ?? step.kind;
  const cls = `task-step__kind--${step.kind}`;
  const content = escHtml(step.content.slice(0, 120));

  return `<div class="task-step">
    <span class="task-step__kind ${cls}">${label}</span>
    <span class="task-step__content" title="${escHtml(step.content)}">${content}</span>
  </div>`;
}

function formatTime(iso: string): string {
  try {
    const d = new Date(iso);
    return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });
  } catch {
    return "";
  }
}

function escHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function showToast(msg: string, kind: "success" | "error") {
  const toast = document.createElement("div");
  toast.style.cssText = `
    position: fixed; bottom: 12px; left: 50%; transform: translateX(-50%);
    background: ${kind === "success" ? "rgba(34,197,94,0.18)" : "rgba(239,68,68,0.18)"};
    border: 1px solid ${kind === "success" ? "rgba(34,197,94,0.45)" : "rgba(239,68,68,0.45)"};
    color: ${kind === "success" ? "hsl(145,72%,52%)" : "hsl(0,85%,62%)"};
    padding: 8px 18px; border-radius: 999px; font-size: 13px; font-weight: 500;
    z-index: 9999; animation: slide-in-up 200ms ease forwards;
    backdrop-filter: blur(12px);
  `;
  toast.textContent = msg;
  document.body.appendChild(toast);
  setTimeout(() => toast.remove(), 3500);
}
