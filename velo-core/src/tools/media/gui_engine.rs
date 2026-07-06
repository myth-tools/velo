use std::time::Duration;

use async_trait::async_trait;
use autoagents::core::tool::{ToolCallError, ToolInputT, ToolRuntime, ToolT};
use autoagents_derive::tool;
use enigo::{Axis, Button, Coordinate, Direction, Enigo, Keyboard, Mouse, Settings};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tools::{exec_err, ToolOutput};

#[derive(Serialize, Deserialize, Debug)]
pub struct GuiClickArgs {
    pub coordinate: String,
    pub monitor: Option<usize>,
}

impl ToolInputT for GuiClickArgs {
    fn io_schema() -> &'static str {
        r#"{"type":"object","properties":{"coordinate":{"type":"string","description":"Coordinate in 'x,y' or 'x,y,monitor' format (comma-separated, no spaces). Typically obtained from screen analysis or gui_get_coords output. Example: '150,320' or '150,320,1'."},"monitor":{"type":"integer","description":"Monitor index (0-based). Overrides any monitor value embedded in the coordinate string."}}}"#
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct GuiTypeArgs {
    pub text: String,
}

impl ToolInputT for GuiTypeArgs {
    fn io_schema() -> &'static str {
        r#"{"type":"object","properties":{"text":{"type":"string","description":"Text to type using simulated keyboard. Supports regular alphanumeric characters. Does NOT support special keys (Enter, Tab, shortcuts). Use send_keystrokes (window tool) for typing into a focused window; use gui_click first to focus an input field if needed."}}}"#
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct GuiDragArgs {
    pub from: String,
    pub to: String,
    pub monitor: Option<usize>,
}

impl ToolInputT for GuiDragArgs {
    fn io_schema() -> &'static str {
        r#"{"type":"object","properties":{"from":{"type":"string","description":"Start coordinate in 'x,y' format (where to begin the drag)."},"to":{"type":"string","description":"End coordinate in 'x,y' format (where to release)."},"monitor":{"type":"integer","description":"Monitor index (0-based)."}}}"#
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct GuiScrollArgs {
    pub x: i32,
    pub y: i32,
    pub delta_x: Option<i32>,
    pub delta_y: Option<i32>,
    pub monitor: Option<usize>,
}

impl ToolInputT for GuiScrollArgs {
    fn io_schema() -> &'static str {
        r#"{"type":"object","properties":{"x":{"type":"integer","description":"X coordinate where to scroll (required)."},"y":{"type":"integer","description":"Y coordinate where to scroll (required)."},"delta_x":{"type":"integer","description":"Horizontal scroll amount. Negative = scroll left, positive = scroll right."},"delta_y":{"type":"integer","description":"Vertical scroll amount. Negative = scroll up, positive = scroll down."},"monitor":{"type":"integer","description":"Monitor index (0-based)."}}}"#
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct GuiGetCoordsArgs {
    pub label: Option<String>,
}

impl ToolInputT for GuiGetCoordsArgs {
    fn io_schema() -> &'static str {
        r#"{"type":"object","properties":{"label":{"type":"string","description":"Optional label for this action (returned in the output for reference). Useful when taking multiple readings."}}}"#
    }
}

#[tool(name = "gui_click", description = "Left-click at a coordinate on screen using 'x,y' format (from vision/screen analysis). Uses coordinate format 'x,y' or 'x,y,monitor' without spaces. BEST FOR: clicking UI elements identified by screen analysis coordinates. Use mouse_control (with action='click') for programmatic x/y params. Use click_element for in-browser CSS selector clicking.", input = GuiClickArgs)]
#[derive(Default, Clone)]
pub struct GuiClickTool;

#[tool(name = "gui_type", description = "Type text using simulated keyboard input at the current cursor position. Does NOT support special keys. BEST FOR: typing into text fields after clicking them. Use send_keystrokes for typing into a specific focused window.", input = GuiTypeArgs)]
#[derive(Default, Clone)]
pub struct GuiTypeTool;

#[tool(name = "gui_drag", description = "Drag the mouse from one coordinate to another (e.g., for drawing, selecting text, moving items). Takes 'from' and 'to' in 'x,y' format. BEST FOR: drag-and-drop operations, selecting text/ranges, drawing.", input = GuiDragArgs)]
#[derive(Default, Clone)]
pub struct GuiDragTool;

#[tool(name = "gui_scroll", description = "Scroll horizontally or vertically at a specific screen coordinate. Delta values: negative = scroll up/left, positive = scroll down/right. BEST FOR: scrolling documents, web pages, file lists. Use mouse_control (action='scroll') as an alternative with the same capability.", input = GuiScrollArgs)]
#[derive(Default, Clone)]
pub struct GuiScrollTool;

#[tool(name = "gui_get_coords", description = "Get the current mouse cursor screen coordinates (x, y) and active monitor info. Returns coordinates compatible with other gui_* tools. BEST FOR: finding cursor position before clicking, recording coordinates for automation. Use mouse_control (action='get_position') as an alternative.", input = GuiGetCoordsArgs)]
#[derive(Default, Clone)]
pub struct GuiGetCoordsTool;

#[tool(name = "gui_middle_click", description = "Middle-click at a coordinate on screen (typically opens links in new browser tabs). Uses same 'x,y' format as gui_click. BEST FOR: opening links in background tabs, paste on Linux.", input = GuiClickArgs)]
#[derive(Default, Clone)]
pub struct GuiMiddleClickTool;

#[tool(name = "gui_right_click", description = "Right-click at a coordinate on screen (opens context menu). Uses same 'x,y' format as gui_click. BEST FOR: accessing context menus, copy/paste operations.", input = GuiClickArgs)]
#[derive(Default, Clone)]
pub struct GuiRightClickTool;

#[tool(name = "gui_double_click", description = "Double-click at a coordinate on screen. Uses same 'x,y' format as gui_click. BEST FOR: opening files/folders, selecting words, launching applications.", input = GuiClickArgs)]
#[derive(Default, Clone)]
pub struct GuiDoubleClickTool;

fn parse_vision_coords(s: &str) -> Result<(i32, i32, Option<usize>), String> {
    let cleaned = s
        .trim()
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim()
        .to_string();

    let parts: Vec<&str> = cleaned.split(',').collect();
    if parts.len() < 2 || parts.len() > 3 {
        return Err(format!("Expected 'x,y' or 'x,y,monitor', got: '{cleaned}'"));
    }
    let x: i32 = parts[0]
        .trim()
        .parse()
        .map_err(|_| format!("Invalid X: '{}'", parts[0]))?;
    let y: i32 = parts[1]
        .trim()
        .parse()
        .map_err(|_| format!("Invalid Y: '{}'", parts[1]))?;
    let monitor = if parts.len() == 3 {
        Some(
            parts[2]
                .trim()
                .parse()
                .map_err(|_| format!("Invalid monitor: '{}'", parts[2]))?,
        )
    } else {
        None
    };
    Ok((x, y, monitor))
}

fn enigo_click(x: i32, y: i32, button: Button) -> Result<String, String> {
    let mut enigo =
        Enigo::new(&Settings::default()).map_err(|e| format!("Failed to create Enigo: {e}"))?;

    enigo
        .move_mouse(x, y, Coordinate::Abs)
        .map_err(|e| format!("Move failed: {e}"))?;

    std::thread::sleep(Duration::from_millis(50));

    enigo
        .button(button, Direction::Click)
        .map_err(|e| format!("Click failed: {e}"))?;

    Ok(format!("Clicked at ({x}, {y}) with {button:?}"))
}

// The ToolRuntime impls must stay async, but enigo ops are blocking.
// We use spawn_blocking for each call.

#[async_trait]
impl ToolRuntime for GuiClickTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let a: GuiClickArgs = serde_json::from_value(args)?;
        let (x, y, _) = parse_vision_coords(&a.coordinate).map_err(exec_err)?;

