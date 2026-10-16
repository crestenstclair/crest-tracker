use crate::real_time::callback_safety::CallbackSafetyScope;
use crate::shell::audio_output::{
    AudioClockDomain, AudioDeviceConfig, AudioDeviceRuntimeError, AudioDeviceStatusCallback,
    AudioFrameClock, AudioFrameClockBoundary, AudioFrameClockWriter, AudioOutput, AudioOutputError,
    AudioRenderCallback, AudioSampleFormat, AudioStream, NegotiatedAudioOutput,
};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    BufferSize, Device, ErrorKind, FromSample, SampleFormat, SizedSample, Stream, StreamConfig,
    SupportedBufferSize, SupportedStreamConfig, SupportedStreamConfigRange, I24, U24,
};

const STEREO_SCRATCH_SAMPLES: usize = 2_048;
const PREFERRED_SAMPLE_RATE: u32 = 48_000;
const PREFERRED_CALLBACK_FRAMES: u32 = 256;
const UNKNOWN_CALLBACK_RENDER_CAPACITY: usize = STEREO_SCRATCH_SAMPLES / 2;

/// Physical low-latency stereo output backed by the system's default CPAL host.
#[derive(Clone, Copy, Debug, Default)]
pub struct CpalAudioOutput;

impl CpalAudioOutput {
    pub const fn new() -> Self {
        Self
    }
}

/// A selected CPAL device whose callback has not started yet.
pub struct CpalNegotiatedAudioOutput {
    device: Device,
    sample_format: SampleFormat,
    stream_config: StreamConfig,
    config: AudioDeviceConfig,
    clock_domain: AudioClockDomain,
}

impl AudioOutput for CpalAudioOutput {
    type Negotiated = CpalNegotiatedAudioOutput;

    fn negotiate(self) -> Result<Self::Negotiated, AudioOutputError> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| AudioOutputError::new("default output device is unavailable"))?;
        let supported = select_output_config(&device)?;
        let sample_format = supported.sample_format();
        let requested_frames = std::env::var("CREST_AUDIO_BUFFER_FRAMES")
            .map(Some)
            .or_else(|error| match error {
                std::env::VarError::NotPresent => Ok(None),
                error => Err(AudioOutputError::new(format!(
                    "invalid CREST_AUDIO_BUFFER_FRAMES: {error}"
                ))),
            })?;
        let requested_frames = parse_buffer_frames(requested_frames.as_deref())?;
        let (stream_config, render_capacity_frames) =
            configure_buffer_size(&supported, requested_frames)?;
        let canonical_format = canonical_sample_format(sample_format).ok_or_else(|| {
            AudioOutputError::new(format!(
                "unsupported default output sample format: {sample_format}"
            ))
        })?;
        let config = AudioDeviceConfig::new(
            stream_config.sample_rate as f32,
            stream_config.channels,
            canonical_format,
            render_capacity_frames,
        )
        .map_err(|error| AudioOutputError::new(error.to_string()))?;

        Ok(CpalNegotiatedAudioOutput {
            device,
            sample_format,
            stream_config,
            config,
            clock_domain: audio_clock_domain(&host),
        })
    }
}

impl NegotiatedAudioOutput for CpalNegotiatedAudioOutput {
    fn config(&self) -> AudioDeviceConfig {
        self.config
    }

    fn start(
        self,
        render: AudioRenderCallback,
        on_runtime_error: AudioDeviceStatusCallback,
    ) -> Result<AudioStream, AudioOutputError> {
        let (clock_writer, clock_reader) = AudioFrameClockBoundary::create_pair();
        let clock = CpalFrameClock {
            writer: clock_writer,
            domain: self.clock_domain,
            sample_rate: self.stream_config.sample_rate,
            frame_start: 0,
        };
        let stream = build_stream(
            &self.device,
            self.sample_format,
            self.stream_config,
            render,
            on_runtime_error,
            clock,
        )?;
        stream.play().map_err(|error| {
            AudioOutputError::new(format!(
                "failed to start the default PCM output stream: {error}"
            ))
        })?;
        Ok(AudioStream::with_clock(stream, clock_reader))
    }
}

fn audio_clock_domain(host: &cpal::Host) -> AudioClockDomain {
    #[cfg(target_os = "macos")]
    if host.id() == cpal::HostId::CoreAudio {
        return AudioClockDomain::CoreAudioHost;
    }
    let _ = host;
    AudioClockDomain::Unknown
}

