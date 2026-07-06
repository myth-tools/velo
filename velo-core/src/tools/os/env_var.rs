use std::collections::BTreeMap;
use std::env;

use async_trait::async_trait;
use autoagents::core::tool::{ToolCallError, ToolInputT, ToolRuntime, ToolT};
use autoagents_derive::tool;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tools::{exec_err, ToolOutput};

#[derive(Serialize, Deserialize, Debug)]
pub struct EnvVarArgs {
    pub action: String,
    pub name: Option<String>,
    pub value: Option<String>,
    pub filter: Option<String>,
}

impl ToolInputT for EnvVarArgs {
    fn io_schema() -> &'static str {
        r#"{"type":"object","properties":{"action":{"type":"string","description":"Action: 'get' — read a single var; 'set' — set/change a var (current process only); 'list' — show all vars (optionally filtered); 'unset' — remove a var."},"name":{"type":"string","description":"Environment variable name (case-sensitive on Unix, case-insensitive on Windows). Required for get, set, unset."},"value":{"type":"string","description":"Value to assign. Only used with 'set' action. Empty string if omitted."},"filter":{"type":"string","description":"Substring filter for 'list' action. Only returns variables whose NAME contains this string (case-insensitive)."}}}"#
    }
}

#[tool(name = "env_var", description = "Get, set, list, or unset environment variables for the current process. Changes affect the running agent only (not the system persistently). BEST FOR: checking configuration, setting secrets for child processes, inspecting the environment. Use the shell tool to modify env for specific commands via the env parameter.", input = EnvVarArgs)]
#[derive(Default, Clone)]
pub struct EnvVarTool;

#[async_trait]
impl ToolRuntime for EnvVarTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let a: EnvVarArgs = serde_json::from_value(args)?;
        let action = a.action.to_lowercase();

        match action.as_str() {
            "get" => {
                let name = a.name.ok_or_else(|| exec_err("name required for get"))?;
                match env::var(&name) {
                    Ok(val) => Ok(ToolOutput::ok(format!("{}={}", name, val)).into()),
                    Err(env::VarError::NotPresent) => {
                        Ok(ToolOutput::ok(format!("{name}: not set")).into())
                    }
                    Err(e) => Err(exec_err(format!("Error reading {name}: {e}"))),
                }
            }
            "set" => {
                let name = a.name.ok_or_else(|| exec_err("name required for set"))?;
                let value = a.value.unwrap_or_default();
                env::set_var(&name, &value);
                Ok(ToolOutput::ok(format!("Set {}={}", name, value)).into())
            }
            "unset" => {
                let name = a.name.ok_or_else(|| exec_err("name required for unset"))?;
                env::remove_var(&name);
                Ok(ToolOutput::ok(format!("Unset {name}")).into())
            }
            "list" => {
                let filter = a.filter.as_ref().map(|s| s.to_lowercase());
                let mut vars: BTreeMap<String, String> = BTreeMap::new();
                for (key, val) in env::vars() {
                    if let Some(ref f) = filter {
                        if !key.to_lowercase().contains(f) {
                            continue;
                        }
                    }
                    vars.insert(key, val);
                }
                if vars.is_empty() {
                    return Ok(ToolOutput::ok("No matching environment variables.").into());
                }
                let mut out = format!("Environment variables ({}):\n", vars.len());
                for (key, val) in &vars {
                    out.push_str(&format!("  {key}={val}\n"));
                }
                Ok(ToolOutput::ok(out).into())
            }
            other => Err(exec_err(format!("Unknown action '{other}'"))),
        }
    }
}
