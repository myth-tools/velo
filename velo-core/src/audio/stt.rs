//! Cloud STT client — Google Gemini speech-to-text.
//!
//! Sends 16 kHz mono PCM audio as inline base64 data to the Gemini
//! `generateContent` endpoint.

use base64::Engine;
use chrono::Utc;
use tracing::info;

use crate::{config::VeloConfig, error::VeloError, events::SttTranscript};

/// Transcribe a buffer of 16 kHz mono f32 PCM samples via Google Gemini.
pub async fn transcribe(
    pcm_mono_16khz: &[f32],
    config: &VeloConfig,
) -> Result<SttTranscript, VeloError> {
    let wav_bytes = pcm_to_wav(pcm_mono_16khz, 16_000);
    let b64 = base64::engine::general_purpose::STANDARD.encode(&wav_bytes);

    let endpoint = format!(
        "{}/{}:generateContent",
        config.stt_base_url.trim_end_matches('/'),
        config.stt_model.trim_start_matches('/'),
    );

    let body = serde_json::json!({
        "contents": [{
            "parts": [
                {
                    "inline_data": {
                        "mime_type": "audio/wav",
                        "data": b64,
                    }
                },
                {
                    "text": "Transcribe the audio exactly as spoken. Return only the transcribed text."
                }
            ]
        }]
    });

    let client = reqwest::Client::new();
    let response = client
        .post(&endpoint)
        .header("x-goog-api-key", &config.stt_api_key)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| VeloError::Stt(format!("HTTP request failed: {e}")))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(VeloError::Stt(format!("Gemini API error {status}: {body}")));
    }

    let result: serde_json::Value = response
        .json()
        .await
        .map_err(|e| VeloError::Stt(format!("JSON parse error: {e}")))?;

    let text = result["candidates"][0]["content"]["parts"][0]["text"]
        .as_str()
        .ok_or_else(|| {
            let snippet = serde_json::to_string(&result).unwrap_or_default();
            VeloError::Stt(format!("unexpected Gemini response format: {snippet}"))
        })?
        .trim()
        .to_string();

    info!(text = %text, "STT transcript (Gemini)");
    Ok(SttTranscript {
        partial: false,
        text,
        timestamp: Utc::now(),
    })
}

// ── WAV encoding ───────────────────────────────────────────────────────────────

/// Encode 16-bit mono PCM samples into a WAV container (RIFF).
fn pcm_to_wav(pcm: &[f32], sample_rate: u32) -> Vec<u8> {
    let num_samples = pcm.len();
    let data_size = num_samples * 2;
    let header_size = 44;
    let file_size = header_size + data_size;

    let mut buf = Vec::with_capacity(file_size);

    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&(file_size as u32 - 8).to_le_bytes());
    buf.extend_from_slice(b"WAVE");

    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes());
    buf.extend_from_slice(&1u16.to_le_bytes());
    buf.extend_from_slice(&1u16.to_le_bytes());
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    buf.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    buf.extend_from_slice(&2u16.to_le_bytes());
    buf.extend_from_slice(&16u16.to_le_bytes());

    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&(data_size as u32).to_le_bytes());

    for &sample in pcm {
        let clamped = sample.clamp(-1.0, 1.0);
        let sample_i16 = (clamped * i16::MAX as f32) as i16;
        buf.extend_from_slice(&sample_i16.to_le_bytes());
    }

    buf
}
