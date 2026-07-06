use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::engine::Engine;
use futures_util::stream::FuturesUnordered;
use futures_util::StreamExt;
use regex::Regex;
use serde_json::Value;
use tokio::sync::Semaphore;
use tracing::Instrument as _;
use tracing::{info_span, instrument, warn};

use super::{SubAgent, SubAgentError};
use crate::config::VeloConfig;

// ── Hard limits (industry‑grade resource guards) ───────────────────────────

const MAX_FILE_SIZE_IMAGE: u64 = 20 * 1024 * 1024; // 20 MiB
const MAX_FILE_SIZE_AUDIO: u64 = 50 * 1024 * 1024; // 50 MiB
const MAX_FILE_SIZE_VIDEO: u64 = 500 * 1024 * 1024; // 500 MiB
const MAX_FILE_SIZE_PDF: u64 = 100 * 1024 * 1024; // 100 MiB
const MAX_FRAMES: usize = 8;
const MAX_CONCURRENT_VISION_CALLS: usize = 3;
const MAX_RETRIES: u32 = 3;
const BASE_RETRY_DELAY_MS: u64 = 1000;

// ── Public sub-agent ──────────────────────────────────────────────────────

pub struct MediaAnalysisSubAgent;

#[async_trait]
impl SubAgent for MediaAnalysisSubAgent {
    fn name(&self) -> &str {
        "media_analysis"
    }

    fn description(&self) -> &str {
        "Analyzes images (describe, OCR, caption), transcribes audio (voice memos, recordings), \
         analyzes video (combined frame + audio analysis), and extracts text from PDFs. \
         Handles common formats: JPG/PNG/GIF/BMP/WebP, MP3/WAV/OGG/FLAC/M4A/AAC, \
         MP4/AVI/MOV/MKV/WEBM, and PDF."
    }

    #[instrument(name = "media_analysis", skip(self, config, cancel), fields(prompt_len = %prompt.len()))]
    async fn execute(
        &self,
        prompt: &str,
        config: &VeloConfig,
        cancel: super::CancelToken,
    ) -> Result<String, SubAgentError> {
        if cancel.is_cancelled() {
            return Err(SubAgentError::Cancelled);
        }
        let file_paths = extract_file_paths(prompt);

        // No files → pure text‑to‑vision prompt (e.g. "describe the last screenshot").
        if file_paths.is_empty() {
            return with_retry(
                || call_vision_model(prompt, None, config, cancel.clone()),
                cancel.clone(),
                "vision_text_only",
            )
            .await;
        }

        let mut results: Vec<String> = Vec::with_capacity(file_paths.len());

        for fp in &file_paths {
            if cancel.is_cancelled() {
                return Err(SubAgentError::Cancelled);
            }

            match classify_and_process(fp, prompt, config, cancel.clone()).await {
                Ok(out) => results.push(format!("[{fp}]\n{out}")),
                Err(e) => results.push(format!("[{fp}]\nError: {e}")),
            }
        }

        Ok(results.join("\n\n---\n\n"))
    }
}

// ── File routing ───────────────────────────────────────────────────────────

