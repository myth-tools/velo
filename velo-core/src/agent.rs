use std::sync::Arc;

use autoagents::core::agent::prebuilt::executor::ReActAgentOutput;
use autoagents::core::agent::AgentOutputT;
use autoagents_derive::agent;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, Mutex};
use uuid::Uuid;

use crate::memory::MemoryManagerHandle;
use crate::tools;

pub const SYSTEM_PROMPT: &str = r#"You are Velo, a hyper-capable autonomous OS and desktop agent running on the user's computer.
You have access to a set of tools to execute shell commands, control applications, browse the web,
capture the screen, read/write files, and manage the clipboard.

When given a task:
1. THINK step-by-step about the best approach.
2. ACT by calling the appropriate tool.
3. OBSERVE the result and reflect if needed.
4. REPEAT until the task is complete or you have a final answer.

Rules:
- Prefer precise, targeted tool calls over broad ones.
- Always explain your reasoning before acting.
- For destructive operations (delete, overwrite, system changes), clearly warn the user.
- Never fabricate tool results; if a tool fails, reflect and try an alternative approach.
- Be concise in your final answers."#;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VeloOutput {
    pub response: String,
}

impl AgentOutputT for VeloOutput {
    fn output_schema() -> &'static str {
        r#"{"type":"object","properties":{"response":{"type":"string","description":"The agent's final response text"}}}"#
    }

    fn structured_output_format() -> serde_json::Value {
        serde_json::json!({
            "type": "json_schema",
            "json_schema": {
                "name": "velo_output",
                "strict": true,
                "schema": {
                    "type": "object",
                    "properties": {
                        "response": { "type": "string" }
                    },
                    "required": ["response"],
                    "additionalProperties": false
                }
            }
        })
    }
}

impl From<ReActAgentOutput> for VeloOutput {
    fn from(out: ReActAgentOutput) -> Self {
        VeloOutput {
            response: out.response,
        }
    }
}

#[agent(
    name = "velo",
    description = "Hyper-capable autonomous OS and desktop agent.",
    tools = [
        tools::ShellTool,
        tools::ReadFileTool, tools::WriteFileTool, tools::ListDirTool, tools::DeletePathTool,
        tools::CopyFileTool, tools::MoveFileTool, tools::FindFileTool,
        tools::NavigateUrlTool, tools::ClickElementTool, tools::ScrapePageTool, tools::BrowserScreenshotTool, tools::EvaluateJavaScriptTool, tools::FillFormFieldTool,
        tools::FocusAppTool, tools::SendKeystrokesTool, tools::ListWindowsTool, tools::GetWindowGeometryTool, tools::MouseControlTool,
        tools::GetClipboardTool, tools::SetClipboardTool, tools::SetClipboardImageTool,
        tools::CaptureScreenTool,
        tools::RipgrepTool, tools::SysinfoTool, tools::ProcessTool, tools::WatchPathTool,
        tools::HttpRequestTool,
        tools::CryptoHashTool, tools::DateTimeTool, tools::EnvVarTool,
        tools::NotifyTool, tools::CompressTool, tools::ImageInfoTool,
        tools::GuiClickTool, tools::GuiTypeTool, tools::GuiDragTool, tools::GuiScrollTool,
        tools::GuiGetCoordsTool, tools::GuiMiddleClickTool, tools::GuiRightClickTool, tools::GuiDoubleClickTool,
        tools::TaskTool,
    ],
    output = VeloOutput
)]
#[derive(Clone)]
pub struct VeloAgent {
    pub hooks: Option<Arc<VeloAgentHooks>>,
}

#[derive(Clone)]
pub struct VeloAgentHooks {
    pub event_tx: mpsc::UnboundedSender<(String, serde_json::Value)>,
    pub approval_tx: mpsc::UnboundedSender<(Uuid, mpsc::UnboundedSender<bool>)>,
    pub pending_task_id: Arc<Mutex<Option<Uuid>>>,
    pub memory: Option<MemoryManagerHandle>,
}
