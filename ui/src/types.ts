/** Shared TypeScript types mirroring velo-core/src/events.rs */

export interface NimStreamChunk {
  request_id: string;
  delta: string;
  done: boolean;
}

export type TaskStatus =
  | "pending"
  | "running"
  | "reflecting"
  | "succeeded"
  | "failed"
  | "cancelled";

export interface StepRecord {
  id: string;
  kind: "thought" | "tool_call" | "tool_result" | "reflection" | "final_answer";
  content: string;
  timestamp: string;
}

export interface TaskRecord {
  id: string;
  description: string;
  status: TaskStatus;
  started_at: string;
  finished_at: string | null;
  steps: StepRecord[];
}

export type RiskLevel = "LOW" | "MEDIUM" | "HIGH" | "CRITICAL";

export interface DestructiveActionRequest {
  action_id: string;
  task_id: string;
  risk_level: RiskLevel;
  description: string;
  tool_name: string;
  tool_args: Record<string, unknown>;
}

export interface SuggestionReady {
  id: string;
  headline: string;
  body: string;
  trigger_snippet: string;
}

export interface SttTranscript {
  partial: boolean;
  text: string;
  timestamp: string;
}

export interface VoiceStateUpdate {
  recording: boolean;
  level: number;
}
