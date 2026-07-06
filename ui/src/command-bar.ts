/**
 * CommandBar — handles user input, voice toggle, dashboard expand/collapse,
 * and window controls. Token streaming now goes to the Conversation module.
 */

import { invoke } from "@tauri-apps/api/core";
import { conversation } from "./conversation";
import type { OrbController } from "./orb";

export class CommandBar {
  private input = document.getElementById("cmd-input") as HTMLInputElement;
  private voiceBtn = document.getElementById("voice-btn") as HTMLButtonElement;
  private dashBtn = document.getElementById("dashboard-btn") as HTMLButtonElement;
  private minBtn = document.getElementById("minimize-btn") as HTMLButtonElement;
  private closeBtn = document.getElementById("close-btn") as HTMLButtonElement;
  private waveCanvas = document.getElementById("voice-waveform") as HTMLCanvasElement;
  private waveCtx = this.waveCanvas.getContext("2d")!;

  private recording = false;
  private animFrame = 0;
  private orb: OrbController;

  constructor(orb: OrbController) {
    this.orb = orb;
    this.bindInputEvents();
    this.bindVoiceButton();
    this.bindDashboardButton();
    this.bindWindowControls();
  }

  // ── Input ──────────────────────────────────────────────────────────────────

  private bindInputEvents() {
    this.input.addEventListener("keydown", async (e) => {
      if (e.key !== "Enter") return;
      const text = this.input.value.trim();
      if (!text) return;

      this.input.value = "";
      conversation.pushUser(text);
      conversation.beginAssistant();

      try {
        await invoke<string>("submit_text_command", { text });
      } catch (err) {
        console.error("submit_text_command error:", err);
        conversation.showError(`${err}`);
      }
    });
  }

  // ── STT transcript injection ───────────────────────────────────────────────

  setTranscript(text: string) {
    this.input.value = text;
    this.input.dispatchEvent(new Event("input"));
  }

  // ── Voice ──────────────────────────────────────────────────────────────────

  private bindVoiceButton() {
    this.voiceBtn.addEventListener("click", async () => {
      if (this.recording) {
        await this.stopVoice();
      } else {
        await this.startVoice();
      }
    });
  }

  private async startVoice() {
    try {
      await invoke("start_voice_input");
      this.recording = true;
      this.voiceBtn.classList.add("recording");
    } catch (err) {
      console.error("Voice start failed:", err);
    }
  }

  private async stopVoice() {
    try {
      await invoke("stop_voice_input");
      this.recording = false;
      this.voiceBtn.classList.remove("recording");
      cancelAnimationFrame(this.animFrame);
    } catch (err) {
      console.error("Voice stop failed:", err);
    }
  }

  setVoiceState(recording: boolean, level: number) {
    if (recording) {
      this.drawWaveform(level);
    } else {
      cancelAnimationFrame(this.animFrame);
    }
  }

  private drawWaveform(level: number) {
    const ctx = this.waveCtx;
    const w = this.waveCanvas.width;
    const h = this.waveCanvas.height;
    ctx.clearRect(0, 0, w, h);

    const barCount = 5;
    const barW = 2;
    const gap = (w - barCount * barW) / (barCount + 1);

    for (let i = 0; i < barCount; i++) {
      const heightFactor = 0.3 + Math.random() * level * 0.7;
      const barH = h * heightFactor;
      const x = gap + i * (barW + gap);
      const y = (h - barH) / 2;

      ctx.fillStyle = "hsl(0, 80%, 62%)";
      ctx.beginPath();
      ctx.roundRect(x, y, barW, barH, 1);
      ctx.fill();
    }

    this.animFrame = requestAnimationFrame(() => this.drawWaveform(level));
  }

  // ── Dashboard ──────────────────────────────────────────────────────────────

  private bindDashboardButton() {
    this.dashBtn.addEventListener("click", () => {
      const dashboard = document.getElementById("dashboard")!;
      const opening = dashboard.hidden;

      if (opening) {
        dashboard.hidden = false;
        this.dashBtn.classList.add("active");
        conversation.onDashboardToggle();
      } else {
        this.dashBtn.classList.remove("active");
        // Let the shrink animation play, then hide dashboard and resize.
        setTimeout(() => {
          dashboard.hidden = true;
          conversation.onDashboardToggle();
        }, 280);
      }
    });
  }

  // ── Window controls ────────────────────────────────────────────────────────

  private bindWindowControls() {
    this.minBtn.addEventListener("click", () => {
      this.orb.onMinimize();
    });

    this.closeBtn.addEventListener("click", async () => {
      try {
        await invoke("close_window");
      } catch (err) {
        console.error("close_window error:", err);
      }
    });
  }
}
