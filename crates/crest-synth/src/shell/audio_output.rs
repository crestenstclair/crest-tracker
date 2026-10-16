use core::any::Any;
use core::fmt;
use core::sync::atomic::{fence, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;

/// The render function installed in an audio-device callback.
///
/// The buffer always contains interleaved stereo `f32` frames. The negotiated
/// [`AudioDeviceConfig`] is known before this callback is created, so the
/// callback never receives or reinterprets device configuration at run time.
pub type AudioRenderCallback = Box<dyn FnMut(&mut [f32]) + Send + 'static>;

/// A bounded, allocation-free notification emitted by a running device.
pub type AudioDeviceStatusCallback = Box<dyn FnMut(AudioDeviceRuntimeError) + Send + 'static>;

/// Only explicitly shared backend clocks may correlate physical MIDI to audio.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum AudioClockDomain {
    Unknown = 0,
    /// macOS CoreAudio host time, also used by midir's CoreMIDI timestamps.
    CoreAudioHost = 1,
}

/// Device timing for the first frame of one rendered output buffer.
/// Playback time is the backend's prediction of when that frame reaches the
/// device, so input recording can follow the heard timeline without UI latency.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioFrameClock {
    frame_start: u64,
    callback_micros: u64,
    playback_micros: u64,
    sample_rate: u32,
    domain: AudioClockDomain,
}

impl AudioFrameClock {
    pub const fn new(
        frame_start: u64,
        callback_micros: u64,
        playback_micros: u64,
        sample_rate: u32,
        domain: AudioClockDomain,
    ) -> Option<Self> {
        if sample_rate == 0 || playback_micros < callback_micros {
            return None;
        }
        Some(Self {
            frame_start,
            callback_micros,
            playback_micros,
            sample_rate,
            domain,
        })
    }

    pub const fn frame_start(self) -> u64 {
        self.frame_start
    }
    pub const fn callback_micros(self) -> u64 {
        self.callback_micros
    }
    pub const fn playback_micros(self) -> u64 {
        self.playback_micros
    }
    pub const fn sample_rate(self) -> u32 {
        self.sample_rate
    }
    pub const fn domain(self) -> AudioClockDomain {
        self.domain
    }

    /// Maps a CoreMIDI host timestamp to the nearest heard audio frame.
    /// Times before stream frame zero clamp to zero. An uncorrelated clock or
    /// an unrepresentable future frame returns None; receipt time is never used.
    pub fn frame_at_midi_timestamp(self, timestamp_micros: u64) -> Option<u64> {
        if self.domain != AudioClockDomain::CoreAudioHost {
            return None;
        }
        let delta = i128::from(timestamp_micros) - i128::from(self.playback_micros);
        let scaled = delta * i128::from(self.sample_rate);
        let rounded = (scaled + if scaled < 0 { -500_000 } else { 500_000 }) / 1_000_000;
        let frame = (i128::from(self.frame_start) + rounded).max(0);
        u64::try_from(frame).ok()
    }
}

/// Allocates one stream-owned clock transport before its callback starts.
pub struct AudioFrameClockBoundary;

impl AudioFrameClockBoundary {
    pub fn create_pair() -> (AudioFrameClockWriter, AudioFrameClockReader) {
        let shared = Arc::new(AudioFrameClockFields {
            version: AtomicU64::new(0),
            frame_start: AtomicU64::new(0),
            callback_micros: AtomicU64::new(0),
            playback_micros: AtomicU64::new(0),
            sample_rate: AtomicU32::new(0),
            domain: AtomicU8::new(AudioClockDomain::Unknown as u8),
        });
        (
            AudioFrameClockWriter {
                shared: Arc::clone(&shared),
            },
            AudioFrameClockReader { shared },
        )
    }
}

struct AudioFrameClockFields {
    version: AtomicU64,
    frame_start: AtomicU64,
    callback_micros: AtomicU64,
    playback_micros: AtomicU64,
    sample_rate: AtomicU32,
    domain: AtomicU8,
}

/// Unique callback writer. Publication performs only bounded numeric stores.
pub struct AudioFrameClockWriter {
    shared: Arc<AudioFrameClockFields>,
}

impl AudioFrameClockWriter {
    pub fn publish_from_callback(&mut self, clock: AudioFrameClock) {
        self.store(Some(clock));
    }

    /// Invalid device timing must not leave an old correlation looking current.
    pub fn clear_from_callback(&mut self) {
        self.store(None);
    }

