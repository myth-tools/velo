use async_trait::async_trait;
use autoagents::core::tool::{ToolCallError, ToolInputT, ToolRuntime, ToolT};
use autoagents_derive::tool;
use image::GenericImageView;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tools::{exec_err, ToolOutput};

#[derive(Serialize, Deserialize, Debug)]
pub struct ImageInfoArgs {
    pub path: String,
}

impl ToolInputT for ImageInfoArgs {
    fn io_schema() -> &'static str {
        r#"{"type":"object","properties":{"path":{"type":"string","description":"Path to the image file. Supports PNG, JPEG, GIF, BMP, TIFF, WebP, ICO, PNM. Returns dimensions, format, color type, bit depth, and file size."}}}"#
    }
}

#[tool(name = "image_info", description = "Get image metadata: dimensions (width×height), format (detected from file content via magic bytes, not just extension), color type, bit depth, and file size. Supports PNG, JPEG, GIF, BMP, TIFF, WebP, ICO, PNM. BEST FOR: checking image dimensions before display, verifying format, inspecting color profiles. Use the shell tool with identify/mediainfo for EXIF and camera metadata.", input = ImageInfoArgs)]
#[derive(Default, Clone)]
pub struct ImageInfoTool;

#[async_trait]
impl ToolRuntime for ImageInfoTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let a: ImageInfoArgs = serde_json::from_value(args)?;

        let result = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let path = std::path::Path::new(&a.path);
            let metadata =
                std::fs::metadata(path).map_err(|e| format!("Cannot access file: {e}"))?;
            let file_size = metadata.len();

            let img =
                image::open(path).map_err(|e| format!("Cannot open image: {e}"))?;

            let (w, h) = img.dimensions();
            let color_type = img.color();

            // Detect format from image content, not just extension
            let format_name = detect_format(path);

            // Read file header for more accurate format detection
            let format_from_content = read_format_from_bytes(path).unwrap_or_default();
            let format_label = if !format_from_content.is_empty() && format_from_content != format_name
            {
                format!("{format_name} (detected: {format_from_content})")
            } else {
                format_name
            };

            // Bit depth estimation
            let bit_depth = color_type.bits_per_pixel();

            // Color type description
            let color_desc = describe_color_type(color_type);

            Ok(format!(
                "File: {path}\nFormat: {format_label}\nDimensions: {w}x{h}\nBit depth: {bit_depth} bpp\nColor: {color_desc}\nFile size: {file_size} bytes ({size_kb:.1} KB)",
                path = path.display(),
                size_kb = file_size as f64 / 1024.0,
            ))
        })
        .await
        .map_err(|e| exec_err(format!("Spawn: {e}")))?;

        Ok(ToolOutput::ok(result.map_err(exec_err)?).into())
    }
}

fn detect_format(path: &std::path::Path) -> String {
    match image::ImageFormat::from_path(path) {
        Ok(f) => format!("{f:?}"),
        Err(_) => "unknown".into(),
    }
}

fn read_format_from_bytes(path: &std::path::Path) -> Result<String, String> {
    use std::io::Read;

    let mut file = std::fs::File::open(path).map_err(|e| format!("Cannot open: {e}"))?;
    let mut magic = [0u8; 16];
    let n = file.read(&mut magic).unwrap_or(0);

    if n < 4 {
        return Err("File too small".into());
    }

    Ok(match &magic[..n] {
        [0x89, 0x50, 0x4E, 0x47, ..] => "PNG",
        [0xFF, 0xD8, 0xFF, ..] => "JPEG",
        [0x47, 0x49, 0x46, 0x38, 0x37, 0x61, ..] | [0x47, 0x49, 0x46, 0x38, 0x39, 0x61, ..] => {
            "GIF"
        }
        [0x42, 0x4D, ..] => "BMP",
        [0x49, 0x49, 0x2A, 0x00, ..] | [0x4D, 0x4D, 0x00, 0x2A, ..] => "TIFF",
        [0x52, 0x49, 0x46, 0x46, ..] if n >= 12 => match &magic[8..12] {
            [0x57, 0x45, 0x42, 0x50] => "WebP",
            _ => "RIFF container",
        },
        [0x00, 0x00, 0x01, 0x00, ..] => "ICO",
        [0x50, 0x31, ..]
        | [0x50, 0x32, ..]
        | [0x50, 0x33, ..]
        | [0x50, 0x34, ..]
        | [0x50, 0x35, ..]
        | [0x50, 0x36, ..] => "PNM/PPM",
        _ => "",
    }
    .into())
}

fn describe_color_type(ct: image::ColorType) -> String {
    use image::ColorType;
    match ct {
        ColorType::L8 => "Grayscale 8-bit".into(),
        ColorType::La8 => "Grayscale + Alpha 8-bit".into(),
        ColorType::Rgb8 => "RGB 8-bit".into(),
        ColorType::Rgba8 => "RGBA 8-bit".into(),
        ColorType::L16 => "Grayscale 16-bit".into(),
        ColorType::La16 => "Grayscale + Alpha 16-bit".into(),
        ColorType::Rgb16 => "RGB 16-bit".into(),
        ColorType::Rgba16 => "RGBA 16-bit".into(),
        ColorType::Rgb32F => "RGB 32-bit float".into(),
        ColorType::Rgba32F => "RGBA 32-bit float".into(),
        other => format!("{other:?}"),
    }
}
