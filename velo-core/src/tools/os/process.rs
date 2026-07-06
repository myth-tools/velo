use std::time::Duration;

use async_trait::async_trait;
use autoagents::core::tool::{ToolCallError, ToolInputT, ToolRuntime, ToolT};
use autoagents_derive::tool;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tools::{exec_err, ToolOutput};

#[derive(Serialize, Deserialize, Debug)]
pub struct ProcessArgs {
    pub action: String,
    pub name: Option<String>,
    pub pid: Option<u32>,
    pub exact_match: Option<bool>,
    pub limit: Option<usize>,
}

impl ToolInputT for ProcessArgs {
    fn io_schema() -> &'static str {
        r#"{"type":"object","properties":{"action":{"type":"string","description":"Action to perform: 'list' — show running processes (filterable by name); 'kill' — terminate a process by name or PID."},"name":{"type":"string","description":"Process name to filter by (for list) or kill. Does substring matching by default; use exact_match for precise match. Examples: 'firefox', 'python3'."},"pid":{"type":"integer","description":"Process ID to kill (alternative to name). Only used when action='kill'."},"exact_match":{"type":"boolean","description":"If true, require exact process name match instead of substring. Default: false."},"limit":{"type":"integer","description":"Maximum number of results returned. Default: 50, Max: 200."}}}"#
    }
}

#[tool(name = "manage_process", description = "List running processes (filterable by name) or kill a process by name/PID. Kill sends SIGTERM first, then SIGKILL after 2 seconds if still alive. BEST FOR: finding what's running, terminating hung processes. Use the shell tool for more advanced process management (signals, priority).", input = ProcessArgs)]
#[derive(Default, Clone)]
pub struct ProcessTool;

#[async_trait]
impl ToolRuntime for ProcessTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let a: ProcessArgs = serde_json::from_value(args)?;
        let action = a.action.to_lowercase();
        let limit = a.limit.unwrap_or(50).min(200);

        match action.as_str() {
            "list" => {
                let exact = a.exact_match.unwrap_or(false);
                let name_filter = a.name.clone();

                let output = tokio::task::spawn_blocking(move || -> Result<String, String> {
                    let mut sys = sysinfo::System::new_all();
                    sys.refresh_all();
                    std::thread::sleep(Duration::from_millis(200));
                    sys.refresh_all();

                    let mut processes: Vec<(u32, String, f32, u64)> = sys
                        .processes()
                        .iter()
                        .filter(|(_, p)| {
                            if let Some(ref filter) = name_filter {
                                let pname = p.name().to_string().to_lowercase();
                                let filter_lower = filter.to_lowercase();
                                if exact {
                                    pname == filter_lower
                                } else {
                                    pname.contains(&filter_lower)
                                }
                            } else {
                                true
                            }
                        })
                        .map(|(_, p)| {
                            (
                                p.pid().as_u32(),
                                p.name().to_string(),
                                p.cpu_usage(),
                                p.memory(),
                            )
                        })
                        .collect();

                    processes
                        .sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
                    processes.truncate(limit);

                    if processes.is_empty() {
                        return Ok("No matching processes found.".into());
                    }

                    let mut out = format!("Processes ({} shown):\n", processes.len());
                    out.push_str(&format!(
                        "{:<8} {:<30} {:>6} {:>8}\n",
                        "PID", "NAME", "CPU%", "MEM(KB)"
                    ));
                    for (pid, name, cpu, mem) in &processes {
                        out.push_str(&format!(
                            "{:<8} {:<30} {:>5.1}% {:>8}\n",
                            pid,
                            name,
                            cpu,
                            mem / 1024
                        ));
                    }
                    Ok(out)
                })
                .await
                .map_err(|e| exec_err(format!("Spawn: {e}")))?;

                Ok(ToolOutput::ok(output.map_err(exec_err)?).into())
            }
            "kill" => {
                if let Some(pid) = a.pid {
                    kill_pid(pid).await?;
                    Ok(ToolOutput::ok(format!("Sent SIGTERM to PID {pid}")).into())
                } else if let Some(ref name) = a.name {
                    let exact = a.exact_match.unwrap_or(false);
                    let name_display = name.clone();
                    let killed = tokio::task::spawn_blocking({
                        let name = name.clone();
                        move || -> Result<Vec<u32>, String> {
                            let mut sys = sysinfo::System::new_all();
                            sys.refresh_all();

                            let pids: Vec<u32> = sys
                                .processes()
                                .iter()
                                .filter(|(_, p)| {
                                    let pname = p.name().to_string().to_lowercase();
                                    let filter = name.to_lowercase();
                                    if exact {
                                        pname == filter
                                    } else {
                                        pname.contains(&filter)
                                    }
                                })
                                .map(|(_, p)| p.pid().as_u32())
                                .collect();

                            for pid in &pids {
                                let pid_obj = sysinfo::Pid::from_u32(*pid);
                                if let Some(p) = sys.process(pid_obj) {
                                    // SIGTERM first
                                    let _ = p.kill_with(sysinfo::Signal::Term);
                                }
                                std::thread::sleep(Duration::from_millis(100));
                                // Check if still alive, then SIGKILL
                                sys.refresh_process(pid_obj);
                                if sys.process(pid_obj).is_some() {
                                    if let Some(p) = sys.process(pid_obj) {
                                        let _ = p.kill_with(sysinfo::Signal::Kill);
                                    }
                                }
                            }
                            Ok(pids)
                        }
                    })
                    .await
                    .map_err(|e| exec_err(format!("Spawn: {e}")))?;

                    let pids = killed.map_err(exec_err)?;
                    if pids.is_empty() {
                        Ok(
                            ToolOutput::ok(format!("No process found matching '{name_display}'"))
                                .into(),
                        )
                    } else {
                        Ok(ToolOutput::ok(format!(
                            "Killed {} process(es) matching '{name_display}': {:?}",
                            pids.len(),
                            pids
                        ))
                        .into())
                    }
                } else {
                    Err(exec_err("Provide either name or pid for kill action"))
                }
            }
            other => Err(exec_err(format!(
                "Unknown action '{other}'. Use list or kill"
            ))),
        }
    }
}

async fn kill_pid(pid: u32) -> Result<(), ToolCallError> {
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let mut sys = sysinfo::System::new();
        sys.refresh_process(sysinfo::Pid::from_u32(pid));
        let pid_obj = sysinfo::Pid::from_u32(pid);
        if let Some(p) = sys.process(pid_obj) {
            let _ = p.kill_with(sysinfo::Signal::Term);
            std::thread::sleep(Duration::from_millis(100));
            sys.refresh_process(pid_obj);
            if sys.process(pid_obj).is_some() {
                if let Some(p) = sys.process(pid_obj) {
                    let _ = p.kill_with(sysinfo::Signal::Kill);
                }
            }
            Ok(())
        } else {
            Err(format!("PID {pid} not found"))
        }
    })
    .await
    .map_err(|e| exec_err(format!("Spawn: {e}")))?
    .map_err(exec_err)?;
    Ok(())
}