    fn store(&mut self, clock: Option<AudioFrameClock>) {
        let fields = &self.shared;
        let version = fields.version.load(Ordering::Relaxed);
        fields
            .version
            .store(version.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        fields.frame_start.store(
            clock.map_or(0, |value| value.frame_start),
            Ordering::Relaxed,
        );
        fields.callback_micros.store(
            clock.map_or(0, |value| value.callback_micros),
            Ordering::Relaxed,
        );
        fields.playback_micros.store(
            clock.map_or(0, |value| value.playback_micros),
            Ordering::Relaxed,
        );
        fields.sample_rate.store(
            clock.map_or(0, |value| value.sample_rate),
            Ordering::Relaxed,
        );
        fields.domain.store(
            clock.map_or(AudioClockDomain::Unknown, |value| value.domain) as u8,
            Ordering::Relaxed,
        );
        fields
            .version
            .store(version.wrapping_add(2), Ordering::Release);
    }
}

/// Control-owned reader; clone it only off callback. A concurrent publication
/// can produce None after bounded retries, never a partly mixed snapshot.
#[derive(Clone)]
pub struct AudioFrameClockReader {
    shared: Arc<AudioFrameClockFields>,
}

impl AudioFrameClockReader {
    pub fn read_latest_on_control(&self) -> Option<AudioFrameClock> {
        let fields = &self.shared;
        for _ in 0..3 {
            let version = fields.version.load(Ordering::Acquire);
            if version & 1 != 0 {
                continue;
            }
            let frame_start = fields.frame_start.load(Ordering::Relaxed);
            let callback_micros = fields.callback_micros.load(Ordering::Relaxed);
            let playback_micros = fields.playback_micros.load(Ordering::Relaxed);
            let sample_rate = fields.sample_rate.load(Ordering::Relaxed);
            let domain = fields.domain.load(Ordering::Relaxed);
            fence(Ordering::Acquire);
            if fields.version.load(Ordering::Relaxed) != version {
                continue;
            }
            let domain = match domain {
                0 => AudioClockDomain::Unknown,
                1 => AudioClockDomain::CoreAudioHost,
                _ => return None,
            };
            return AudioFrameClock::new(
                frame_start,
                callback_micros,
                playback_micros,
                sample_rate,
                domain,
            );
        }
        None
    }
}

/// The PCM representation selected by an output adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioSampleFormat {
    F32,
    F64,
    I8,
    I16,
    I24,
    I32,
    I64,
    U8,
    U16,
    U24,
    U32,
    U64,
}

/// The only device-channel mapping supported by the current stereo renderer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioChannelMapping {
    /// Render stereo into the first two device channels and silence the rest.
    StereoToFirstTwo,
}

/// Validated device facts that must be known before graph preparation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudioDeviceConfig {
    sample_rate: f32,
    channels: u16,
    sample_format: AudioSampleFormat,
    channel_mapping: AudioChannelMapping,
    render_capacity_frames: usize,
}

impl AudioDeviceConfig {
    pub fn new(
        sample_rate: f32,
        channels: u16,
        sample_format: AudioSampleFormat,
        render_capacity_frames: usize,
    ) -> Result<Self, AudioDeviceConfigError> {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err(AudioDeviceConfigError::InvalidSampleRate);
        }
        if channels < 2 {
            return Err(AudioDeviceConfigError::InsufficientChannels);
        }
        if render_capacity_frames == 0 {
            return Err(AudioDeviceConfigError::InvalidRenderCapacity);
        }
        Ok(Self {
            sample_rate,
            channels,
            sample_format,
            channel_mapping: AudioChannelMapping::StereoToFirstTwo,
            render_capacity_frames,
        })
    }

    pub const fn sample_rate(self) -> f32 {
        self.sample_rate
    }

    pub const fn channels(self) -> u16 {
        self.channels
    }

    pub const fn sample_format(self) -> AudioSampleFormat {
        self.sample_format
    }

    pub const fn channel_mapping(self) -> AudioChannelMapping {
        self.channel_mapping
    }

    /// Maximum number of stereo frames submitted to the prepared graph in one
    /// render operation. Larger native callbacks are split into these bounded
    /// chunks by the renderer or format adapter.
    pub const fn render_capacity_frames(self) -> usize {
        self.render_capacity_frames
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioDeviceConfigError {
    InvalidSampleRate,
    InsufficientChannels,
    InvalidRenderCapacity,
}

impl fmt::Display for AudioDeviceConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidSampleRate => "audio sample rate must be finite and positive",
            Self::InsufficientChannels => "audio output requires at least two channels",
            Self::InvalidRenderCapacity => "audio render capacity must be nonzero",
        })
    }
}