/// The adapter and renderer start at the same frame zero and account for the
/// same complete device frames, even when format conversion splits a buffer.
struct CpalFrameClock {
    writer: AudioFrameClockWriter,
    domain: AudioClockDomain,
    sample_rate: u32,
    frame_start: u64,
}

impl CpalFrameClock {
    fn publish_buffer(&mut self, timestamp: cpal::OutputStreamTimestamp, frame_count: usize) {
        let clock = u64::try_from(timestamp.callback.as_nanos() / 1_000)
            .ok()
            .zip(u64::try_from(timestamp.playback.as_nanos() / 1_000).ok())
            .and_then(|(callback, playback)| {
                AudioFrameClock::new(
                    self.frame_start,
                    callback,
                    playback,
                    self.sample_rate,
                    self.domain,
                )
            });
        if let Some(clock) = clock {
            self.writer.publish_from_callback(clock);
        } else {
            self.writer.clear_from_callback();
        }
        self.frame_start = self.frame_start.saturating_add(frame_count as u64);
    }
}

fn select_output_config(device: &Device) -> Result<SupportedStreamConfig, AudioOutputError> {
    let default_config = device
        .default_output_config()
        .map_err(|error| error.to_string());
    choose_output_config(default_config, || {
        device
            .supported_output_configs()
            .map(|ranges| ranges.filter(is_usable_range).collect::<Vec<_>>())
            .map_err(|error| error.to_string())
    })
}

fn choose_output_config<QueryRanges>(
    default_result: Result<SupportedStreamConfig, String>,
    query_ranges: QueryRanges,
) -> Result<SupportedStreamConfig, AudioOutputError>
where
    QueryRanges: FnOnce() -> Result<Vec<SupportedStreamConfigRange>, String>,
{
    let (default_config, default_error) = match default_result {
        Ok(config) => (Some(config), None),
        Err(error) => (None, Some(error)),
    };

    if let Some(config) = default_config.filter(is_preferred_config) {
        return Ok(config);
    }

    let pcm_ranges = match query_ranges() {
        Ok(ranges) => ranges,
        Err(range_error) => {
            if let Some(config) = default_config.filter(is_usable_config) {
                return Ok(config);
            }
            let message = match default_error {
                Some(default_error) => format!(
                    "failed to query the default output configuration: {default_error}; failed to query PCM output configurations: {range_error}"
                ),
                None => format!("failed to query PCM output configurations: {range_error}"),
            };
            return Err(AudioOutputError::new(message));
        }
    };

    if let Some(preferred) = pcm_ranges
        .iter()
        .copied()
        .filter(|range| range.contains_rate(PREFERRED_SAMPLE_RATE))
        .max_by(|left, right| left.cmp_default_heuristics(right))
        .and_then(|range| range.try_with_sample_rate(PREFERRED_SAMPLE_RATE))
    {
        return Ok(preferred);
    }

    if let Some(config) = default_config.filter(is_usable_config) {
        return Ok(config);
    }

    pcm_ranges
        .into_iter()
        .max_by(|left, right| left.cmp_default_heuristics(right))
        .map(|range| {
            range
                .try_with_standard_sample_rate()
                .unwrap_or_else(|| range.with_max_sample_rate())
        })
        .ok_or_else(|| {
            let message = default_error.map_or_else(
                || "default output device has no PCM configuration with two channels".to_owned(),
                |error| {
                    format!(
                        "failed to query the default output configuration: {error}; default output device has no PCM configuration with two channels"
                    )
                },
            );
            AudioOutputError::new(message)
        })
}

fn is_usable_config(config: &SupportedStreamConfig) -> bool {
    config.channels() >= 2 && !config.sample_format().is_dsd()
}

fn is_preferred_config(config: &SupportedStreamConfig) -> bool {
    is_usable_config(config) && config.sample_rate() == PREFERRED_SAMPLE_RATE
}

fn is_usable_range(config: &SupportedStreamConfigRange) -> bool {
    config.channels() >= 2 && !config.sample_format().is_dsd()
}

