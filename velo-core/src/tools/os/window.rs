use std::time::Duration;

use async_trait::async_trait;
use autoagents::core::tool::{ToolCallError, ToolInputT, ToolRuntime, ToolT};
use autoagents_derive::tool;
use enigo::{Enigo, Keyboard, Settings};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tools::{exec_err, ToolOutput};

#[derive(Serialize, Deserialize, Debug)]
pub struct FocusAppArgs {
    pub name: String,
}

impl ToolInputT for FocusAppArgs {
    fn io_schema() -> &'static str {
        r#"{"type":"object","properties":{"name":{"type":"string","description":"Window title (or application name) to bring to the foreground. Uses substring matching. Works on Wayland (ydotool) and X11 (xdotool). Examples: 'Firefox', 'Terminal', 'code'."}}}"#
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SendKeystrokesArgs {
    pub text: String,
    pub app_name: Option<String>,
}

impl ToolInputT for SendKeystrokesArgs {
    fn io_schema() -> &'static str {
        r#"{"type":"object","properties":{"text":{"type":"string","description":"Text to type into the focused window. Supports regular characters and spaces. Does NOT support special keys (Enter, Tab, Ctrl+C) — use the shell tool with xdotool/ydotool for that."},"app_name":{"type":"string","description":"Optional window/app name to focus BEFORE typing. Same as calling focus_app first."}}}"#
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ListWindowsArgs {
    pub filter: Option<String>,
}

impl ToolInputT for ListWindowsArgs {
    fn io_schema() -> &'static str {
        r#"{"type":"object","properties":{"filter":{"type":"string","description":"Optional substring filter. Only returns windows whose title contains this string. Omit to list ALL open windows."}}}"#
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct GetWindowGeometryArgs {
    pub name: String,
}

impl ToolInputT for GetWindowGeometryArgs {
    fn io_schema() -> &'static str {
        r#"{"type":"object","properties":{"name":{"type":"string","description":"Window title or app name to find. Returns position (x, y) and size (width, height)."}}}"#
    }
}

#[tool(name = "focus_app", description = "Bring a window to the foreground by matching its title or app name (substring). Uses ydotool (Wayland) or xdotool (X11). BEST FOR: switching to a specific application before typing or clicking. Use list_windows first to find the exact title. Use gui_get_coords to find the mouse position after focusing.", input = FocusAppArgs)]
#[derive(Default, Clone)]
pub struct FocusAppTool;

#[tool(name = "send_keystrokes", description = "Type text into the currently focused window. Optionally focus an app first (same as calling focus_app before typing). ONLY supports regular character typing — does NOT support special keys (Enter, Tab, Ctrl shortcuts). BEST FOR: filling forms, typing commands, entering text. Use the shell tool with xdotool/ydotool for special key combinations. Use gui_click first to focus a specific input field if needed.", input = SendKeystrokesArgs)]
#[derive(Default, Clone)]
pub struct SendKeystrokesTool;

#[tool(name = "list_windows", description = "List all open desktop windows with their titles. Supports optional title filter. Returns window count and titles. BEST FOR: discovering available windows before focusing one (focus_app), checking what apps are open. Use get_window_geometry for position/size after identifying the target.", input = ListWindowsArgs)]
#[derive(Default, Clone)]
pub struct ListWindowsTool;

#[tool(name = "get_window_geometry", description = "Get the position (x, y) and size (width, height) of a desktop window by title or app name. BEST FOR: determining where a window is located before using mouse_control or capture_screen with a region. Use list_windows first to discover available windows.", input = GetWindowGeometryArgs)]
#[derive(Default, Clone)]
pub struct GetWindowGeometryTool;

fn is_wayland() -> bool {
    std::env::var("WAYLAND_DISPLAY").is_ok()
}

fn xdotool_activate_window(name: &str) -> Result<String, String> {
    let output = std::process::Command::new("xdotool")
        .args(["search", "--onlyvisible", "--name", name])
        .output()
        .map_err(|e| format!("xdotool search failed: {e}"))?;

    if !output.status.success() || output.stdout.is_empty() {
        let output = std::process::Command::new("xdotool")
            .args(["search", "--onlyvisible", "--class", name])
            .output()
            .map_err(|e| format!("xdotool class search failed: {e}"))?;

        if !output.status.success() || output.stdout.is_empty() {
            return Err(format!("No window found matching '{name}'"));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let wid = stdout.lines().next().unwrap_or("").trim().to_string();
        if wid.is_empty() {
            return Err(format!("No window found matching '{name}'"));
        }

        std::process::Command::new("xdotool")
            .args(["windowactivate", &wid])
            .output()
            .map_err(|e| format!("xdotool activate failed: {e}"))?;

        return Ok(format!("Focused window '{name}' (class match, wid={wid})"));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let wid = stdout.lines().next().unwrap_or("").trim().to_string();

    if wid.is_empty() {
        return Err(format!("No window found matching '{name}'"));
    }

    std::process::Command::new("xdotool")
        .args(["windowactivate", &wid])
        .output()
        .map_err(|e| format!("xdotool activate failed: {e}"))?;

    Ok(format!("Focused window '{name}' (wid={wid})"))
}

fn ydotool_activate_window(name: &str) -> Result<String, String> {
    let output = std::process::Command::new("ydotool")
        .arg("--help")
        .output()
        .ok();
    if output.is_some() {
        return Err(format!(
            "Wayland detected with ydotool. Focus of specific windows is limited on Wayland. \
             The app name '{name}' could not be auto-focused. Try using GUI engine tools to click on the window instead."
        ));
    }
    Err(format!(
        "Wayland detected but ydotool not found. Install ydotool for limited Wayland automation support. \
         App: '{name}' could not be focused."
    ))
}

#[async_trait]
impl ToolRuntime for FocusAppTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let a: FocusAppArgs = serde_json::from_value(args)?;

        let result = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let mut last_err = String::new();
            for attempt in 0..2 {
                let r = if is_wayland() {
                    ydotool_activate_window(&a.name)
                } else {
                    xdotool_activate_window(&a.name)
                };
                match r {
                    Ok(msg) => return Ok(msg),
                    Err(e) => {
                        last_err = e;
                        if attempt == 0 {
                            std::thread::sleep(Duration::from_millis(300));
                        }
                    }
                }
            }
            Err(last_err)
        })
        .await
        .map_err(|e| exec_err(format!("Spawn: {e}")))?;

