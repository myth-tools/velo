//! cpal audio capture: opens the default input device and streams raw f32 PCM
//! samples into a crossbeam channel for downstream processing.

use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    BufferSize, SampleFormat, StreamConfig,
};
use crossbeam_channel::{bounded, Receiver, Sender};
use tracing::{error, info, warn};

use crate::error::VeloError;

/// Target sample rate for the STT pipeline (16 kHz mono f32).
pub const STT_SAMPLE_RATE: u32 = 16_000;

/// A live audio capture session.
pub struct AudioCapture {
    _stream: cpal::Stream,
    pub sample_rate: u32,
    pub channels: u16,
}

/// A cloneable handle for controlling an `AudioCapture` session.
pub struct AudioCaptureHandle {
    pub rx: Receiver<Vec<f32>>,
    pub sample_rate: u32,
    pub channels: u16,
}

impl AudioCapture {
    /// Open the default input device and start capturing.
    /// Returns `(AudioCapture, AudioCaptureHandle)`.
    pub fn start() -> Result<(Self, AudioCaptureHandle), VeloError> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| VeloError::AudioDevice("No default input device found".into()))?;

        let name = device.name().unwrap_or_else(|_| "unknown".into());
        info!(device = %name, "Opening audio input device");

        let supported = device
            .default_input_config()
            .map_err(|e| VeloError::AudioDevice(e.to_string()))?;

        let sample_rate = supported.sample_rate().0;
        let channels = supported.channels();

        let config = StreamConfig {
            channels,
            sample_rate: cpal::SampleRate(sample_rate),
            buffer_size: BufferSize::Fixed(1024),
        };

        let (tx, rx): (Sender<Vec<f32>>, Receiver<Vec<f32>>) = bounded(256);

        let stream = build_stream(&device, &config, supported.sample_format(), tx.clone())
            .map_err(VeloError::AudioStream)?;

        stream
            .play()
            .map_err(|e| VeloError::AudioStream(e.to_string()))?;

        info!(sample_rate, channels, "Audio capture started");

        let capture = Self {
            _stream: stream,
            sample_rate,
            channels,
        };
        let handle = AudioCaptureHandle {
            rx,
            sample_rate,
            channels,
        };
        Ok((capture, handle))
    }

    /// Stop the audio stream.
    pub fn stop(&self) {
        if let Err(e) = self._stream.pause() {
            warn!("Failed to pause audio stream: {e}");
        }
    }
}

// ── Stream builder ─────────────────────────────────────────────────────────────

fn build_stream(
    device: &cpal::Device,
    config: &StreamConfig,
    sample_format: SampleFormat,
    tx: Sender<Vec<f32>>,
) -> Result<cpal::Stream, String> {
    let err_fn = |e| error!("Audio stream error: {e}");

    match sample_format {
        SampleFormat::F32 => device
            .build_input_stream(
                config,
                move |data: &[f32], _| {
                    let _ = tx.try_send(data.to_vec());
                },
                err_fn,
                None,
            )
            .map_err(|e| e.to_string()),

        SampleFormat::I16 => device
            .build_input_stream(
                config,
                move |data: &[i16], _| {
                    let floats: Vec<f32> =
                        data.iter().map(|&s| s as f32 / i16::MAX as f32).collect();
                    let _ = tx.try_send(floats);
                },
                err_fn,
                None,
            )
            .map_err(|e| e.to_string()),

        SampleFormat::U16 => device
            .build_input_stream(
                config,
                move |data: &[u16], _| {
                    let floats: Vec<f32> = data
                        .iter()
                        .map(|&s| (s as f32 / u16::MAX as f32) * 2.0 - 1.0)
                        .collect();
                    let _ = tx.try_send(floats);
                },
                err_fn,
                None,
            )
            .map_err(|e| e.to_string()),

        other => Err(format!("Unsupported sample format: {other:?}")),
    }
}
