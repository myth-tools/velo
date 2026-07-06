use std::io::Cursor;

use async_trait::async_trait;
use autoagents::core::tool::{ToolCallError, ToolInputT, ToolRuntime, ToolT};
use autoagents_derive::tool;
use base64::Engine;
use image::GenericImageView;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use xcap::Monitor;

use crate::tools::{exec_err, ToolOutput};

#[derive(Serialize, Deserialize, Debug)]
pub struct CaptureScreenArgs {
    pub analyze: Option<bool>,
    pub monitor: Option<usize>,
    pub region: Option<Vec<i32>>,
}

impl ToolInputT for CaptureScreenArgs {
    fn io_schema() -> &'static str {
        r#"{"type":"object","properties":{"analyze":{"type":"boolean","description":"If true, run vision analysis on the screenshot and return the analysis text. If false (default), just returns the base64 PNG data URL for use with other tools."},"monitor":{"type":"integer","description":"Monitor index to capture (0-based). Defaults to the primary monitor."},"region":{"type":"array","items":{"type":"integer"},"description":"Crop the capture to a specific region: [x, y, width, height] in screen coordinates."}}}"#
    }
}

#[tool(name = "capture_screen", description = "Capture a screenshot of the entire screen, a specific monitor, or a region. Returns a base64 PNG data URL (data:image/png;base64,...) or optionally runs vision analysis. BEST FOR: seeing what's on the user's screen, getting visual context. Use browser_screenshot to capture only the browser tab (faster, in-browser). Use set_clipboard_image to put the screenshot on clipboard.", input = CaptureScreenArgs)]
#[derive(Default, Clone)]
pub struct CaptureScreenTool;

#[async_trait]
impl ToolRuntime for CaptureScreenTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let a: CaptureScreenArgs = serde_json::from_value(args)?;
        let analyze = a.analyze.unwrap_or(false);

        let monitors = Monitor::all().map_err(|e| exec_err(format!("Monitor error: {e}")))?;
        if monitors.is_empty() {
            return Err(exec_err("No monitors found"));
        }

        let monitor_idx = a.monitor.unwrap_or(0).min(monitors.len() - 1);
        let monitor = &monitors[monitor_idx];

        let scale = monitor.scale_factor();
        let (mon_x, mon_y) = (monitor.x(), monitor.y());
        let (mon_w, mon_h) = (monitor.width(), monitor.height());

        let image = monitor
            .capture_image()
            .map_err(|e| exec_err(e.to_string()))?;
        let mut img = image::DynamicImage::ImageRgba8(image);

        // Apply region crop if specified
        if let Some(region) = a.region {
            if region.len() == 4 {
                let (rx, ry, rw, rh) = (region[0], region[1], region[2], region[3]);
                // Clamp to screen bounds
                let x = rx.max(0).min(mon_w as i32) as u32;
                let y = ry.max(0).min(mon_h as i32) as u32;
                let w = rw.max(1).min((mon_w as i32 - rx).max(0)) as u32;
                let h = rh.max(1).min((mon_h as i32 - ry).max(0)) as u32;
                img = img.crop_imm(x, y, w, h);
            }
        }

        // Encode to PNG
        let mut png_bytes = Vec::new();
        img.write_to(&mut Cursor::new(&mut png_bytes), image::ImageFormat::Png)
            .map_err(|e| exec_err(format!("PNG encode: {e}")))?;

        let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
        let data_url = format!("data:image/png;base64,{b64}");

        if !analyze {
            return Ok(ToolOutput::ok(format!(
                "{}\nMonitor {monitor_idx}: {mon_x},{mon_y} {mon_w}x{mon_h} (scale: {scale})",
                data_url
            ))
            .into());
        }

        // Vision analysis
        let config = crate::tools::config();
        let client = crate::tools::http_client();

        let (_w, _h) = img.dimensions();
        let payload = serde_json::json!({
            "model": config.nim_model,
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {"type": "text", "text": "Describe the contents of this screen in detail. What applications or UI elements are visible? Focus on UI elements, text, buttons, and their positions."},
                        {"type": "image_url", "image_url": {"url": &data_url}}
                    ]
                }
            ],
            "max_tokens": config.max_tokens
        });

        let response = client
            .post(format!(
                "{}/chat/completions",
                config.nim_base_url.trim_end_matches('/')
            ))
            .header("Authorization", format!("Bearer {}", config.nvidia_api_key))
            .json(&payload)
            .send()
            .await
            .map_err(|e| exec_err(format!("Vision request failed: {e}")))?;

        let body: Value = response
            .json()
            .await
            .map_err(|e| exec_err(format!("Vision parse: {e}")))?;

        let description = body["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("(no analysis)")
            .to_string();

        Ok(ToolOutput::ok(format!("{}\n\nScreen analysis:\n{}", data_url, description)).into())
    }
}