        Ok(ToolOutput::ok(result.map_err(exec_err)?).into())
    }
}

#[async_trait]
impl ToolRuntime for SendKeystrokesTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let a: SendKeystrokesArgs = serde_json::from_value(args)?;

        if let Some(ref app) = a.app_name {
            let _ = FocusAppTool
                .execute(serde_json::to_value(&FocusAppArgs { name: app.clone() }).unwrap())
                .await;
            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        let text = a.text.clone();
        let result = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let mut enigo =
                Enigo::new(&Settings::default()).map_err(|e| format!("Enigo error: {e}"))?;

            enigo.text(&text).map_err(|e| format!("Type failed: {e}"))?;
            Ok(format!("Typed {} chars", text.len()))
        })
        .await
        .map_err(|e| exec_err(format!("Spawn: {e}")))?;

        Ok(ToolOutput::ok(result.map_err(exec_err)?).into())
    }
}

#[async_trait]
impl ToolRuntime for ListWindowsTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let a: ListWindowsArgs = serde_json::from_value(args)?;
        let filter = a.filter.clone().map(|s| s.to_lowercase());

        let output = tokio::task::spawn_blocking(move || -> Result<String, String> {
            if is_wayland() {
                if let Ok(output) = std::process::Command::new("wlrctl")
                    .args(["top-level"])
                    .output()
                {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    let windows: Vec<&str> = stdout.lines().filter(|l| !l.is_empty()).collect();
                    if windows.is_empty() {
                        return Ok("No windows detected via wlrctl.".into());
                    }
                    let filtered: Vec<&&str> = match filter {
                        Some(ref f) => windows
                            .iter()
                            .filter(|w| w.to_lowercase().contains(f))
                            .collect(),
                        None => windows.iter().collect(),
                    };
                    if filtered.is_empty() {
                        return Ok("No matching windows found.".into());
                    }
                    let mut out = format!("Windows ({}):\n", filtered.len());
                    for w in filtered {
                        out.push_str(&format!("  - {w}\n"));
                    }
                    return Ok(out);
                }
                return Ok("Wayland: install wlrctl or use xdotool under XWayland.".into());
            }

            let output = std::process::Command::new("xdotool")
                .args(["search", "--onlyvisible", "--name", ".*"])
                .output()
                .map_err(|e| format!("xdotool search failed: {e}"))?;

            if !output.status.success() || output.stdout.is_empty() {
                return Ok("No windows found.".into());
            }

            let stdout = String::from_utf8_lossy(&output.stdout);
            let wids: Vec<&str> = stdout.lines().filter(|l| !l.is_empty()).collect();

            let mut windows = Vec::new();
            for wid in &wids {
                if let Ok(name_output) = std::process::Command::new("xdotool")
                    .args(["getwindowname", wid])
                    .output()
                {
                    if name_output.status.success() {
                        let name = String::from_utf8_lossy(&name_output.stdout)
                            .trim()
                            .to_string();
                        if !name.is_empty() {
                            windows.push(name);
                        }
                    }
                }
            }

            let filtered: Vec<String> = match filter {
                Some(ref f) => windows
                    .into_iter()
                    .filter(|w| w.to_lowercase().contains(f))
                    .collect(),
                None => windows,
            };

            if filtered.is_empty() {
                return Ok("No matching windows found.".into());
            }

            let mut out = format!("Windows ({}):\n", filtered.len());
            for w in &filtered {
                out.push_str(&format!("  - {w}\n"));
            }
            Ok(out)
        })
        .await
        .map_err(|e| exec_err(format!("Spawn: {e}")))?;

        Ok(ToolOutput::ok(output.map_err(exec_err)?).into())
    }
}

