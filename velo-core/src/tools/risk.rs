use serde_json::Value;
use uuid::Uuid;

use crate::events::{DestructiveActionRequest, RiskLevel};

pub(crate) fn classify_risk(tool_name: &str, args: &Value) -> RiskLevel {
    match tool_name {
        "shell" => {
            let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
            let tokens: Vec<&str> = cmd.split_whitespace().collect();
            if tokens.is_empty() {
                return RiskLevel::Low;
            }

            let base_cmd = tokens[0].to_lowercase();
            let lower_cmd = cmd.to_lowercase();

            if ["mkfs", "dd", "format"].contains(&base_cmd.as_str()) {
                return RiskLevel::Critical;
            }

            if base_cmd == "rm"
                && tokens
                    .iter()
                    .any(|t| t.to_lowercase().contains("-r") || t.to_lowercase().contains("-f"))
            {
                return RiskLevel::Critical;
            }

            if lower_cmd.contains("> /dev/")
                || lower_cmd.contains(":(){:|:&};:")
                || lower_cmd.contains("chmod 777 /")
                || lower_cmd.contains("chmod -R 777")
            {
                return RiskLevel::Critical;
            }

            if ["reboot", "shutdown", "poweroff", "halt", "init 0", "init 6"]
                .contains(&base_cmd.as_str())
            {
                return RiskLevel::Critical;
            }

            if ["sudo", "systemctl", "kill", "truncate"].contains(&base_cmd.as_str()) {
                return RiskLevel::High;
            }

            if base_cmd == "chmod" || base_cmd == "chown" {
                return RiskLevel::High;
            }

            RiskLevel::Low
        }
        "delete_path" => RiskLevel::High,
        "write_file" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            if path.contains("/etc/")
                || path.contains("/usr/")
                || path.contains("/boot/")
                || path.contains("/sys/")
                || path.contains("/proc/")
            {
                RiskLevel::High
            } else {
                RiskLevel::Low
            }
        }
        "manage_process" => {
            let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");
            if action == "kill" {
                RiskLevel::High
            } else {
                RiskLevel::Low
            }
        }
        _ => RiskLevel::Low,
    }
}

fn build_description(tool_name: &str, args: &Value) -> String {
    match tool_name {
        "shell" => format!(
            "Execute shell command: `{}`",
            args.get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("(unknown)")
        ),
        "delete_path" => format!(
            "Permanently delete path: `{}`",
            args.get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("(unknown)")
        ),
        "write_file" => format!(
            "Overwrite file: `{}`",
            args.get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("(unknown)")
        ),
        "manage_process" => {
            let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");
            let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let pid = args.get("pid").and_then(|v| v.as_u64()).unwrap_or(0);
            if action == "kill" && !name.is_empty() {
                format!("Kill process(es) by name: `{name}`")
            } else if action == "kill" && pid > 0 {
                format!("Kill process by PID: `{pid}`")
            } else {
                format!("Tool: {tool_name} with args: {args}")
            }
        }
        _ => format!("Tool: {tool_name} with args: {args}"),
    }
}

pub fn check_destructive(tool_name: &str, args: &Value) -> Option<DestructiveActionRequest> {
    let risk = classify_risk(tool_name, args);
    if matches!(risk, RiskLevel::High | RiskLevel::Critical) {
        Some(DestructiveActionRequest {
            action_id: Uuid::new_v4(),
            task_id: Uuid::nil(),
            risk_level: risk,
            description: build_description(tool_name, args),
            tool_name: tool_name.to_string(),
            tool_args: args.clone(),
        })
    } else {
        None
    }
}
