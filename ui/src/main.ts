import { listen } from "@tauri-apps/api/event";
import { CommandBar } from "./command-bar";
import { conversation } from "./conversation";
import { Dashboard } from "./dashboard";
import { Modal } from "./modal";
import { OrbController } from "./orb";
import { SuggestionBar } from "./suggestion-bar";

import type {
  DestructiveActionRequest,
  NimStreamChunk,
  SttTranscript,
  SuggestionReady,
  TaskRecord,
  VoiceStateUpdate,
} from "./types";

async function main() {
  const orbController = new OrbController();

  /* Initial shell state — window starts expanded (full desktop app) */
  document.body.dataset.shell = "app";

  const commandBar = new CommandBar(orbController);
  const dashboard = new Dashboard();
  const modal = new Modal();
  const suggestionBar = new SuggestionBar();

  /* ── Global keyboard ────────────────────────────────────────────────────── */
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      orbController.onEscape();
    }
  });

  /* ── NIM token streaming ──────────────────────────────────────────────────── */
  await listen<NimStreamChunk>("nim-stream-chunk", ({ payload }) => {
    if (payload.done) {
      conversation.endAssistant();
    } else {
      conversation.appendToken(payload.delta);
    }
  });

  /* ── Task status updates ──────────────────────────────────────────────────── */
  await listen<TaskRecord>("task-status-update", ({ payload }) => {
    dashboard.upsertTask(payload);
  });

  /* ── Destructive action intercept ─────────────────────────────────────────── */
  await listen<DestructiveActionRequest>("destructive-action-intercept", ({ payload }) => {
    modal.show(payload);
  });

  /* ── Clipboard suggestion ─────────────────────────────────────────────────── */
  await listen<SuggestionReady>("suggestion-ready", ({ payload }) => {
    suggestionBar.show(payload);
  });

  /* ── STT transcript ───────────────────────────────────────────────────────── */
  await listen<SttTranscript>("stt-transcript", ({ payload }) => {
    commandBar.setTranscript(payload.text);
  });

  /* ── Voice state ──────────────────────────────────────────────────────────── */
  await listen<VoiceStateUpdate>("voice-state-update", ({ payload }) => {
    commandBar.setVoiceState(payload.recording, payload.level);
  });

  console.log("[Velo] UI ready — app mode");
}

main().catch(console.error);
