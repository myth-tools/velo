//! PCM resampler: converts native device sample rate to 16 kHz mono f32
//! required by the STT pipeline, using the `rubato` crate (sinc interpolation).

use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use tracing::debug;

use crate::{audio::capture::STT_SAMPLE_RATE, error::VeloError};

/// Stateful resampler for a single audio stream.
pub struct PcmResampler {
    resampler: SincFixedIn<f32>,
    channels: usize,
    chunk_size: usize,
    /// Leftover samples from the previous call that didn't fill a full chunk.
    remainder: Vec<Vec<f32>>,
}

impl PcmResampler {
    /// Create a new resampler.
    ///
    /// * `input_sample_rate` — native device rate (e.g. 44100 or 48000)
    /// * `channels` — number of channels (will be mixed to mono internally)
    pub fn new(input_sample_rate: u32, channels: usize) -> Result<Self, VeloError> {
        let ratio = STT_SAMPLE_RATE as f64 / input_sample_rate as f64;
        let chunk_size = 1024_usize;

        let params = SincInterpolationParameters {
            sinc_len: 256,
            f_cutoff: 0.95,
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: 256,
            window: WindowFunction::BlackmanHarris2,
        };

        let resampler = SincFixedIn::<f32>::new(ratio, 2.0, params, chunk_size, channels)
            .map_err(|e| VeloError::Resample(e.to_string()))?;

        let remainder = vec![Vec::new(); channels];

        Ok(Self {
            resampler,
            channels,
            chunk_size,
            remainder,
        })
    }

    /// Process a flat interleaved PCM buffer.
    /// Returns resampled mono f32 samples at 16 kHz.
    pub fn process_interleaved(&mut self, data: &[f32]) -> Result<Vec<f32>, VeloError> {
        // De-interleave
        let mut deinterleaved: Vec<Vec<f32>> = vec![Vec::new(); self.channels];
        for (i, &sample) in data.iter().enumerate() {
            deinterleaved[i % self.channels].push(sample);
        }

        // Append to remainder
        for (ch, chunk) in deinterleaved.iter().enumerate() {
            self.remainder[ch].extend_from_slice(chunk);
        }

        let mut output_mono = Vec::new();

        // Process complete chunks
        while self.remainder[0].len() >= self.chunk_size {
            let input_chunk: Vec<Vec<f32>> = (0..self.channels)
                .map(|ch| self.remainder[ch].drain(..self.chunk_size).collect())
                .collect();

            let resampled = self
                .resampler
                .process(
                    &input_chunk.iter().map(|v| v.as_slice()).collect::<Vec<_>>(),
                    None,
                )
                .map_err(|e| VeloError::Resample(e.to_string()))?;

            // Mix all channels to mono
            let frame_count = resampled[0].len();
            for frame in 0..frame_count {
                let mono: f32 =
                    resampled.iter().map(|ch| ch[frame]).sum::<f32>() / self.channels as f32;
                output_mono.push(mono);
            }
        }

        debug!(
            "Resampled {} → {} samples (mono 16kHz)",
            data.len(),
            output_mono.len()
        );
        Ok(output_mono)
    }
}
