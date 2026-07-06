use std::time::Duration;

use async_trait::async_trait;
use autoagents::core::tool::{ToolCallError, ToolInputT, ToolRuntime, ToolT};
use autoagents_derive::tool;
use enigo::{Axis, Button, Coordinate, Direction, Enigo, Mouse, Settings};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tools::{exec_err, ToolOutput};

#[derive(Serialize, Deserialize, Debug)]
pub struct MouseControlArgs {
    pub action: String,
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub absolute: Option<bool>,
    pub scroll_x: Option<i32>,
    pub scroll_y: Option<i32>,
    pub clicks: Option<i32>,
    pub button: Option<String>,
}

impl ToolInputT for MouseControlArgs {
    fn io_schema() -> &'static str {
        r#"{"type":"object","properties":{"action":{"type":"string","description":"Mouse action: 'move' — move cursor to (x,y); 'click' — click at current or (x,y) position; 'doubleclick' — double-click; 'rightclick' — right-click; 'middleclick' — middle-click; 'scroll' — scroll by scroll_x/scroll_y; 'get_position' — returns current cursor coordinates."},"x":{"type":"integer","description":"X coordinate (screen position in pixels). Required for move, click, doubleclick, rightclick, middleclick."},"y":{"type":"integer","description":"Y coordinate (screen position in pixels). Required for move, click, doubleclick, rightclick, middleclick."},"absolute":{"type":"boolean","description":"If true (default), coordinates are absolute screen positions. If false, coordinates are relative to current cursor position."},"scroll_x":{"type":"integer","description":"Horizontal scroll amount (positive=right, negative=left). Only for scroll action."},"scroll_y":{"type":"integer","description":"Vertical scroll amount (positive=down, negative=up). Only for scroll action."},"clicks":{"type":"integer","description":"Number of clicks (default 1). Only for click action."},"button":{"type":"string","description":"Mouse button: 'left' (default), 'right', 'middle'. Only for click action."}}}"#
    }
}

#[tool(name = "mouse_control", description = "Direct mouse control: move, click (left/right/middle), double-click, scroll, or get cursor position. Coordinates can be absolute or relative. BEST FOR: precise cursor positioning by pixel coordinates. Use gui_click/gui_right_click/gui_middle_click/gui_double_click (from gui_engine) when you have coordinates in 'x,y' format from screen analysis. Use click_element (from browser) for in-browser clicking by CSS selector.", input = MouseControlArgs)]
#[derive(Default, Clone)]
pub struct MouseControlTool;

fn parse_button(s: &str) -> Result<Button, String> {
    Ok(match s.to_lowercase().as_str() {
        "left" => Button::Left,
        "right" => Button::Right,
        "middle" => Button::Middle,
        other => {
            return Err(format!(
                "Unknown button '{other}'. Use left, right, or middle"
            ))
        }
    })
}

fn do_click(
    enigo: &mut Enigo,
    x: Option<i32>,
    y: Option<i32>,
    button: Button,
    clicks: i32,
    absolute: bool,
) -> Result<(), String> {
    if let (Some(x), Some(y)) = (x, y) {
        let coord = if absolute {
            Coordinate::Abs
        } else {
            Coordinate::Rel
        };
        enigo
            .move_mouse(x, y, coord)
            .map_err(|e| format!("Move failed: {e}"))?;
        std::thread::sleep(Duration::from_millis(30));
    }

    for _ in 0..clicks {
        enigo
            .button(button, Direction::Click)
            .map_err(|e| format!("Click failed: {e}"))?;
        std::thread::sleep(Duration::from_millis(40));
    }
    Ok(())
}

#[async_trait]
impl ToolRuntime for MouseControlTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let a: MouseControlArgs = serde_json::from_value(args)?;
        let action = a.action.to_lowercase();

        let result = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let mut enigo = Enigo::new(&Settings::default())
                .map_err(|e| format!("Enigo error: {e}"))?;
            let absolute = a.absolute.unwrap_or(true);

            match action.as_str() {
                "move" => {
                    let x = a.x.ok_or("x required for move")?;
                    let y = a.y.ok_or("y required for move")?;
                    let coord = if absolute { Coordinate::Abs } else { Coordinate::Rel };
                    enigo.move_mouse(x, y, coord).map_err(|e| format!("Move failed: {e}"))?;
                    Ok(format!("Moved mouse to ({x}, {y})"))
                }
                "click" => {
                    let button = parse_button(a.button.as_deref().unwrap_or("left"))?;
                    do_click(&mut enigo, a.x, a.y, button, a.clicks.unwrap_or(1), absolute)?;
                    Ok(format!("Clicked with {button:?}"))
                }
                "doubleclick" | "double_click" => {
                    let button = parse_button(a.button.as_deref().unwrap_or("left"))?;
                    do_click(&mut enigo, a.x, a.y, button, 2, absolute)?;
                    Ok("Double-clicked".into())
                }
                "rightclick" | "right_click" => {
                    do_click(&mut enigo, a.x, a.y, Button::Right, 1, absolute)?;
                    Ok("Right-clicked".into())
                }
                "middleclick" | "middle_click" => {
                    do_click(&mut enigo, a.x, a.y, Button::Middle, 1, absolute)?;
                    Ok("Middle-clicked".into())
                }
                "scroll" => {
                    let x = a.x.unwrap_or(0);
                    let y = a.y.unwrap_or(0);
                    enigo.move_mouse(x, y, Coordinate::Abs)
                        .map_err(|e| format!("Move failed: {e}"))?;

                    let dx = a.scroll_x.unwrap_or(0);
                    let dy = a.scroll_y.unwrap_or(0);
                    if dx != 0 {
                        enigo.scroll(dx, Axis::Horizontal)
                            .map_err(|e| format!("H-scroll failed: {e}"))?;
                    }
                    if dy != 0 {
                        enigo.scroll(dy, Axis::Vertical)
                            .map_err(|e| format!("V-scroll failed: {e}"))?;
                    }
                    Ok(format!("Scrolled at ({x},{y}) by ({dx},{dy})"))
                }
                "get_position" | "getpos" | "position" => {
                    let pos = enigo.location().map_err(|e| format!("Location error: {e}"))?;
                    Ok(format!("Position: ({}, {})", pos.0, pos.1))
                }
                other => Err(format!("Unknown action '{other}'. Use: move, click, doubleclick, rightclick, middleclick, scroll, get_position")),
            }
        })
        .await
        .map_err(|e| exec_err(format!("Spawn error: {e}")))?;

        Ok(ToolOutput::ok(result.map_err(exec_err)?).into())
    }
}