#[async_trait]
impl ToolRuntime for GetWindowGeometryTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let a: GetWindowGeometryArgs = serde_json::from_value(args)?;

        let output = tokio::task::spawn_blocking(move || -> Result<String, String> {
            if is_wayland() {
                return Err("get_window_geometry: not supported on Wayland (wlrctl doesn't expose geometry)".into());
            }

            let output = std::process::Command::new("xdotool")
                .args(["search", "--onlyvisible", "--name", &a.name])
                .output()
                .map_err(|e| format!("xdotool search: {e}"))?;

            if !output.status.success() || output.stdout.is_empty() {
                let output = std::process::Command::new("xdotool")
                    .args(["search", "--onlyvisible", "--class", &a.name])
                    .output()
                    .map_err(|e| format!("xdotool class search: {e}"))?;

                if !output.status.success() || output.stdout.is_empty() {
                    return Err(format!("Window not found: {}", a.name));
                }
            }

            let stdout = String::from_utf8_lossy(&output.stdout);
            let wid = stdout.lines().next().unwrap_or("").trim().to_string();

            if wid.is_empty() {
                return Err(format!("Window not found: {}", a.name));
            }

            let geo_output = std::process::Command::new("xdotool")
                .args(["getwindowgeometry", &wid])
                .output()
                .map_err(|e| format!("xdotool geometry: {e}"))?;

            let geo = String::from_utf8_lossy(&geo_output.stdout).to_string();

            let pos_output = std::process::Command::new("xdotool")
                .args(["getwindowgeometry", "--shell", &wid])
                .output()
                .ok();

            let pos_info = match pos_output {
                Some(ref p) if p.status.success() => String::from_utf8_lossy(&p.stdout).to_string(),
                _ => String::new(),
            };

            Ok(format!(
                "Window: {}\nID: {wid}\n{}\n{}",
                a.name, geo.trim(), pos_info.trim()
            ))
        })
        .await
        .map_err(|e| exec_err(format!("Spawn: {e}")))?;

        Ok(ToolOutput::ok(output.map_err(exec_err)?).into())
    }
}