        let result = tokio::task::spawn_blocking(move || enigo_click(x, y, Button::Left))
            .await
            .map_err(|e| exec_err(format!("Spawn error: {e}")))?;

        Ok(ToolOutput::ok(result.map_err(exec_err)?).into())
    }
}

#[async_trait]
impl ToolRuntime for GuiTypeTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let a: GuiTypeArgs = serde_json::from_value(args)?;
        let text = a.text.clone();

        let result = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let mut enigo =
                Enigo::new(&Settings::default()).map_err(|e| format!("Enigo error: {e}"))?;

            enigo.text(&text).map_err(|e| format!("Type failed: {e}"))?;
            Ok(format!("Typed {} chars", text.len()))
        })
        .await
        .map_err(|e| exec_err(format!("Spawn error: {e}")))?;

        Ok(ToolOutput::ok(result.map_err(exec_err)?).into())
    }
}

#[async_trait]
impl ToolRuntime for GuiDragTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let a: GuiDragArgs = serde_json::from_value(args)?;
        let (x1, y1, _) = parse_vision_coords(&a.from).map_err(exec_err)?;
        let (x2, y2, _) = parse_vision_coords(&a.to).map_err(exec_err)?;

        let result = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let mut enigo =
                Enigo::new(&Settings::default()).map_err(|e| format!("Enigo error: {e}"))?;

            enigo
                .move_mouse(x1, y1, Coordinate::Abs)
                .map_err(|e| format!("Move failed: {e}"))?;
            std::thread::sleep(Duration::from_millis(50));

            enigo
                .button(Button::Left, Direction::Press)
                .map_err(|e| format!("Press failed: {e}"))?;

            std::thread::sleep(Duration::from_millis(80));
            enigo
                .move_mouse(x2, y2, Coordinate::Abs)
                .map_err(|e| format!("Drag move failed: {e}"))?;

            enigo
                .button(Button::Left, Direction::Release)
                .map_err(|e| format!("Release failed: {e}"))?;

            Ok(format!("Dragged from ({x1},{y1}) to ({x2},{y2})"))
        })
        .await
        .map_err(|e| exec_err(format!("Spawn error: {e}")))?;

        Ok(ToolOutput::ok(result.map_err(exec_err)?).into())
    }
}