fn parse_buffer_frames(value: Option<&str>) -> Result<Option<u32>, AudioOutputError> {
    value
        .map(|value| {
            value
                .parse::<u32>()
                .ok()
                .filter(|frames| *frames > 0)
                .ok_or_else(|| {
                    AudioOutputError::new(
                        "CREST_AUDIO_BUFFER_FRAMES must be a positive frame count",
                    )
                })
        })
        .transpose()
}

fn configure_buffer_size(
    supported: &SupportedStreamConfig,
    requested_frames: Option<u32>,
) -> Result<(StreamConfig, usize), AudioOutputError> {
    let mut config = supported.config();
    if let Some(frames) = requested_frames {
        if let SupportedBufferSize::Range { min, max } = *supported.buffer_size() {
            if !(min..=max).contains(&frames) {
                return Err(AudioOutputError::new(format!(
                    "requested audio buffer of {frames} frames is outside the device range {min}..={max}"
                )));
            }
        }
        config.buffer_size = BufferSize::Fixed(frames);
        return Ok((config, frames as usize));
    }
    Ok(match *supported.buffer_size() {
        SupportedBufferSize::Range { min, max } => {
            let frames = PREFERRED_CALLBACK_FRAMES.clamp(min, max);
            config.buffer_size = BufferSize::Fixed(frames);
            (config, frames as usize)
        }
        SupportedBufferSize::Unknown => (config, UNKNOWN_CALLBACK_RENDER_CAPACITY),
    })
}

fn canonical_sample_format(sample_format: SampleFormat) -> Option<AudioSampleFormat> {
    Some(match sample_format {
        SampleFormat::F32 => AudioSampleFormat::F32,
        SampleFormat::F64 => AudioSampleFormat::F64,
        SampleFormat::I8 => AudioSampleFormat::I8,
        SampleFormat::I16 => AudioSampleFormat::I16,
        SampleFormat::I24 => AudioSampleFormat::I24,
        SampleFormat::I32 => AudioSampleFormat::I32,
        SampleFormat::I64 => AudioSampleFormat::I64,
        SampleFormat::U8 => AudioSampleFormat::U8,
        SampleFormat::U16 => AudioSampleFormat::U16,
        SampleFormat::U24 => AudioSampleFormat::U24,
        SampleFormat::U32 => AudioSampleFormat::U32,
        SampleFormat::U64 => AudioSampleFormat::U64,
        _ => return None,
    })
}

fn build_stream(
    device: &Device,
    sample_format: SampleFormat,
    config: StreamConfig,
    render: AudioRenderCallback,
    on_runtime_error: AudioDeviceStatusCallback,
    clock: CpalFrameClock,
) -> Result<Stream, AudioOutputError> {
    let channels = usize::from(config.channels);
    match sample_format {
        SampleFormat::F32 if channels == 2 => {
            build_native_stereo_stream(device, config, render, on_runtime_error, clock)
        }
        SampleFormat::F32 => {
            build_mapped_stream::<f32>(device, config, render, on_runtime_error, clock)
        }
        SampleFormat::F64 => {
            build_mapped_stream::<f64>(device, config, render, on_runtime_error, clock)
        }
        SampleFormat::I8 => {
            build_mapped_stream::<i8>(device, config, render, on_runtime_error, clock)
        }
        SampleFormat::I16 => {
            build_mapped_stream::<i16>(device, config, render, on_runtime_error, clock)
        }
        SampleFormat::I24 => {
            build_mapped_stream::<I24>(device, config, render, on_runtime_error, clock)
        }
        SampleFormat::I32 => {
            build_mapped_stream::<i32>(device, config, render, on_runtime_error, clock)
        }
        SampleFormat::I64 => {
            build_mapped_stream::<i64>(device, config, render, on_runtime_error, clock)
        }
        SampleFormat::U8 => {
            build_mapped_stream::<u8>(device, config, render, on_runtime_error, clock)
        }
        SampleFormat::U16 => {
            build_mapped_stream::<u16>(device, config, render, on_runtime_error, clock)
        }
        SampleFormat::U24 => {
            build_mapped_stream::<U24>(device, config, render, on_runtime_error, clock)
        }
        SampleFormat::U32 => {
            build_mapped_stream::<u32>(device, config, render, on_runtime_error, clock)
        }
        SampleFormat::U64 => {
            build_mapped_stream::<u64>(device, config, render, on_runtime_error, clock)
        }
        unsupported => Err(AudioOutputError::new(format!(
            "unsupported default output sample format: {unsupported}"
        ))),
    }
}