impl std::error::Error for AudioDeviceConfigError {}

/// Fixed-size device failures that may arrive after a stream has started.
///
/// The adapter maps its framework error to this enum in the device callback;
/// formatting and application behavior remain control-side work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum AudioDeviceRuntimeError {
    DeviceBusy = 1,
    DeviceChanged = 2,
    DeviceUnavailable = 3,
    HostUnavailable = 4,
    InvalidInput = 5,
    PermissionDenied = 6,
    RealtimeDenied = 7,
    ResourceExhausted = 8,
    StreamInvalidated = 9,
    UnsupportedConfig = 10,
    UnsupportedOperation = 11,
    Xrun = 12,
    Backend = 13,
    Other = 14,
}

impl AudioDeviceRuntimeError {
    pub(crate) const fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            1 => Self::DeviceBusy,
            2 => Self::DeviceChanged,
            3 => Self::DeviceUnavailable,
            4 => Self::HostUnavailable,
            5 => Self::InvalidInput,
            6 => Self::PermissionDenied,
            7 => Self::RealtimeDenied,
            8 => Self::ResourceExhausted,
            9 => Self::StreamInvalidated,
            10 => Self::UnsupportedConfig,
            11 => Self::UnsupportedOperation,
            12 => Self::Xrun,
            13 => Self::Backend,
            14 => Self::Other,
            _ => return None,
        })
    }
}

impl fmt::Display for AudioDeviceRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::DeviceBusy => "audio device became busy",
            Self::DeviceChanged => "audio device configuration changed",
            Self::DeviceUnavailable => "audio device became unavailable",
            Self::HostUnavailable => "audio host became unavailable",
            Self::InvalidInput => "audio device reported invalid input",
            Self::PermissionDenied => "audio device permission was denied",
            Self::RealtimeDenied => "audio device denied real-time operation",
            Self::ResourceExhausted => "audio device resources were exhausted",
            Self::StreamInvalidated => "audio stream was invalidated",
            Self::UnsupportedConfig => "audio device configuration became unsupported",
            Self::UnsupportedOperation => "audio device operation became unsupported",
            Self::Xrun => "audio device reported a buffer overrun or underrun",
            Self::Backend => "audio backend failed",
            Self::Other => "audio device failed",
        })
    }
}

impl std::error::Error for AudioDeviceRuntimeError {}

/// Keeps an opened device stream alive on the control side.
pub struct AudioStream {
    _handle: Box<dyn Any>,
    clock: Option<AudioFrameClockReader>,
}

impl AudioStream {
    pub fn new<Handle>(handle: Handle) -> Self
    where
        Handle: 'static,
    {
        Self {
            _handle: Box::new(handle),
            clock: None,
        }
    }

    pub fn with_clock<Handle: 'static>(handle: Handle, clock: AudioFrameClockReader) -> Self {
        Self {
            _handle: Box::new(handle),
            clock: Some(clock),
        }
    }

    /// Obtains an optional stream clock reader on the control side.
    pub fn clock_reader(&self) -> Option<AudioFrameClockReader> {
        self.clock.clone()
    }
}

impl fmt::Debug for AudioStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AudioStream")
            .finish_non_exhaustive()
    }
}

/// A failure while selecting, configuring, opening, or starting audio output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioOutputError {
    message: String,
}

impl AudioOutputError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for AudioOutputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for AudioOutputError {}

/// A selected device/configuration that has not started its callback yet.
pub trait NegotiatedAudioOutput: Sized {
    fn config(&self) -> AudioDeviceConfig;

    /// Opens and starts the already-selected device only after the application
    /// has prepared a compatible graph and renderer.
    fn start(
        self,
        render: AudioRenderCallback,
        on_runtime_error: AudioDeviceStatusCallback,
    ) -> Result<AudioStream, AudioOutputError>;
}

/// Outbound audio port with an explicit control-side negotiation phase.
pub trait AudioOutput: Sized {
    type Negotiated: NegotiatedAudioOutput;

    /// Selects and validates a device configuration without starting a stream.
    fn negotiate(self) -> Result<Self::Negotiated, AudioOutputError>;
}

