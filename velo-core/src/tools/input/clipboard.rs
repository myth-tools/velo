use std::sync::{Mutex, OnceLock};

use async_trait::async_trait;
use autoagents::core::tool::{ToolCallError, ToolInputT, ToolRuntime, ToolT};
use autoagents_derive::tool;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tools::{exec_err, EmptyArgs, ToolOutput};

#[derive(Serialize, Deserialize, Debug)]
pub struct SetClipboardArgs {
    pub text: String,
}

impl ToolInputT for SetClipboardArgs {
    fn io_schema() -> &'static str {
        r#"{"type":"object","properties":{"text":{"type":"string","description":"Text content to place on the system clipboard. Overwrites any previous clipboard content."}}}"#
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SetClipboardImageArgs {
    pub base64_png: String,
}

impl ToolInputT for SetClipboardImageArgs {
    fn io_schema() -> &'static str {
        r#"{"type":"object","properties":{"base64_png":{"type":"string","description":"Base64-encoded PNG image data to set on the system clipboard. Get this from capture_screen or browser_screenshot which return base64 data URLs."}}}"#
    }
}

#[tool(name = "get_clipboard", description = "Read the current text content from the system clipboard. Returns the clipboard text as a string. BEST FOR: retrieving copied text, URLs, or data from other applications.", input = EmptyArgs)]
#[derive(Default, Clone)]
pub struct GetClipboardTool;

#[tool(name = "set_clipboard", description = "Set text content on the system clipboard. Overwrites whatever was there. BEST FOR: copying text for pasting into other applications, preparing data for a paste operation.", input = SetClipboardArgs)]
#[derive(Default, Clone)]
pub struct SetClipboardTool;

#[tool(name = "set_clipboard_image", description = "Set an image (base64-encoded PNG) on the system clipboard for pasting into image editors or chat apps. BEST FOR: screenshots (get base64 from capture_screen or browser_screenshot then pass here).", input = SetClipboardImageArgs)]
#[derive(Default, Clone)]
pub struct SetClipboardImageTool;

static CLIPBOARD: OnceLock<Mutex<arboard::Clipboard>> = OnceLock::new();

fn clipboard() -> &'static Mutex<arboard::Clipboard> {
    CLIPBOARD
        .get_or_init(|| Mutex::new(arboard::Clipboard::new().expect("Failed to open clipboard")))
}

#[async_trait]
impl ToolRuntime for GetClipboardTool {
    async fn execute(&self, _args: Value) -> Result<Value, ToolCallError> {
        let text = tokio::task::spawn_blocking(|| {
            clipboard()
                .lock()
                .unwrap()
                .get_text()
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| exec_err(format!("Spawn error: {e}")))?
        .map_err(exec_err)?;

        Ok(ToolOutput::ok(text).into())
    }
}

#[async_trait]
impl ToolRuntime for SetClipboardTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let a: SetClipboardArgs = serde_json::from_value(args)?;
        let text = a.text.clone();

        tokio::task::spawn_blocking(move || {
            clipboard()
                .lock()
                .unwrap()
                .set_text(text)
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| exec_err(format!("Spawn error: {e}")))?
        .map_err(exec_err)?;

        Ok(ToolOutput::ok(format!("Clipboard set ({} chars)", a.text.len())).into())
    }
}

#[async_trait]
impl ToolRuntime for SetClipboardImageTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let a: SetClipboardImageArgs = serde_json::from_value(args)?;

        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&a.base64_png)
            .map_err(|e| exec_err(format!("Invalid base64: {e}")))?;

        let img =
            image::load_from_memory(&bytes).map_err(|e| exec_err(format!("Invalid image: {e}")))?;

        let rgba = img.to_rgba8();
        let (w, h) = rgba.dimensions();
        let img_data = rgba.into_raw();

        tokio::task::spawn_blocking(move || {
            let mut cb = clipboard().lock().unwrap();
            // arboard 3.x supports set_image with width, height, and raw RGBA bytes
            cb.set_image(arboard::ImageData {
                width: w as usize,
                height: h as usize,
                bytes: std::borrow::Cow::Owned(img_data),
            })
            .map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| exec_err(format!("Spawn error: {e}")))?
        .map_err(exec_err)?;

        Ok(ToolOutput::ok(format!(
            "Image set in clipboard ({}×{} px, {} bytes)",
            w,
            h,
            bytes.len()
        ))
        .into())
    }
}