fn build_native_stereo_stream(
    device: &Device,
    config: StreamConfig,
    mut render: AudioRenderCallback,
    mut on_runtime_error: AudioDeviceStatusCallback,
    mut clock: CpalFrameClock,
) -> Result<Stream, AudioOutputError> {
    device
        .build_output_stream(
            config,
            move |device_buffer: &mut [f32], info| {
                let _callback_scope = CallbackSafetyScope::enter();
                render(device_buffer);
                clock.publish_buffer(info.timestamp(), device_buffer.len() / 2);
            },
            move |error| {
                let _callback_scope = CallbackSafetyScope::enter();
                on_runtime_error(map_runtime_error(error.kind()));
            },
            None,
        )
        .map_err(|error| {
            AudioOutputError::new(format!(
                "failed to open the default native stereo f32 output stream: {error}"
            ))
        })
}

fn build_mapped_stream<Sample>(
    device: &Device,
    config: StreamConfig,
    mut render: AudioRenderCallback,
    mut on_runtime_error: AudioDeviceStatusCallback,
    mut clock: CpalFrameClock,
) -> Result<Stream, AudioOutputError>
where
    Sample: SizedSample + FromSample<f32> + Send + 'static,
{
    let channels = usize::from(config.channels);
    let device_chunk_samples = (STEREO_SCRATCH_SAMPLES / 2) * channels;
    let silence = Sample::from_sample(0.0);
    let mut stereo_buffer = [0.0; STEREO_SCRATCH_SAMPLES];

    device
        .build_output_stream(
            config,
            move |device_buffer: &mut [Sample], info| {
                let _callback_scope = CallbackSafetyScope::enter();
                for device_chunk in device_buffer.chunks_mut(device_chunk_samples) {
                    let frame_count = device_chunk.len() / channels;
                    let stereo_sample_count = frame_count * 2;
                    let render_buffer = &mut stereo_buffer[..stereo_sample_count];
                    render(render_buffer);
                    map_stereo_samples(render_buffer, device_chunk, channels, silence);
                }
                clock.publish_buffer(info.timestamp(), device_buffer.len() / channels);
            },
            move |error| {
                let _callback_scope = CallbackSafetyScope::enter();
                on_runtime_error(map_runtime_error(error.kind()));
            },
            None,
        )
        .map_err(|error| {
            AudioOutputError::new(format!(
                "failed to open the default mapped PCM output stream: {error}"
            ))
        })
}

const fn map_runtime_error(kind: ErrorKind) -> AudioDeviceRuntimeError {
    match kind {
        ErrorKind::DeviceBusy => AudioDeviceRuntimeError::DeviceBusy,
        ErrorKind::DeviceChanged => AudioDeviceRuntimeError::DeviceChanged,
        ErrorKind::DeviceNotAvailable => AudioDeviceRuntimeError::DeviceUnavailable,
        ErrorKind::HostUnavailable => AudioDeviceRuntimeError::HostUnavailable,
        ErrorKind::InvalidInput => AudioDeviceRuntimeError::InvalidInput,
        ErrorKind::PermissionDenied => AudioDeviceRuntimeError::PermissionDenied,
        ErrorKind::RealtimeDenied => AudioDeviceRuntimeError::RealtimeDenied,
        ErrorKind::ResourceExhausted => AudioDeviceRuntimeError::ResourceExhausted,
        ErrorKind::StreamInvalidated => AudioDeviceRuntimeError::StreamInvalidated,
        ErrorKind::UnsupportedConfig => AudioDeviceRuntimeError::UnsupportedConfig,
        ErrorKind::UnsupportedOperation => AudioDeviceRuntimeError::UnsupportedOperation,
        ErrorKind::Xrun => AudioDeviceRuntimeError::Xrun,
        ErrorKind::BackendError => AudioDeviceRuntimeError::Backend,
        ErrorKind::Other => AudioDeviceRuntimeError::Other,
        _ => AudioDeviceRuntimeError::Other,
    }
}