#[cfg(test)]
mod tests {
    use super::{
        AudioClockDomain, AudioDeviceConfig, AudioDeviceRuntimeError, AudioFrameClock,
        AudioFrameClockBoundary, AudioOutput, AudioOutputError, AudioRenderCallback,
        AudioSampleFormat, AudioStream, NegotiatedAudioOutput,
    };
    use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn midi_timestamp_follows_playback_time_independent_of_control_receipt() {
        let clock = AudioFrameClock::new(
            48_000,
            2_000_000,
            2_010_000,
            48_000,
            AudioClockDomain::CoreAudioHost,
        )
        .unwrap();

        assert_eq!(clock.frame_at_midi_timestamp(2_010_000), Some(48_000));
        assert_eq!(clock.frame_at_midi_timestamp(2_000_000), Some(47_520));
        assert_eq!(clock.frame_at_midi_timestamp(2_020_000), Some(48_480));
        assert_eq!(clock.frame_at_midi_timestamp(0), Some(0));
        assert_eq!(clock.callback_micros(), 2_000_000);
        assert_eq!(clock.playback_micros(), 2_010_000);
    }

    #[test]
    fn midi_timestamp_rounds_to_nearest_frame_in_both_directions() {
        let clock = AudioFrameClock::new(100, 1_000, 1_000, 1_000, AudioClockDomain::CoreAudioHost)
            .unwrap();

        assert_eq!(clock.frame_at_midi_timestamp(1_499), Some(100));
        assert_eq!(clock.frame_at_midi_timestamp(1_500), Some(101));
        assert_eq!(clock.frame_at_midi_timestamp(501), Some(100));
        assert_eq!(clock.frame_at_midi_timestamp(500), Some(99));
    }

    #[test]
    fn missing_clock_domain_or_unrepresentable_frame_cannot_correlate() {
        let unknown = AudioFrameClock::new(0, 1, 1, 48_000, AudioClockDomain::Unknown).unwrap();
        assert_eq!(unknown.frame_at_midi_timestamp(1), None);
        assert!(AudioFrameClock::new(0, 1, 1, 0, AudioClockDomain::CoreAudioHost).is_none());
        assert!(AudioFrameClock::new(0, 2, 1, 48_000, AudioClockDomain::CoreAudioHost).is_none());

        let last_frame =
            AudioFrameClock::new(u64::MAX, 0, 0, 48_000, AudioClockDomain::CoreAudioHost).unwrap();
        assert_eq!(last_frame.frame_at_midi_timestamp(0), Some(u64::MAX));
        assert_eq!(last_frame.frame_at_midi_timestamp(1_000_000), None);
        assert_eq!(last_frame.frame_at_midi_timestamp(u64::MAX), None);
    }

    #[test]
    fn stream_readers_share_publications_and_invalidations() {
        let (mut writer, reader) = AudioFrameClockBoundary::create_pair();
        assert_eq!(reader.read_latest_on_control(), None);
        let stream = AudioStream::with_clock((), reader);
        let first_reader = stream.clock_reader().unwrap();
        let second_reader = stream.clock_reader().unwrap();
        let clock =
            AudioFrameClock::new(128, 100, 200, 48_000, AudioClockDomain::CoreAudioHost).unwrap();

        writer.publish_from_callback(clock);
        assert_eq!(first_reader.read_latest_on_control(), Some(clock));
        assert_eq!(second_reader.read_latest_on_control(), Some(clock));
        writer.clear_from_callback();
        assert_eq!(first_reader.read_latest_on_control(), None);
        assert_eq!(second_reader.read_latest_on_control(), None);
        assert!(AudioStream::new(()).clock_reader().is_none());
    }

