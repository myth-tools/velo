use std::sync::Arc;

use tokio::sync::Mutex;

static PROCESS_REGISTRY: std::sync::OnceLock<Arc<Mutex<ProcessTracker>>> =
    std::sync::OnceLock::new();

fn process_registry() -> &'static Arc<Mutex<ProcessTracker>> {
    PROCESS_REGISTRY.get_or_init(|| Arc::new(Mutex::new(ProcessTracker::new())))
}

pub struct ProcessTracker {
    groups: Vec<ProcessGroup>,
    next_id: u64,
}

struct ProcessGroup {
    id: u64,
    children: Vec<u32>,
}

impl ProcessTracker {
    fn new() -> Self {
        Self {
            groups: Vec::new(),
            next_id: 1,
        }
    }

    pub async fn register(_label: &str) -> u64 {
        let mut registry = process_registry().lock().await;
        let id = registry.next_id;
        registry.next_id += 1;
        registry.groups.push(ProcessGroup {
            id,
            children: Vec::new(),
        });
        id
    }

    pub async fn track_pid(group_id: u64, pid: u32) {
        let mut registry = process_registry().lock().await;
        if let Some(group) = registry.groups.iter_mut().find(|g| g.id == group_id) {
            group.children.push(pid);
        }
    }

    pub async fn kill_group(group_id: u64) {
        let pids = {
            let mut registry = process_registry().lock().await;
            if let Some(pos) = registry.groups.iter().position(|g| g.id == group_id) {
                let group = registry.groups.remove(pos);
                group.children
            } else {
                return;
            }
        };

        for pid in &pids {
            let _ = kill_pid_tree(*pid);
        }
    }
}

#[cfg(unix)]
fn kill_pid_tree(pid: u32) -> Result<(), String> {
    use std::process::Command;
    let _ = Command::new("kill").args(["--", &pid.to_string()]).output();
    let _ = Command::new("kill").args(["-9", &pid.to_string()]).output();
    Ok(())
}

#[cfg(not(unix))]
fn kill_pid_tree(pid: u32) -> Result<(), String> {
    use std::process::Command;
    let _ = Command::new("taskkill")
        .args(["/F", "/T", "/PID", &pid.to_string()])
        .output();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_register_and_kill() {
        let id = ProcessTracker::register("test-group").await;
        ProcessTracker::track_pid(id, 999999).await;
        ProcessTracker::kill_group(id).await;
    }
}