#[instrument(name = "classify", skip(config, cancel), fields(path = %path))]
async fn classify_and_process(
    path: &str,
    original_prompt: &str,
    config: &VeloConfig,
    cancel: super::CancelToken,
) -> Result<String, SubAgentError> {
    let p = std::path::Path::new(path);

    // 1. Validate existence.
    if !p.exists() {
        return Err(SubAgentError::Media(format!(
            "Path does not exist: {path}. Make sure the file path is correct and accessible."
        )));
    }

    // 2. Validate file size and read metadata.
    let meta = tokio::fs::metadata(path).await?;
    let file_size = meta.len();

    // 3. Determine type — use both extension and magic bytes.
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let is_image = matches!(
        ext.as_str(),
        "jpg"
            | "jpeg"
            | "png"
            | "gif"
            | "bmp"
            | "webp"
            | "tiff"
            | "tif"
            | "ico"
            | "avif"
            | "heic"
            | "heif"
    ) || magic_matches_image(path).await;

    let is_audio = matches!(
        ext.as_str(),
        "mp3" | "wav" | "ogg" | "flac" | "m4a" | "aac" | "wma"
    ) || magic_matches_audio(path).await;

    let is_video = matches!(
        ext.as_str(),
        "mp4" | "avi" | "mov" | "mkv" | "wmv" | "flv" | "webm"
    );

    let is_pdf = ext == "pdf";

    // 4. Size‑gate and dispatch.
    if is_image {
        if file_size > MAX_FILE_SIZE_IMAGE {
            return Err(SubAgentError::Media(format!(
                "Image too large ({file_size} bytes, max {MAX_FILE_SIZE_IMAGE}). \
                 Resize or compress the image before analysis."
            )));
        }
        let description = analyze_image_file(path, original_prompt, config, cancel.clone()).await?;
        return Ok(description);
    }

    if is_audio {
        if file_size > MAX_FILE_SIZE_AUDIO {
            return Err(SubAgentError::Media(format!(
                "Audio file too large ({file_size} bytes, max {MAX_FILE_SIZE_AUDIO}). \
                 Consider compressing or trimming the file."
            )));
        }
        let transcription = transcribe_audio_file(path, config, cancel.clone()).await?;
        return Ok(format!("Transcription: {transcription}"));
    }

    if is_video {
        if file_size > MAX_FILE_SIZE_VIDEO {
            return Err(SubAgentError::Media(format!(
                "Video too large ({file_size} bytes, max {MAX_FILE_SIZE_VIDEO}). \
                 Consider compressing or trimming the file."
            )));
        }
        let analysis = analyze_video_file(path, original_prompt, config, cancel.clone()).await?;
        return Ok(analysis);
    }

    if is_pdf {
        if file_size > MAX_FILE_SIZE_PDF {
            return Err(SubAgentError::Media(format!(
                "PDF too large ({file_size} bytes, max {MAX_FILE_SIZE_PDF}). \
                 Consider splitting or compressing the PDF."
            )));
        }
        let text = extract_pdf_text(path).await?;
        return Ok(text);
    }

    // 5. Last resort — report unsupported.
    Err(SubAgentError::Media(format!(
        "Unrecognised file type for `{path}`. \
         Supported formats: images (jpg/png/gif/bmp/webp/tiff/ico/avif/heic), \
         audio (mp3/wav/ogg/flac/m4a/aac/wma), video (mp4/avi/mov/mkv/wmv/flv/webm), \
         and PDF. Extension: `.{ext}`, size: {file_size} bytes."
    )))
}

// ── Retry helper ───────────────────────────────────────────────────────────

