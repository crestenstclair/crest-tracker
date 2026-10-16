use crate::synth::{AssetFileId, FileBrowserFolderId, FileBrowserListing};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub const MAX_SAMPLE_SOURCE_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_SAMPLE_DURATION_SECONDS: u64 = 300;
pub const MIN_SAMPLE_RATE: u32 = 8_000;
pub const MAX_SAMPLE_RATE: u32 = 192_000;
pub const MAX_SAMPLE_SCALARS: usize = 28_800_000;
pub const MAX_SAMPLE_GRAPH_PCM_BYTES: usize = 512 * 1024 * 1024;
pub const MAX_WAVEFORM_PAIRS: usize = 2_048;
pub const SAMPLE_VOICE_COUNT: usize = 16;
pub const MAX_LOOP_CROSSFADE_MILLISECONDS: f32 = 200.0;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SampleEncoding {
    SignedPcm,
    Float,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleMetadata {
    asset_id: AssetFileId,
    source_bytes: u64,
    sample_rate: u32,
    channels: u16,
    bits_per_sample: u16,
    encoding: SampleEncoding,
    frames: u64,
    duration_milliseconds: u64,
}

impl SampleMetadata {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        asset_id: AssetFileId,
        source_bytes: u64,
        sample_rate: u32,
        channels: u16,
        bits_per_sample: u16,
        encoding: SampleEncoding,
        frames: u64,
    ) -> Result<Self, SampleAssetError> {
        if source_bytes > MAX_SAMPLE_SOURCE_BYTES {
            return Err(SampleAssetError::SourceTooLarge);
        }
        if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&sample_rate) {
            return Err(SampleAssetError::UnsupportedSampleRate);
        }
        if !(1..=2).contains(&channels) {
            return Err(SampleAssetError::UnsupportedChannelCount);
        }
        let admitted_depth = match encoding {
            SampleEncoding::SignedPcm => matches!(bits_per_sample, 16 | 24 | 32),
            SampleEncoding::Float => bits_per_sample == 32,
        };
        if !admitted_depth {
            return Err(SampleAssetError::UnsupportedBitDepth);
        }
        let duration_milliseconds = frames
            .checked_mul(1_000)
            .ok_or(SampleAssetError::ArithmeticOverflow)?
            / u64::from(sample_rate);
        if frames > u64::from(sample_rate) * MAX_SAMPLE_DURATION_SECONDS {
            return Err(SampleAssetError::DurationTooLong);
        }
        Ok(Self {
            asset_id,
            source_bytes,
            sample_rate,
            channels,
            bits_per_sample,
            encoding,
            frames,
            duration_milliseconds,
        })
    }

    pub const fn asset_id(&self) -> &AssetFileId {
        &self.asset_id
    }
    pub const fn source_bytes(&self) -> u64 {
        self.source_bytes
    }
    pub const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    pub const fn channels(&self) -> u16 {
        self.channels
    }
    pub const fn bits_per_sample(&self) -> u16 {
        self.bits_per_sample
    }
    pub const fn encoding(&self) -> SampleEncoding {
        self.encoding
    }
    pub const fn frames(&self) -> u64 {
        self.frames
    }
    pub const fn duration_milliseconds(&self) -> u64 {
        self.duration_milliseconds
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DecodedSample {
    metadata: SampleMetadata,
    interleaved: Vec<f32>,
}

impl DecodedSample {
    pub fn new(metadata: SampleMetadata, interleaved: Vec<f32>) -> Result<Self, SampleAssetError> {
        let expected = usize::try_from(metadata.frames())
            .ok()
            .and_then(|frames| frames.checked_mul(usize::from(metadata.channels())))
            .ok_or(SampleAssetError::ArithmeticOverflow)?;
        if interleaved.len() != expected {
            return Err(SampleAssetError::MalformedPcm);
        }
        if interleaved.len() > MAX_SAMPLE_SCALARS {
            return Err(SampleAssetError::AssetScalarCapacityExceeded);
        }
        if interleaved.iter().any(|sample| !sample.is_finite()) {
            return Err(SampleAssetError::NonFinitePcm);
        }
        Ok(Self {
            metadata,
            interleaved,
        })
    }

    pub const fn metadata(&self) -> &SampleMetadata {
        &self.metadata
    }
    pub fn interleaved(&self) -> &[f32] {
        &self.interleaved
    }
    pub fn into_interleaved(self) -> Vec<f32> {
        self.interleaved
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WaveformPair {
    pub left_min: f32,
    pub left_max: f32,
    pub right_min: f32,
    pub right_max: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PreparedSamplePcm {
    asset_id: AssetFileId,
    sample_rate: u32,
    channels: u16,
    frames: usize,
    interleaved: Arc<[f32]>,
    waveform: Arc<[WaveformPair]>,
}

impl PreparedSamplePcm {
    pub fn new(
        asset_id: AssetFileId,
        sample_rate: u32,
        channels: u16,
        interleaved: Arc<[f32]>,
        waveform: Arc<[WaveformPair]>,
    ) -> Result<Self, SampleAssetError> {
        if !(1..=2).contains(&channels)
            || interleaved.len() > MAX_SAMPLE_SCALARS
            || waveform.len() > MAX_WAVEFORM_PAIRS
            || interleaved.iter().any(|sample| !sample.is_finite())
        {
            return Err(SampleAssetError::MalformedPcm);
        }
        let frames = interleaved.len() / usize::from(channels);
        if frames == 0 || frames * usize::from(channels) != interleaved.len() {
            return Err(SampleAssetError::MalformedPcm);
        }
        Ok(Self {
            asset_id,
            sample_rate,
            channels,
            frames,
            interleaved,
            waveform,
        })
    }

    pub const fn asset_id(&self) -> &AssetFileId {
        &self.asset_id
    }
    pub const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    pub const fn channels(&self) -> u16 {
        self.channels
    }
    pub const fn frames(&self) -> usize {
        self.frames
    }
    pub fn interleaved(&self) -> &[f32] {
        &self.interleaved
    }
    pub fn waveform(&self) -> &[WaveformPair] {
        &self.waveform
    }
    pub fn byte_len(&self) -> usize {
        self.interleaved.len() * core::mem::size_of::<f32>()
    }
}

pub trait SampleAssetCatalogPort: Send + Sync {
    fn list(&self, folder: &FileBrowserFolderId) -> Result<FileBrowserListing, SampleAssetError>;
    fn read(&self, asset: &AssetFileId) -> Result<Vec<u8>, SampleAssetError>;
}

pub trait SampleDecoderPort: Send + Sync {
    fn decode(&self, asset: &AssetFileId, bytes: &[u8]) -> Result<DecodedSample, SampleAssetError>;
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SampleLoopMode {
    Off,
    Forward,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SamplePlaybackConfig {
    pub asset_id: AssetFileId,
    pub root_note: f32,
    pub playback_start: f32,
    pub playback_end: f32,
    pub loop_mode: SampleLoopMode,
    pub loop_start: f32,
    pub loop_end: f32,
    pub crossfade_milliseconds: f32,
}

impl SamplePlaybackConfig {
    pub fn validate(&self) -> Result<(), SampleAssetError> {
        let normalized = [
            self.playback_start,
            self.playback_end,
            self.loop_start,
            self.loop_end,
        ];
        if !self.root_note.is_finite()
            || !(0.0..=127.0).contains(&self.root_note)
            || normalized
                .iter()
                .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
            || self.playback_start >= self.playback_end
            || !self.crossfade_milliseconds.is_finite()
            || self.crossfade_milliseconds < 0.0
            || self.crossfade_milliseconds > MAX_LOOP_CROSSFADE_MILLISECONDS
        {
            return Err(SampleAssetError::InvalidLandmark);
        }
        if self.loop_mode == SampleLoopMode::Forward
            && !(self.playback_start <= self.loop_start
                && self.loop_start < self.loop_end
                && self.loop_end <= self.playback_end)
        {
            return Err(SampleAssetError::InvalidLoopRange);
        }
        Ok(())
    }

    pub fn prepared_landmarks(
        &self,
        frames: usize,
        sample_rate: u32,
    ) -> Result<PreparedSampleLandmarks, SampleAssetError> {
        self.validate()?;
        if frames < 2 {
            return Err(SampleAssetError::InvalidLandmark);
        }
        let frame = |value: f32| ((frames as f64) * f64::from(value)).floor() as usize;
        let start = frame(self.playback_start).min(frames - 1);
        let end = frame(self.playback_end).clamp(start + 1, frames);
        let loop_start = frame(self.loop_start).clamp(start, end - 1);
        let loop_end = frame(self.loop_end).clamp(loop_start + 1, end);
        let requested = ((f64::from(self.crossfade_milliseconds) / 1_000.0)
            * f64::from(sample_rate))
        .round() as usize;
        let maximum = ((f64::from(MAX_LOOP_CROSSFADE_MILLISECONDS) / 1_000.0)
            * f64::from(sample_rate))
        .round() as usize;
        let crossfade_frames = requested.min(maximum).min((loop_end - loop_start) / 2);
        if requested != crossfade_frames {
            return Err(SampleAssetError::CrossfadeTooLong);
        }
        Ok(PreparedSampleLandmarks {
            start,
            end,
            loop_start,
            loop_end,
            crossfade_frames,
            loop_mode: self.loop_mode,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreparedSampleLandmarks {
    pub start: usize,
    pub end: usize,
    pub loop_start: usize,
    pub loop_end: usize,
    pub crossfade_frames: usize,
    pub loop_mode: SampleLoopMode,
}

/// Bounded control-side visualization copied from worker-owned preparation.
///
/// The summary shares only the at-most-2,048 waveform pairs. Decoded PCM and
/// callback voice state remain exclusively prepared-graph ownership.
#[derive(Clone, Debug, PartialEq)]
pub struct PreparedSampleVisualization {
    asset_id: AssetFileId,
    sample_rate: u32,
    channels: u16,
    frames: usize,
    waveform: Arc<[WaveformPair]>,
    landmarks: PreparedSampleLandmarks,
}

impl PreparedSampleVisualization {
    pub fn new(pcm: &PreparedSamplePcm, landmarks: PreparedSampleLandmarks) -> Self {
        Self {
            asset_id: pcm.asset_id().clone(),
            sample_rate: pcm.sample_rate(),
            channels: pcm.channels(),
            frames: pcm.frames(),
            waveform: Arc::clone(&pcm.waveform),
            landmarks,
        }
    }

    pub const fn asset_id(&self) -> &AssetFileId {
        &self.asset_id
    }
    pub const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    pub const fn channels(&self) -> u16 {
        self.channels
    }
    pub const fn frames(&self) -> usize {
        self.frames
    }
    pub fn waveform(&self) -> &[WaveformPair] {
        &self.waveform
    }
    pub const fn landmarks(&self) -> PreparedSampleLandmarks {
        self.landmarks
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, thiserror::Error)]
#[serde(rename_all = "camelCase")]
pub enum SampleAssetError {
    #[error("invalid library-relative identity")]
    InvalidRelativeId,
    #[error("asset is not an admitted RIFF/WAVE file")]
    UnsupportedContainer,
    #[error("WAV encoding is not admitted")]
    UnsupportedEncoding,
    #[error("WAV bit depth is not admitted")]
    UnsupportedBitDepth,
    #[error("WAV channel count is not admitted")]
    UnsupportedChannelCount,
    #[error("WAV sample rate is not admitted")]
    UnsupportedSampleRate,
    #[error("source exceeds 256 MiB")]
    SourceTooLarge,
    #[error("source exceeds 300 seconds")]
    DurationTooLong,
    #[error("decoded PCM exceeds the per-asset scalar capacity")]
    AssetScalarCapacityExceeded,
    #[error("complete graph exceeds the deduplicated Sample PCM budget")]
    GraphPcmCapacityExceeded,
    #[error("PCM contains a non-finite sample")]
    NonFinitePcm,
    #[error("decoded PCM shape is malformed")]
    MalformedPcm,
    #[error("RIFF/WAVE structure is malformed")]
    MalformedWave,
    #[error("SoundFont bank is malformed or contains no supported playable presets")]
    MalformedSoundFont,
    #[error("DX7 SysEx library has invalid framing, payload, parameter ranges, or checksum")]
    MalformedSysEx,
    #[error("model or impulse could not be prepared by its upstream processor")]
    MalformedModel,
    #[error("catalog listing is malformed")]
    MalformedCatalog,
    #[error("asset is unavailable")]
    Unavailable,
    #[error("file is not downloaded; choose Make available offline in your cloud storage app, then select it again")]
    DownloadRequired,
    #[error("file is empty (0 bytes); no audio data is available")]
    EmptyFile,
    #[error("library path escapes its configured root")]
    PathEscape,
    #[error("checked arithmetic overflow")]
    ArithmeticOverflow,
    #[error("allocation failed")]
    AllocationFailed,
    #[error("playback landmark is invalid")]
    InvalidLandmark,
    #[error("forward loop range is invalid")]
    InvalidLoopRange,
    #[error("loop crossfade exceeds the admitted maximum")]
    CrossfadeTooLong,
    #[error("operation was cancelled")]
    Cancelled,
}

#[cfg(test)]
mod tests {
    use super::{
        AssetFileId, SampleAssetError, SampleLoopMode, SamplePlaybackConfig,
        MAX_LOOP_CROSSFADE_MILLISECONDS,
    };

    fn playback() -> SamplePlaybackConfig {
        SamplePlaybackConfig {
            asset_id: AssetFileId::new("fixture.wav").unwrap(),
            root_note: 60.0,
            playback_start: 0.0,
            playback_end: 1.0,
            loop_mode: SampleLoopMode::Forward,
            loop_start: 0.25,
            loop_end: 0.75,
            crossfade_milliseconds: 0.0,
        }
    }

    #[test]
    fn playback_landmarks_are_inclusive_exclusive_and_crossfade_is_refused_not_clamped() {
        let prepared = playback().prepared_landmarks(8, 1_000).unwrap();
        assert_eq!((prepared.start, prepared.end), (0, 8));
        assert_eq!((prepared.loop_start, prepared.loop_end), (2, 6));

        let mut excessive = playback();
        excessive.crossfade_milliseconds = 3.0;
        assert_eq!(
            excessive.prepared_landmarks(8, 1_000),
            Err(SampleAssetError::CrossfadeTooLong)
        );
        excessive.loop_start = 0.8;
        excessive.loop_end = 0.7;
        assert_eq!(
            excessive.validate(),
            Err(SampleAssetError::InvalidLoopRange)
        );
    }

    #[test]
    fn maximum_crossfade_and_one_frame_neighbor_boundaries_are_exact() {
        let maximum = SamplePlaybackConfig {
            asset_id: AssetFileId::new("maximum.wav").unwrap(),
            root_note: 60.0,
            playback_start: 0.0,
            playback_end: 1.0,
            loop_mode: SampleLoopMode::Forward,
            loop_start: 0.0,
            loop_end: 1.0,
            crossfade_milliseconds: MAX_LOOP_CROSSFADE_MILLISECONDS,
        };
        let prepared = maximum.prepared_landmarks(19_200, 48_000).unwrap();
        assert_eq!(prepared.crossfade_frames, 9_600);
        assert_eq!((prepared.loop_start, prepared.loop_end), (0, 19_200));

        let one_frame = SamplePlaybackConfig {
            asset_id: AssetFileId::new("one-frame.wav").unwrap(),
            root_note: 60.0,
            playback_start: 0.0,
            playback_end: 1.0,
            loop_mode: SampleLoopMode::Forward,
            loop_start: 0.5,
            loop_end: 1.0,
            crossfade_milliseconds: 0.0,
        };
        let prepared = one_frame.prepared_landmarks(2, 48_000).unwrap();
        assert_eq!((prepared.loop_start, prepared.loop_end), (1, 2));
        assert_eq!(prepared.crossfade_frames, 0);
    }
}