    #[test]
    fn concurrent_publication_never_mixes_audio_buffers() {
        let (mut writer, reader) = AudioFrameClockBoundary::create_pair();
        let done = Arc::new(AtomicBool::new(false));
        let writer_done = Arc::clone(&done);
        let producer = std::thread::spawn(move || {
            for index in 1..=50_000_u64 {
                let clock = AudioFrameClock::new(
                    index * 128,
                    index * 1_000,
                    index * 1_000 + 400,
                    if index % 2 == 0 { 48_000 } else { 44_100 },
                    AudioClockDomain::CoreAudioHost,
                )
                .unwrap();
                writer.publish_from_callback(clock);
            }
            writer_done.store(true, Ordering::Release);
        });
        while !done.load(Ordering::Acquire) {
            if let Some(clock) = reader.read_latest_on_control() {
                let index = clock.frame_start() / 128;
                assert_eq!(clock.frame_start(), index * 128);
                assert_eq!(clock.callback_micros(), index * 1_000);
                assert_eq!(clock.playback_micros(), index * 1_000 + 400);
                assert_eq!(
                    clock.sample_rate(),
                    if index % 2 == 0 { 48_000 } else { 44_100 }
                );
                assert_eq!(clock.domain(), AudioClockDomain::CoreAudioHost);
            }
        }
        producer.join().unwrap();
        assert_eq!(
            reader.read_latest_on_control().unwrap().frame_start(),
            50_000 * 128
        );
    }

    struct TestAudioOutput {
        negotiated: Arc<AtomicBool>,
        buffer_address: Arc<AtomicUsize>,
    }

    struct TestNegotiatedOutput {
        config: AudioDeviceConfig,
        negotiated: Arc<AtomicBool>,
        buffer_address: Arc<AtomicUsize>,
    }

    impl AudioOutput for TestAudioOutput {
        type Negotiated = TestNegotiatedOutput;

        fn negotiate(self) -> Result<Self::Negotiated, AudioOutputError> {
            self.negotiated.store(true, Ordering::Release);
            Ok(TestNegotiatedOutput {
                config: AudioDeviceConfig::new(44_100.0, 2, AudioSampleFormat::F32, 2).unwrap(),
                negotiated: self.negotiated,
                buffer_address: self.buffer_address,
            })
        }
    }

    impl NegotiatedAudioOutput for TestNegotiatedOutput {
        fn config(&self) -> AudioDeviceConfig {
            self.config
        }

        fn start(
            self,
            mut render: AudioRenderCallback,
            mut on_runtime_error: super::AudioDeviceStatusCallback,
        ) -> Result<AudioStream, AudioOutputError> {
            assert!(self.negotiated.load(Ordering::Acquire));
            let mut device_buffer = [0.0; 4];
            self.buffer_address
                .store(device_buffer.as_mut_ptr() as usize, Ordering::Release);
            render(&mut device_buffer);
            on_runtime_error(AudioDeviceRuntimeError::Xrun);
            Ok(AudioStream::new(()))
        }
    }

    #[test]
    fn negotiation_precedes_start_and_forwards_the_device_buffer() {
        let negotiated = Arc::new(AtomicBool::new(false));
        let expected_address = Arc::new(AtomicUsize::new(0));
        let observed_address = Arc::new(AtomicUsize::new(0));
        let runtime_error_seen = Arc::new(AtomicBool::new(false));

        let output = TestAudioOutput {
            negotiated: Arc::clone(&negotiated),
            buffer_address: Arc::clone(&expected_address),
        };
        let selected = output.negotiate().unwrap();
        assert_eq!(selected.config().sample_rate(), 44_100.0);

        let callback_address = Arc::clone(&observed_address);
        let render: AudioRenderCallback = Box::new(move |buffer| {
            callback_address.store(buffer.as_mut_ptr() as usize, Ordering::Release);
        });
        let runtime_error_seen_callback = Arc::clone(&runtime_error_seen);
        selected
            .start(
                render,
                Box::new(move |error| {
                    assert_eq!(error, AudioDeviceRuntimeError::Xrun);
                    runtime_error_seen_callback.store(true, Ordering::Release);
                }),
            )
            .unwrap();

        assert_eq!(
            observed_address.load(Ordering::Acquire),
            expected_address.load(Ordering::Acquire)
        );
        assert!(runtime_error_seen.load(Ordering::Acquire));
    }

    #[test]
    fn device_configuration_rejects_unsupported_values() {
        assert!(AudioDeviceConfig::new(0.0, 2, AudioSampleFormat::F32, 64).is_err());
        assert!(AudioDeviceConfig::new(48_000.0, 1, AudioSampleFormat::F32, 64).is_err());
        assert!(AudioDeviceConfig::new(48_000.0, 2, AudioSampleFormat::F32, 0).is_err());
    }

    #[test]
    fn setup_error_preserves_its_actionable_message() {
        let error = AudioOutputError::new("default output device is unavailable");
        assert_eq!(error.message(), "default output device is unavailable");
        assert_eq!(error.to_string(), "default output device is unavailable");
    }
}
