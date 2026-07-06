use std::time::Duration;

use async_trait::async_trait;
use autoagents::core::tool::{ToolCallError, ToolInputT, ToolRuntime, ToolT};
use autoagents_derive::tool;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::process::Command;

use crate::sandbox::{build_sandboxed_env, ProcessTracker};
use crate::tools::{config, exec_err, ToolOutput};

#[derive(Serialize, Deserialize, Debug)]
pub struct ShellArgs {
    pub command: String,
    pub cwd: Option<String>,
    pub env: Option<std::collections::HashMap<String, String>>,
    pub timeout_secs: Option<u64>,
    pub stdin: Option<String>,
}

impl ToolInputT for ShellArgs {
    fn io_schema() -> &'static str {
        r#"{"type":"object","properties":{"command":{"type":"string","description":"Shell command to run (e.g. 'ls -la', 'git log --oneline'). Uses sh -c on Unix, cmd /C on Windows. Pipes, redirects, and chaining (&&, ||, ;) work as usual."},"cwd":{"type":"string","description":"Working directory for the command. Defaults to the agent's current directory."},"env":{"type":"object","description":"Extra environment variables as a JSON object, e.g. {'KEY':'val'}. Merged into the command's environment; does NOT affect the parent process.","additionalProperties":{"type":"string"}},"timeout_secs":{"type":"integer","description":"Maximum execution time in seconds. Command is killed with SIGKILL on timeout. Default uses the agent's configured shell timeout."},"stdin":{"type":"string","description":"Text to pipe into the command's standard input. Useful for interactive programs or multi-line input."}}}"#
    }
}

#[tool(name = "shell", description = "Execute a shell command and return stdout+stderr with exit code. Supports stdin, env vars, custom cwd, timeout. Output truncated at 64KB. Cross-platform: uses sh -c (Unix) or cmd /C (Windows). BEST FOR: running git, compilers, scripts, package managers, or any tool not covered by a dedicated function. AVOID for: file read/write (use read_file/write_file — safer, no escaping issues), text search (use ripgrep — faster), process management (use manage_process — no grep needed), file copy/move (use copy_file/move_file — preserves metadata).", input = ShellArgs)]
#[derive(Default, Clone)]
pub struct ShellTool;

#[async_trait]
impl ToolRuntime for ShellTool {
    async fn execute(&self, raw_args: Value) -> Result<Value, ToolCallError> {
        let args: ShellArgs = serde_json::from_value(raw_args)?;
        let cfg = config();
        let timeout = Duration::from_secs(args.timeout_secs.unwrap_or(cfg.shell_timeout_secs));

        let mut cmd = if cfg!(target_os = "windows") {
            let mut c = Command::new("cmd");
            c.arg("/C").arg(&args.command);
            c
        } else {
            let mut c = Command::new("sh");
            c.arg("-c").arg(&args.command);
            c
        };

        cmd.stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        if args.stdin.is_some() {
            cmd.stdin(std::process::Stdio::piped());
        }

        if let Some(dir) = &args.cwd {
            cmd.current_dir(dir);
        }

        let sandboxed_env = build_sandboxed_env(args.env.as_ref());
        cmd.env_clear();
        for (k, v) in &sandboxed_env {
            cmd.env(k, v);
        }

        #[cfg(unix)]
        {
            cmd.process_group(0);
        }

        let group_id = ProcessTracker::register("shell").await;

        let mut child = cmd
            .spawn()
            .map_err(|e| exec_err(format!("Spawn failed: {e}")))?;

        if let Some(pid) = child.id() {
            ProcessTracker::track_pid(group_id, pid).await;
        }

        if let Some(stdin_text) = args.stdin {
            if let Some(mut w) = child.stdin.take() {
                let _ = w.write_all(stdin_text.as_bytes()).await;
                let _ = w.shutdown().await;
            }
        }

        let result = tokio::time::timeout(timeout, stream_output(child)).await;

        match result {
            Err(_) => {
                ProcessTracker::kill_group(group_id).await;
                Err(exec_err(format!(
                    "Command timed out after {timeout:?}. The process group has been terminated."
                )))
            }
            Ok(Ok(output)) => {
                let mut result = ToolOutput::shell(output.stdout, output.stderr, output.exit_code);
                if output.truncated {
                    result = result.with_truncated();
                }
                Ok(result.into())
            }
            Ok(Err(e)) => Err(exec_err(e.to_string())),
        }
    }
}

struct ShellOutput {
    stdout: String,
    stderr: String,
    exit_code: i32,
    truncated: bool,
}

async fn stream_output(
    mut child: tokio::process::Child,
) -> Result<ShellOutput, crate::error::VeloError> {
    let stdout = child.stdout.take().ok_or_else(|| {
        crate::error::VeloError::FileOp(std::io::Error::other("stdout not piped"))
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        crate::error::VeloError::FileOp(std::io::Error::other("stderr not piped"))
    })?;

    let mut stdout_buf = Vec::new();
    let mut stderr_buf = Vec::new();
    let mut total = 0usize;
    let max = 65_536;
    let mut truncated = false;

    let mut stdout_reader = tokio::io::BufReader::new(stdout).lines();
    let mut stderr_reader = tokio::io::BufReader::new(stderr).lines();

    loop {
        tokio::select! {
            biased;
            line = stdout_reader.next_line() => {
                match line {
                    Ok(Some(l)) => {
                        if total + l.len() + 1 > max { stdout_buf.push("[...truncated]".into()); truncated = true; break; }
                        total += l.len() + 1;
                        stdout_buf.push(l);
                    }
                    Ok(None) => break,
                    Err(e) => { tracing::warn!("stdout: {e}"); break; }
                }
            }
            line = stderr_reader.next_line() => {
                match line {
                    Ok(Some(l)) => {
                        if total + l.len() + 1 > max { stderr_buf.push("[...truncated]".into()); truncated = true; break; }
                        total += l.len() + 1;
                        stderr_buf.push(l);
                    }
                    Ok(None) => {}
                    Err(e) => { tracing::warn!("stderr: {e}"); }
                }
            }
        }
    }

    while let Ok(Some(l)) = stderr_reader.next_line().await {
        if total + l.len() + 1 > max {
            stderr_buf.push("[...truncated]".into());
            truncated = true;
            break;
        }
        total += l.len() + 1;
        stderr_buf.push(l);
    }

    let status = child
        .wait()
        .await
        .map_err(crate::error::VeloError::FileOp)?;

    Ok(ShellOutput {
        stdout: stdout_buf.join("\n"),
        stderr: stderr_buf.join("\n"),
        exit_code: status.code().unwrap_or(-1),
        truncated,
    })
}
