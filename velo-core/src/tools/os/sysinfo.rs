use std::time::Duration;

use async_trait::async_trait;
use autoagents::core::tool::{ToolCallError, ToolInputT, ToolRuntime, ToolT};
use autoagents_derive::tool;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sysinfo::{Disks, Networks, System};

use crate::tools::{exec_err, ToolOutput};

#[derive(Serialize, Deserialize, Debug)]
pub struct SysinfoArgs {
    pub detail: Option<String>,
}

impl ToolInputT for SysinfoArgs {
    fn io_schema() -> &'static str {
        r#"{"type":"object","properties":{"detail":{"type":"string","description":"Detail level: 'summary' (default) — CPU model, OS, uptime, load, total RAM; 'cpu' — per-core usage; 'memory' — RAM + swap; 'disks' — mounts, used/free; 'network' — interface stats; 'all' — everything combined."}}}"#
    }
}

#[tool(name = "get_sysinfo", description = "Get system information: CPU model/usage, memory, disk mounts, network interfaces, OS version, uptime, load average. Detail levels: summary (default), cpu, memory, disks, network, all. BEST FOR: understanding the user's hardware, checking free disk space/memory, diagnosing performance issues. Faster and more structured than parsing shell commands.", input = SysinfoArgs)]
#[derive(Default, Clone)]
pub struct SysinfoTool;

#[async_trait]
impl ToolRuntime for SysinfoTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let a: SysinfoArgs = serde_json::from_value(args)?;
        let detail = a.detail.unwrap_or_else(|| "summary".into()).to_lowercase();

        let output = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let mut sys = System::new_all();
            sys.refresh_all();
            std::thread::sleep(Duration::from_millis(500));
            sys.refresh_all();
            sys.refresh_cpu_usage();

            let mut out = String::new();
            let show_all = detail == "all";

            if show_all || detail == "summary" || detail == "cpu" {
                out.push_str("── CPU ──\n");
                let cpus = sys.cpus();
                if !cpus.is_empty() {
                    out.push_str(&format!("Model: {}\n", cpus[0].brand()));
                }
                out.push_str(&format!(
                    "Cores: {} physical, {} logical\n",
                    sys.physical_core_count().unwrap_or(0),
                    cpus.len()
                ));
                out.push_str(&format!(
                    "Usage: {:.1}%\n",
                    sys.global_cpu_info().cpu_usage()
                ));

                for (i, cpu) in cpus.iter().enumerate() {
                    out.push_str(&format!("  Core {i}: {:.1}%\n", cpu.cpu_usage()));
                }

                let load = System::load_average();
                out.push_str(&format!(
                    "Load avg: {:.2}, {:.2}, {:.2}\n",
                    load.one, load.five, load.fifteen
                ));

                let uptime = System::uptime();
                let days = uptime / 86400;
                let hours = (uptime % 86400) / 3600;
                let mins = (uptime % 3600) / 60;
                out.push_str(&format!("Uptime: {days}d {hours}h {mins}m\n\n"));
            }

            if show_all || detail == "memory" || detail == "summary" {
                out.push_str("── Memory ──\n");
                let total = sys.total_memory();
                let used = sys.used_memory();
                let total_swap = sys.total_swap();
                let used_swap = sys.used_swap();
                out.push_str(&format!(
                    "RAM: {:.1}/{:.1} GB ({:.0}%)\n",
                    used as f64 / 1_073_741_824.0,
                    total as f64 / 1_073_741_824.0,
                    if total > 0 {
                        used as f64 / total as f64 * 100.0
                    } else {
                        0.0
                    }
                ));
                if total_swap > 0 {
                    out.push_str(&format!(
                        "Swap: {:.1}/{:.1} GB ({:.0}%)\n",
                        used_swap as f64 / 1_073_741_824.0,
                        total_swap as f64 / 1_073_741_824.0,
                        used_swap as f64 / total_swap as f64 * 100.0
                    ));
                }
                out.push('\n');
            }

            if show_all || detail == "disks" || detail == "summary" {
                out.push_str("── Disks ──\n");
                let disks = Disks::new();
                for disk in &disks {
                    let total = disk.total_space();
                    let available = disk.available_space();
                    let used = total.saturating_sub(available);
                    let usage_pct = if total > 0 {
                        used as f64 / total as f64 * 100.0
                    } else {
                        0.0
                    };
                    let mount = disk.mount_point().to_string_lossy();
                    let removable = if disk.is_removable() {
                        " (removable)"
                    } else {
                        ""
                    };
                    out.push_str(&format!(
                        "  {} at {}: {:.1}/{:.1} GB ({:.0}% full){}\n",
                        disk.name().to_string_lossy(),
                        mount,
                        used as f64 / 1_073_741_824.0,
                        total as f64 / 1_073_741_824.0,
                        usage_pct,
                        removable,
                    ));
                }
                out.push('\n');
            }

            if show_all || detail == "network" || detail == "summary" {
                out.push_str("── Network ──\n");
                let networks = Networks::new();
                for (name, data) in &networks {
                    let rx_mb = data.total_received() as f64 / 1_048_576.0;
                    let tx_mb = data.total_transmitted() as f64 / 1_048_576.0;
                    out.push_str(&format!("  {name}: ↓{rx_mb:.1}MB ↑{tx_mb:.1}MB\n"));
                }
                out.push('\n');
            }

            if show_all || detail == "summary" {
                out.push_str("── OS ──\n");
                out.push_str(&format!("Name: {}\n", System::name().unwrap_or_default()));
                out.push_str(&format!(
                    "Kernel: {}\n",
                    System::kernel_version().unwrap_or_default()
                ));
                out.push_str(&format!(
                    "OS version: {}\n",
                    System::os_version().unwrap_or_default()
                ));
                out.push_str(&format!(
                    "Hostname: {}\n",
                    System::host_name().unwrap_or_default()
                ));
            }

            Ok(out)
        })
        .await
        .map_err(|e| exec_err(format!("Spawn: {e}")))?;

        Ok(ToolOutput::ok(output.map_err(exec_err)?).into())
    }
}