async fn with_retry<Fut, T>(
    mut f: impl FnMut() -> Fut,
    cancel: super::CancelToken,
    label: &'static str,
) -> Result<T, SubAgentError>
where
    Fut: Future<Output = Result<T, SubAgentError>>,
{
    let mut delay = Duration::from_millis(BASE_RETRY_DELAY_MS);
    for attempt in 1..=MAX_RETRIES {
        if cancel.is_cancelled() {
            return Err(SubAgentError::Cancelled);
        }
        match f().await {
            Ok(val) => return Ok(val),
            Err(e) if e.is_retryable() && attempt < MAX_RETRIES => {
                warn!(
                    label,
                    attempt,
                    delay_ms = delay.as_millis(),
                    error = %e,
                    "Retrying operation"
                );
                tokio::time::sleep(delay).await;
                delay *= 2;
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}

// ── Temp‑dir guard ─────────────────────────────────────────────────────────

struct TempDirGuard {
    path: Option<PathBuf>,
}

impl TempDirGuard {
    fn new(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        if let Some(ref p) = self.path {
            let _ = std::fs::remove_dir_all(p);
        }
    }
}

// ── Magic‑byte detection ───────────────────────────────────────────────────

async fn magic_matches_image(path: &str) -> bool {
    let Ok(mut f) = tokio::fs::File::open(path).await else {
        return false;
    };
    let mut magic = [0u8; 16];
    if tokio::io::AsyncReadExt::read_exact(&mut f, &mut magic)
        .await
        .is_err()
    {
        return false;
    }
    image_magic(&magic)
}

async fn magic_matches_audio(path: &str) -> bool {
    let Ok(mut f) = tokio::fs::File::open(path).await else {
        return false;
    };
    let mut magic = [0u8; 16];
    if tokio::io::AsyncReadExt::read_exact(&mut f, &mut magic)
        .await
        .is_err()
    {
        return false;
    }
    audio_magic(&magic)
}

fn image_magic(magic: &[u8; 16]) -> bool {
    magic.starts_with(b"\x89PNG")
        || magic.starts_with(b"\xff\xd8\xff")
        || magic.starts_with(b"GIF87a")
        || magic.starts_with(b"GIF89a")
        || magic.starts_with(b"BM")
        || magic.starts_with(b"RIFF")
}

fn audio_magic(magic: &[u8; 16]) -> bool {
    magic.starts_with(b"ID3")
        || magic.starts_with(b"\xff\xfb")
        || magic.starts_with(b"\xff\xf3")
        || magic.starts_with(b"\xff\xfa")
        || magic.starts_with(b"\xff\xfc")
        || magic.starts_with(b"OggS")
        || magic.starts_with(b"fLaC")
        || (&magic[..4] == b"RIFF" && &magic[8..12] == b"WAVE")
}

// ── File‑path extraction ───────────────────────────────────────────────────

fn extract_file_paths(text: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut paths = Vec::new();
    let home = std::env::var("HOME").unwrap_or_default();

    let re = Regex::new(r#"(?m)(?:file://)?(/[^\s"'<>|()\[\]]+|\.\.?/[^\s"'<>|()\[\]]+)"#).unwrap();

    for cap in re.captures_iter(text) {
        let raw = cap
            .get(1)
            .map_or(cap.get(0).unwrap().as_str(), |m| m.as_str());
        let expanded = if let Some(stripped) = raw.strip_prefix("~/") {
            format!("{home}/{stripped}")
        } else {
            raw.to_string()
        };
        if seen.insert(expanded.clone()) && Path::new(&expanded).is_file() {
            paths.push(expanded);
        }
    }

    paths
}

// ── Image analysis ─────────────────────────────────────────────────────────

async fn analyze_image_file(
    file_path: &str,
    original_prompt: &str,
    config: &VeloConfig,
    cancel: super::CancelToken,
) -> Result<String, SubAgentError> {
    if cancel.is_cancelled() {
        return Err(SubAgentError::Cancelled);
    }
    let data_url = file_to_image_data_url(file_path).await?;
    if cancel.is_cancelled() {
        return Err(SubAgentError::Cancelled);
    }
    let prompt_owned = original_prompt.to_string();
    let data_owned = data_url.clone();
    let c = cancel.clone();
    with_retry(
        move || {
            let p = prompt_owned.clone();
            let d = data_owned.clone();
            let cc = c.clone();
            async move { call_vision_model(&p, Some(&d), config, cc).await }
        },
        cancel,
        "vision_image",
    )
    .await
}

async fn file_to_image_data_url(file_path: &str) -> Result<String, SubAgentError> {
    let bytes = tokio::fs::read(file_path).await?;

    let mime = if bytes.starts_with(b"\x89PNG") {
        "image/png"
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        "image/gif"
    } else if bytes.starts_with(b"BM") {
        "image/bmp"
    } else if bytes.starts_with(b"RIFF") {
        "image/webp"
    } else {
        "image/png"
    };

    let b64 = BASE64.encode(&bytes);
    Ok(format!("data:{mime};base64,{b64}"))
}

#[instrument(name = "vision", skip(config, cancel), fields(model = %config.vision_model))]
async fn call_vision_model(
    text_prompt: &str,
    image_data_url: Option<&str>,
    config: &VeloConfig,
    cancel: super::CancelToken,
) -> Result<String, SubAgentError> {
    let client = crate::tools::http_client();

    let mut content = vec![serde_json::json!({
        "type": "text",
        "text": format!("{text_prompt}\n\nProvide a thorough, detailed analysis.")
    })];

    if let Some(url) = image_data_url {
        content.push(serde_json::json!({
            "type": "image_url",
            "image_url": {"url": url}
        }));
    }

    let payload = serde_json::json!({
        "model": &config.vision_model,
        "messages": [{"role": "user", "content": content}],
        "max_tokens": config.max_tokens,
        "temperature": config.temperature
    });

    if cancel.is_cancelled() {
        return Err(SubAgentError::Cancelled);
    }
    let response = client
        .post(format!(
            "{}/chat/completions",
            config.nim_base_url.trim_end_matches('/')
        ))
        .header("Authorization", format!("Bearer {}", config.nvidia_api_key))
        .json(&payload)
        .send()
        .await
        .map_err(|e| SubAgentError::Http(format!("Vision request failed: {e}")))?;

    let body: Value = response
        .json()
        .await
        .map_err(|e| SubAgentError::Parse(format!("Vision response parse: {e}")))?;

    Ok(body["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("(no analysis)")
        .to_string())
}

// ── Audio transcription ────────────────────────────────────────────────────

async fn transcribe_audio_file(
    file_path: &str,
    config: &VeloConfig,
    cancel: super::CancelToken,
) -> Result<String, SubAgentError> {
    let bytes = tokio::fs::read(file_path).await?;
    let b64 = BASE64.encode(&bytes);

    let client = crate::tools::http_client();
    let url = format!(
        "{}/models/{}:generateContent",
        config.stt_base_url.trim_end_matches('/'),
        config.stt_model,
    );

    let payload = serde_json::json!({
        "contents": [{
            "parts": [
                {"inline_data": {"mime_type": mime_for_audio(file_path), "data": b64}},
                {"text": "Transcribe the audio exactly as spoken, including speaker changes if any."}
            ]
        }]
    });

    if cancel.is_cancelled() {
        return Err(SubAgentError::Cancelled);
    }

    let c = cancel.clone();
    let url_owned = url.clone();
    let payload_owned = payload.clone();
    with_retry(
        move || {
            let cc = c.clone();
            let u = url_owned.clone();
            let p = payload_owned.clone();
            async move {
                if cc.is_cancelled() {
                    return Err(SubAgentError::Cancelled);
                }
                let response = client
                    .post(&u)
                    .query(&[("key", &config.stt_api_key)])
                    .json(&p)
                    .send()
                    .await
                    .map_err(|e| SubAgentError::Http(format!("STT request failed: {e}")))?;

                let body: Value = response
                    .json()
                    .await
                    .map_err(|e| SubAgentError::Parse(format!("STT response parse: {e}")))?;

                let text = body["candidates"][0]["content"]["parts"]
                    .as_array()
                    .and_then(|parts| {
                        let t: String = parts.iter().filter_map(|p| p["text"].as_str()).collect();
                        if t.is_empty() {
                            None
                        } else {
                            Some(t)
                        }
                    })
                    .unwrap_or_else(|| {
                        body["error"]["message"]
                            .as_str()
                            .unwrap_or("(no transcription)")
                            .to_string()
                    });

                Ok(text)
            }
        },
        cancel,
        "stt",
    )
    .await
}

fn mime_for_audio(path: &str) -> &'static str {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "flac" => "audio/flac",
        "m4a" | "aac" => "audio/aac",
        "webm" => "audio/webm",
        "wma" => "audio/x-ms-wma",
        _ => "audio/mpeg",
    }
}

// ── Video analysis ─────────────────────────────────────────────────────────

#[instrument(name = "video", skip(config, cancel), fields(path = %file_path))]
async fn analyze_video_file(
    file_path: &str,
    original_prompt: &str,
    config: &VeloConfig,
    cancel: super::CancelToken,
) -> Result<String, SubAgentError> {
    if cancel.is_cancelled() {
        return Err(SubAgentError::Cancelled);
    }

    let tmp = std::env::temp_dir().join(format!(
        "velo_video_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let frames_dir = tmp.join("frames");
    tokio::fs::create_dir_all(&frames_dir).await?;
    let _guard = TempDirGuard::new(tmp.clone());

    let frames_pattern = frames_dir.join("frame_%03d.jpg");
    let audio_path = tmp.join("audio.mp3");

    // Extract frames AND audio track in parallel.
    let extract_frames = tokio::process::Command::new("ffmpeg")
        .args([
            "-i",
            file_path,
            "-vf",
            "fps=1/5,scale=640:-1",
            "-frames:v",
            &MAX_FRAMES.to_string(),
            "-q:v",
            "5",
            "-y",
            &frames_pattern.to_string_lossy(),
        ])
        .output();

    let extract_audio = tokio::process::Command::new("ffmpeg")
        .args([
            "-i",
            file_path,
            "-vn",
            "-acodec",
            "libmp3lame",
            "-y",
            &audio_path.to_string_lossy(),
        ])
        .output();

    let (frames_result, audio_result) = tokio::join!(extract_frames, extract_audio);
    if cancel.is_cancelled() {
        return Err(SubAgentError::Cancelled);
    }

    let mut results: Vec<String> = Vec::new();

    // ── Frame analysis (concurrent, bounded) ─────────────────
    match frames_result {
        Ok(out) if out.status.success() => {
            let mut entries = Vec::new();
            if let Ok(mut dir) = tokio::fs::read_dir(&frames_dir).await {
                while let Ok(Some(entry)) = dir.next_entry().await {
                    entries.push(entry);
                }
            }
            entries.sort_by_key(|e| e.file_name());

            // Convert all frame paths to data URLs (sequential I/O, fast).
            let mut frame_urls = Vec::new();
            for entry in &entries {
                let p = entry.path().to_string_lossy().to_string();
                if let Ok(url) = file_to_image_data_url(&p).await {
                    frame_urls.push(url);
                }
            }

            // Process frames concurrently with a semaphore limit.
            let sem = Arc::new(Semaphore::new(MAX_CONCURRENT_VISION_CALLS));
            let mut concurrent = FuturesUnordered::new();

            for (i, url) in frame_urls.iter().enumerate() {
                if cancel.is_cancelled() {
                    return Err(SubAgentError::Cancelled);
                }
                let fp = format!(
                    "{}\n\nThis is frame {} of a video. Describe what you see in detail.",
                    original_prompt,
                    i + 1
                );
                let url = url.clone();
                let c = cancel.clone();
                let sem_clone = sem.clone();
                let span = info_span!("frame", idx = i + 1);
                concurrent.push(
                    async move {
                        let _permit = sem_clone.acquire().await.unwrap_or_else(|_| unreachable!());
                        call_vision_model(&fp, Some(&url), config, c.clone())
                            .await
                            .map(|analysis| format!("Frame {}: {}", i + 1, analysis))
                    }
                    .instrument(span),
                );
            }

            while let Some(result) = concurrent.next().await {
                match result {
                    Ok(text) => results.push(text),
                    Err(SubAgentError::Cancelled) => return Err(SubAgentError::Cancelled),
                    Err(e) => results.push(format!("Frame analysis error: {e}")),
                }
            }

            // ── Audio transcription ───────────────────
            if cancel.is_cancelled() {
                return Err(SubAgentError::Cancelled);
            }
            if let Ok(ao) = audio_result {
                if ao.status.success() && audio_path.exists() {
                    match transcribe_audio_file(
                        &audio_path.to_string_lossy(),
                        config,
                        cancel.clone(),
                    )
                    .await
                    {
                        Ok(tx) if !tx.is_empty() => {
                            results.push(format!("Audio transcription:\n{tx}"));
                        }
                        Ok(_) => {}
                        Err(e) => {
                            results.push(format!("Audio transcription error: {e}"));
                        }
                    }
                }
            }
        }
        Ok(out) => {
            results.push(format!(
                "ffmpeg frame extraction failed (exit: {:?}). \
                 Ensure ffmpeg is installed (`which ffmpeg`). Error: {}",
                out.status.code(),
                String::from_utf8_lossy(&out.stderr)
                    .lines()
                    .last()
                    .unwrap_or("")
            ));
        }
        Err(e) => {
            results.push(format!(
                "ffmpeg not available: {e}. Install ffmpeg to analyze videos (`sudo apt install ffmpeg`)."
            ));
        }
    }

    if results.is_empty() {
        return Err(SubAgentError::Media(
            "Video analysis produced no results. Ensure the file is a valid video.".into(),
        ));
    }

    Ok(results.join("\n\n"))
}

// ── PDF text extraction ────────────────────────────────────────────────────

#[instrument(name = "pdf_extract", skip(), fields(path = %file_path))]
async fn extract_pdf_text(file_path: &str) -> Result<String, SubAgentError> {
    let output = tokio::time::timeout(Duration::from_secs(60), async {
        tokio::process::Command::new("pdftotext")
            .args(["-layout", file_path, "-"])
            .output()
            .await
    })
    .await;

    match output {
        Ok(Ok(out)) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout).to_string();
            if !text.trim().is_empty() {
                return Ok(truncate(text, 50_000));
            }
            Err(SubAgentError::Media(
                "PDF appears to be empty or image‑based (scanned). \
                 pdftotext extracted no text; consider OCR tools instead."
                    .into(),
            ))
        }
        Ok(Ok(out)) => Err(SubAgentError::Media(format!(
            "pdftotext failed (exit: {:?}). Stderr: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        ))),
        Ok(Err(e)) => Err(SubAgentError::Media(format!(
            "pdftotext not available: {e}. Install poppler-utils (`sudo apt install poppler-utils`)."
        ))),
        Err(_) => Err(SubAgentError::TimedOut),
    }
}

fn truncate(s: String, max: usize) -> String {
    if s.len() <= max {
        s
    } else {
        format!("{}...\n[truncated: {} chars total]", &s[..max], s.len())
    }
}
