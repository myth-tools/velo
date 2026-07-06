/**
 * SuggestionBar — clipboard error suggestion pill.
 * Shows when the Rust backend detects an error log in the clipboard.
 */

import { invoke } from "@tauri-apps/api/core";
import type { SuggestionReady } from "./types";

export class SuggestionBar {
  private bar = document.getElementById("suggestion-bar")! as HTMLDivElement;
  private headline = document.getElementById("suggestion-headline")! as HTMLSpanElement;
  private applyBtn = document.getElementById("suggestion-apply-btn")! as HTMLButtonElement;
  private dismissBtn = document.getElementById("suggestion-dismiss-btn")! as HTMLButtonElement;

  private currentSuggestion: SuggestionReady | null = null;

  constructor() {
    this.applyBtn.addEventListener("click", () => this.apply());
    this.dismissBtn.addEventListener("click", () => this.hide());
  }

  show(suggestion: SuggestionReady) {
    this.currentSuggestion = suggestion;
    this.headline.textContent = suggestion.headline;
    this.bar.hidden = false;
  }

  private async apply() {
    if (!this.currentSuggestion) return;

    this.applyBtn.textContent = "Applying…";
    this.applyBtn.disabled = true;

    try {
      // Convert the suggestion into a task request
      await invoke("submit_text_command", {
        text: `Diagnose and fix the following error:\n\n${this.currentSuggestion.body}`,
      });
    } catch (err) {
      console.error("Failed to apply suggestion:", err);
    } finally {
      this.hide();
      this.applyBtn.textContent = "Apply Fix";
      this.applyBtn.disabled = false;
    }
  }

  private hide() {
    this.bar.hidden = true;
    this.currentSuggestion = null;
  }
}
