//! Audio capture, resampling, and cloud-based speech-to-text pipeline.
//!
//! Pipeline: cpal input stream → crossbeam-channel → rubato resampler → Google Gemini
//! `generateContent` endpoint with inline base64 audio.

pub mod capture;
pub mod resample;
pub mod stt;

pub use capture::{AudioCapture, AudioCaptureHandle, STT_SAMPLE_RATE};
pub use stt::transcribe;