#[async_trait]
impl ToolRuntime for GuiScrollTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let a: GuiScrollArgs = serde_json::from_value(args)?;
        let dx = a.delta_x.unwrap_or(0);
        let dy = a.delta_y.unwrap_or(0);

        let result = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let mut enigo =
                Enigo::new(&Settings::default()).map_err(|e| format!("Enigo error: {e}"))?;

            enigo
                .move_mouse(a.x, a.y, Coordinate::Abs)
                .map_err(|e| format!("Move failed: {e}"))?;

            if dx != 0 {
                enigo
                    .scroll(dx, Axis::Horizontal)
                    .map_err(|e| format!("H-scroll failed: {e}"))?;
            }
            if dy != 0 {
                enigo
                    .scroll(dy, Axis::Vertical)
                    .map_err(|e| format!("V-scroll failed: {e}"))?;
            }

            Ok(format!(
                "Scrolled at ({}, {}) by ({}, {})",
                a.x, a.y, dx, dy
            ))
        })
        .await
        .map_err(|e| exec_err(format!("Spawn error: {e}")))?;

        Ok(ToolOutput::ok(result.map_err(exec_err)?).into())
    }
}

#[async_trait]
impl ToolRuntime for GuiGetCoordsTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let a: GuiGetCoordsArgs = serde_json::from_value(args)?;

        let result = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let enigo =
                Enigo::new(&Settings::default()).map_err(|e| format!("Enigo error: {e}"))?;

            let pos = enigo
                .location()
                .map_err(|e| format!("Location error: {e}"))?;

            let label = a.label.unwrap_or_default();
            let prefix = if label.is_empty() {
                String::new()
            } else {
                format!("[{}] ", label)
            };
            Ok(format!("{}Cursor: ({}, {})", prefix, pos.0, pos.1))
        })
        .await
        .map_err(|e| exec_err(format!("Spawn error: {e}")))?;

        Ok(ToolOutput::ok(result.map_err(exec_err)?).into())
    }
}

#[async_trait]
impl ToolRuntime for GuiMiddleClickTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let a: GuiClickArgs = serde_json::from_value(args)?;
        let (x, y, _) = parse_vision_coords(&a.coordinate).map_err(exec_err)?;

        let result = tokio::task::spawn_blocking(move || enigo_click(x, y, Button::Middle))
            .await
            .map_err(|e| exec_err(format!("Spawn error: {e}")))?;

        Ok(ToolOutput::ok(result.map_err(exec_err)?).into())
    }
}

#[async_trait]
impl ToolRuntime for GuiRightClickTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let a: GuiClickArgs = serde_json::from_value(args)?;
        let (x, y, _) = parse_vision_coords(&a.coordinate).map_err(exec_err)?;

        let result = tokio::task::spawn_blocking(move || enigo_click(x, y, Button::Right))
            .await
            .map_err(|e| exec_err(format!("Spawn error: {e}")))?;

        Ok(ToolOutput::ok(result.map_err(exec_err)?).into())
    }
}

#[async_trait]
impl ToolRuntime for GuiDoubleClickTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let a: GuiClickArgs = serde_json::from_value(args)?;
        let (x, y, _) = parse_vision_coords(&a.coordinate).map_err(exec_err)?;

        let result = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let mut enigo =
                Enigo::new(&Settings::default()).map_err(|e| format!("Enigo error: {e}"))?;

            enigo
                .move_mouse(x, y, Coordinate::Abs)
                .map_err(|e| format!("Move failed: {e}"))?;
            std::thread::sleep(Duration::from_millis(30));

            enigo
                .button(Button::Left, Direction::Click)
                .map_err(|e| format!("First click failed: {e}"))?;
            std::thread::sleep(Duration::from_millis(50));

            enigo
                .button(Button::Left, Direction::Click)
                .map_err(|e| format!("Second click failed: {e}"))?;

            Ok(format!("Double-clicked at ({x}, {y})"))
        })
        .await
        .map_err(|e| exec_err(format!("Spawn error: {e}")))?;

        Ok(ToolOutput::ok(result.map_err(exec_err)?).into())
    }
}