fn map_stereo_samples<Sample>(
    source: &[f32],
    destination: &mut [Sample],
    channels: usize,
    silence: Sample,
) where
    Sample: cpal::Sample + FromSample<f32>,
{
    destination.fill(silence);
    for (source_frame, destination_frame) in source
        .chunks_exact(2)
        .zip(destination.chunks_exact_mut(channels))
    {
        destination_frame[0] = Sample::from_sample(bound_sample(source_frame[0]));
        destination_frame[1] = Sample::from_sample(bound_sample(source_frame[1]));
    }
}

fn bound_sample(sample: f32) -> f32 {
    if sample.is_nan() {
        0.0
    } else {
        sample.clamp(-1.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        bound_sample, canonical_sample_format, choose_output_config, configure_buffer_size,
        map_runtime_error, map_stereo_samples, parse_buffer_frames, CpalAudioOutput,
        CpalFrameClock, STEREO_SCRATCH_SAMPLES,
    };
    use crate::shell::audio_output::{
        AudioClockDomain, AudioDeviceRuntimeError, AudioFrameClockBoundary, AudioOutput,
        AudioSampleFormat,
    };
    use cpal::{
        ErrorKind, OutputStreamTimestamp, SampleFormat, StreamInstant, SupportedBufferSize,
        SupportedStreamConfig,
    };
    use std::cell::Cell;

    fn timestamp(callback_micros: u64, playback_micros: u64) -> OutputStreamTimestamp {
        OutputStreamTimestamp {
            callback: StreamInstant::from_micros(callback_micros),
            playback: StreamInstant::from_micros(playback_micros),
        }
    }

    #[test]
    fn frame_clock_counts_whole_device_buffers_across_conversion_chunks() {
        let (writer, reader) = AudioFrameClockBoundary::create_pair();
        let mut clock = CpalFrameClock {
            writer,
            domain: AudioClockDomain::CoreAudioHost,
            sample_rate: 48_000,
            frame_start: 0,
        };
        clock.publish_buffer(timestamp(1_000_000, 1_010_000), 128);
        let first = reader.read_latest_on_control().unwrap();
        assert_eq!(first.frame_start(), 0);
        assert_eq!(first.callback_micros(), 1_000_000);
        assert_eq!(first.playback_micros(), 1_010_000);
        assert_eq!(first.sample_rate(), 48_000);

        // A large mapped device buffer can require several scratch render calls.
        // It still has only one backend timestamp, marking its first frame.
        let large_buffer_frames = STEREO_SCRATCH_SAMPLES / 2 * 3 + 7;
        clock.publish_buffer(timestamp(1_002_666, 1_012_666), large_buffer_frames);
        assert_eq!(reader.read_latest_on_control().unwrap().frame_start(), 128);
        clock.publish_buffer(timestamp(2_000_000, 2_010_000), 256);
        assert_eq!(
            reader.read_latest_on_control().unwrap().frame_start(),
            128 + large_buffer_frames as u64,
        );
    }

    #[test]
    fn invalid_device_timestamp_clears_correlation_without_resetting_frame_count() {
        let (writer, reader) = AudioFrameClockBoundary::create_pair();
        let mut clock = CpalFrameClock {
            writer,
            domain: AudioClockDomain::CoreAudioHost,
            sample_rate: 48_000,
            frame_start: 0,
        };
        clock.publish_buffer(timestamp(1_000, 2_000), 128);
        assert!(reader.read_latest_on_control().is_some());
        clock.publish_buffer(timestamp(3_000, 2_000), 256);
        assert_eq!(reader.read_latest_on_control(), None);
        clock.publish_buffer(timestamp(4_000, 5_000), 128);
        let recovered = reader.read_latest_on_control().unwrap();
        assert_eq!(recovered.frame_start(), 384);
        assert_eq!(recovered.frame_at_midi_timestamp(5_000), Some(384));
    }

    fn config(sample_rate: u32) -> SupportedStreamConfig {
        SupportedStreamConfig::new(
            2,
            sample_rate,
            SupportedBufferSize::Range { min: 128, max: 512 },
            SampleFormat::F32,
        )
    }

    #[test]
    fn requested_buffer_is_validated_and_sizes_graph_preparation() {
        assert_eq!(parse_buffer_frames(None).unwrap(), None);
        assert_eq!(parse_buffer_frames(Some("512")).unwrap(), Some(512));
        for value in ["0", "-1", "abc", "", "4294967296"] {
            assert!(parse_buffer_frames(Some(value)).is_err());
        }
        let supported = config(48_000);
        let (stream, capacity) = configure_buffer_size(&supported, Some(512)).unwrap();
        assert_eq!(stream.buffer_size, cpal::BufferSize::Fixed(512));
        assert_eq!(capacity, 512);
        assert!(configure_buffer_size(&supported, Some(1024)).is_err());
        let (stream, capacity) = configure_buffer_size(&supported, None).unwrap();
        assert_eq!(stream.buffer_size, cpal::BufferSize::Fixed(256));
        assert_eq!(capacity, 256);
    }

    #[test]
    fn adapter_implements_the_audio_output_port() {
        fn assert_audio_output<Output: AudioOutput>() {}
        assert_audio_output::<CpalAudioOutput>();
        let _output = CpalAudioOutput::new();
    }

    #[test]
    fn preferred_default_does_not_require_optional_range_enumeration() {
        let queried = Cell::new(false);
        let selected = choose_output_config(Ok(config(48_000)), || {
            queried.set(true);
            Err("optional ranges unavailable".to_owned())
        })
        .expect("the valid preferred default is sufficient");

        assert_eq!(selected.sample_rate(), 48_000);
        assert!(!queried.get());
    }

    #[test]
    fn valid_default_survives_optional_range_query_failure() {
        let selected =
            choose_output_config(Ok(config(44_100)), || Err("Unknown property".to_owned()))
                .expect("a valid reported default remains usable");

        assert_eq!(selected.sample_rate(), 44_100);
        assert_eq!(selected.channels(), 2);
    }

    #[test]
    fn failed_default_and_range_queries_remain_one_actionable_error() {
        let error = choose_output_config(Err("default failed".to_owned()), || {
            Err("ranges failed".to_owned())
        })
        .expect_err("no device configuration was reported");

        assert!(error.message().contains("default failed"));
        assert!(error.message().contains("ranges failed"));
    }

    #[test]
    fn every_supported_pcm_format_has_a_canonical_configuration_value() {
        assert_eq!(
            canonical_sample_format(SampleFormat::F32),
            Some(AudioSampleFormat::F32)
        );
        assert_eq!(
            canonical_sample_format(SampleFormat::I16),
            Some(AudioSampleFormat::I16)
        );
        assert_eq!(
            canonical_sample_format(SampleFormat::U24),
            Some(AudioSampleFormat::U24)
        );
    }

    #[test]
    fn cpal_runtime_errors_map_without_formatting_or_allocation() {
        assert_eq!(
            map_runtime_error(ErrorKind::DeviceNotAvailable),
            AudioDeviceRuntimeError::DeviceUnavailable
        );
        assert_eq!(
            map_runtime_error(ErrorKind::Xrun),
            AudioDeviceRuntimeError::Xrun
        );
    }

    #[test]
    fn non_f32_conversion_is_finite_and_bounded() {
        let source = [f32::NEG_INFINITY, -1.0, 0.0, 1.0, f32::INFINITY, f32::NAN];
        let mut destination = [0_i16; 6];
        map_stereo_samples(&source, &mut destination, 2, 0);
        assert_eq!(destination[0], destination[1]);
        assert!(destination[1] < destination[2]);
        assert_eq!(destination[2], 0);
        assert!(destination[2] < destination[3]);
        assert_eq!(destination[3], destination[4]);
        assert_eq!(destination[5], 0);
    }

    #[test]
    fn stereo_samples_fill_first_two_channels_and_silence_surplus_channels() {
        let source = [0.25, -0.5, 0.75, -1.0];
        let mut destination = [9.0_f32; 9];
        map_stereo_samples(&source, &mut destination, 4, 0.0);
        assert_eq!(
            destination,
            [0.25, -0.5, 0.0, 0.0, 0.75, -1.0, 0.0, 0.0, 0.0]
        );
    }

    #[test]
    fn conversion_chunks_are_bounded_stereo_sample_storage() {
        const { assert!(STEREO_SCRATCH_SAMPLES > 0) };
        assert_eq!(STEREO_SCRATCH_SAMPLES % 2, 0);
        assert_eq!(bound_sample(f32::NAN), 0.0);
        assert_eq!(bound_sample(-2.0), -1.0);
        assert_eq!(bound_sample(2.0), 1.0);
    }
}
