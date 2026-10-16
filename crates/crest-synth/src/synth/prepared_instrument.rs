use crate::kernel::midi_message::{MidiMessage, MidiMessageKind};
use crate::kernel::patch_id::PatchId;
use crate::real_time::parameter_snapshot::RtPatchParameters;
use crate::synth::{AssetReference, PreparedSampleVisualization};
use core::fmt;

/// Independent ownership for generic MIDI delivery lanes.
///
/// This is runtime routing state, not Patch content or a musical track ID.
/// All lanes share the instrument's existing prepared voice capacity.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub enum MidiSource {
    Immediate,
    Scheduled,
    Monitor,
}

impl MidiSource {
    pub const COUNT: usize = 3;
    pub const ALL: [Self; Self::COUNT] = [Self::Immediate, Self::Scheduled, Self::Monitor];

    pub const fn index(self) -> usize {
        match self {
            Self::Immediate => 0,
            Self::Scheduled => 1,
            Self::Monitor => 2,
        }
    }
}

/// Host articulation identity, independent of MIDI channel/pitch and musical track concepts.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct MidiVoiceId(core::num::NonZeroU64);
impl MidiVoiceId {
    pub const fn new(value: u64) -> Option<Self> {
        match core::num::NonZeroU64::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

/// Independent expression dimensions for one host-owned articulation. A master
/// dimension never broadcasts to unrelated voices or knows a sequencer track.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub enum MidiExpressionScope {
    #[default]
    Member,
    Master,
}

/// Normalized initial state of one expression dimension. Absence of pressure
/// means neutral gain, never inheritance from a channel's previous attack.
/// Timbre is an absolute CC74 value; only the member dimension may supply it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnedExpressionState {
    raw14: u16,
    semitones: u8,
    cents: u8,
    pressure: Option<u8>,
    timbre: Option<u8>,
    pedal: u8,
}

impl OwnedExpressionState {
    /// Validates MIDI normalization and RPN0 units, independently of an
    /// instrument's pitch interval. The instrument admits the combined pitch.
    pub fn new(
        raw14: u16,
        semitones: u8,
        cents: u8,
        pressure: Option<u8>,
        timbre: Option<u8>,
        pedal: u8,
    ) -> Result<Self, PreparedInstrumentError> {
        if raw14 > 16_383
            || semitones > 127
            || cents > 99
            || pedal > 127
            || pressure.is_some_and(|value| value > 127)
            || timbre.is_some_and(|value| value > 127)
        {
            return Err(PreparedInstrumentError::DispatchRejected);
        }
        Ok(Self {
            raw14,
            semitones,
            cents,
            pressure,
            timbre,
            pedal,
        })
    }

    pub const fn raw14(self) -> u16 {
        self.raw14
    }
    pub const fn semitones(self) -> u8 {
        self.semitones
    }
    pub const fn cents(self) -> u8 {
        self.cents
    }
    pub const fn pressure(self) -> Option<u8> {
        self.pressure
    }
    pub const fn timbre(self) -> Option<u8> {
        self.timbre
    }
    pub const fn pedal(self) -> u8 {
        self.pedal
    }
}

impl Default for OwnedExpressionState {
    fn default() -> Self {
        Self {
            raw14: 8_192,
            semitones: 2,
            cents: 0,
            pressure: None,
            timbre: None,
            pedal: 0,
        }
    }
}

/// One indivisible attack and its independent initial expression dimensions.
/// A queue consumes this complete value as one work item; unsupported engines
/// reject it without first sounding a partially initialized attack.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnedVoiceInitialization {
    message: MidiMessage,
    member: OwnedExpressionState,
    master: OwnedExpressionState,
}

impl OwnedVoiceInitialization {
    pub fn new(
        message: MidiMessage,
        member: OwnedExpressionState,
        master: OwnedExpressionState,
    ) -> Result<Self, PreparedInstrumentError> {
        if message.kind() != MidiMessageKind::NoteOn || message.data2() == 0 {
            return Err(PreparedInstrumentError::DispatchRejected);
        }
        if master.timbre().is_some() {
            return Err(PreparedInstrumentError::UnsupportedExpressionScope);
        }
        Ok(Self {
            message,
            member,
            master,
        })
    }

    pub const fn message(self) -> MidiMessage {
        self.message
    }
    pub const fn member(self) -> OwnedExpressionState {
        self.member
    }
    pub const fn master(self) -> OwnedExpressionState {
        self.master
    }
}

/// One fully prepared Patch-specific synthesis runtime.
///
/// Implementations own all voice state and scratch needed by one Patch. Note
/// admission, dispatch, render, and silence run on the hard real-time path with
/// bounded, preallocated work: no allocation, destruction, locking, blocking,
/// I/O, logging, formatting, panic, or unwind. Asset metadata methods below are
/// explicitly worker-only.
pub trait PreparedInstrument: Send {
    /// Returns the immutable Patch identity prepared into this instrument.
    fn patch_id(&self) -> PatchId;

    /// Static prepared note mapping; unmapped/empty pads do not consume note admission.
    fn accepts_note(&self, _note: u8) -> bool {
        true
    }

    /// Worker-side collection for instruments composed of several resident assets.
    fn prepared_asset_footprints(&self) -> Vec<&PreparedAssetFootprint> {
        self.prepared_asset_footprint().into_iter().collect()
    }

    /// Worker-side summaries; never called by the callback.
    fn prepared_sample_visualizations(&self) -> Vec<PreparedSampleVisualization> {
        self.prepared_sample_visualization()
            .cloned()
            .into_iter()
            .collect()
    }

    /// Delivers one normalized MIDI message to this instrument only.
    fn dispatch(
        &mut self,
        message: MidiMessage,
        parameters: &RtPatchParameters,
    ) -> Result<(), PreparedInstrumentError>;

    /// Delivers MIDI without changing voices or controller state owned by the
    /// other lane. Legacy implementations admit only their immediate source;
    /// an unsupported source must never silently alias it.
    fn dispatch_from(
        &mut self,
        source: MidiSource,
        message: MidiMessage,
        parameters: &RtPatchParameters,
    ) -> Result<(), PreparedInstrumentError> {
        match source {
            MidiSource::Immediate => self.dispatch(message, parameters),
            MidiSource::Scheduled | MidiSource::Monitor => {
                Err(PreparedInstrumentError::UnsupportedMidiSource { source })
            }
        }
    }

    /// Fills exactly `frame_count` frames in caller-owned interleaved stereo
    /// storage. The rack validates storage identity and capacity before this
    /// operation is called.
    fn render(
        &mut self,
        interleaved_stereo: &mut [f32],
        frame_count: usize,
        parameters: &RtPatchParameters,
    ) -> Result<(), PreparedInstrumentError>;

    /// Silences this instrument's voices with bounded work.
    fn all_notes_off(&mut self);

    /// Silences only voices owned by this lane, including sustained voices,
    /// without destroying prepared processors or affecting the other lane.
    /// An exact host articulation can release or modulate one overlapping voice.
    /// Actual held/sustained voice count; legacy fixtures may lack this observation.
    fn active_note_count(&self) -> Option<u32> {
        None
    }

    fn dispatch_voice_from(
        &mut self,
        source: MidiSource,
        voice: Option<MidiVoiceId>,
        message: MidiMessage,
        parameters: &crate::real_time::parameter_snapshot::RtPatchParameters,
    ) -> Result<(), PreparedInstrumentError> {
        if voice.is_some() {
            return Err(PreparedInstrumentError::UnsupportedVoiceIdentity);
        }
        self.dispatch_from(source, message, parameters)
    }

    /// Scoped controls retain their raw MIDI values and independent bend ranges.
    /// Unsupported engines reject master expression instead of flattening it.
    fn dispatch_scoped_voice_from(
        &mut self,
        source: MidiSource,
        voice: MidiVoiceId,
        scope: MidiExpressionScope,
        message: MidiMessage,
        parameters: &RtPatchParameters,
    ) -> Result<(), PreparedInstrumentError> {
        match scope {
            MidiExpressionScope::Member => {
                self.dispatch_voice_from(source, Some(voice), message, parameters)
            }
            MidiExpressionScope::Master => Err(PreparedInstrumentError::UnsupportedExpressionScope),
        }
    }

    /// Admits and initializes a complete owned attack atomically. Implementors
    /// validate both dimensions before changing voice or native state; they
    /// must not expand this into independently fallible MIDI dispatches.
    fn dispatch_initialized_voice_from(
        &mut self,
        _source: MidiSource,
        _voice: MidiVoiceId,
        _initial: OwnedVoiceInitialization,
        _parameters: &RtPatchParameters,
    ) -> Result<(), PreparedInstrumentError> {
        Err(PreparedInstrumentError::UnsupportedExpressionScope)
    }

    fn all_notes_off_from(&mut self, source: MidiSource) -> Result<(), PreparedInstrumentError> {
        match source {
            MidiSource::Immediate => {
                self.all_notes_off();
                Ok(())
            }
            MidiSource::Scheduled | MidiSource::Monitor => {
                Err(PreparedInstrumentError::UnsupportedMidiSource { source })
            }
        }
    }

    /// Reports one immutable shared prepared asset to the worker-side graph
    /// builder. Callback rendering never calls this method. The generic
    /// reference and numeric preparation key let the builder deduplicate and
    /// budget assets without switching on a concrete capability identity.
    fn prepared_asset_footprint(&self) -> Option<&PreparedAssetFootprint> {
        None
    }

    /// Optional bounded control-side visualization produced during
    /// preparation. The callback never calls this method.
    fn prepared_sample_visualization(&self) -> Option<&PreparedSampleVisualization> {
        None
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct PreparedAssetFootprint {
    reference: AssetReference,
    preparation_key: u64,
    bytes: usize,
    private_bytes: usize,
}

impl PreparedAssetFootprint {
    pub const fn new(reference: AssetReference, preparation_key: u64, bytes: usize) -> Self {
        Self {
            reference,
            preparation_key,
            bytes,
            private_bytes: 0,
        }
    }

    /// Media copied into independent upstream instances cannot be deduplicated.
    pub const fn with_private_bytes(mut self, bytes: usize) -> Self {
        self.private_bytes = bytes;
        self
    }
    pub const fn private_bytes(&self) -> usize {
        self.private_bytes
    }

    pub const fn reference(&self) -> &AssetReference {
        &self.reference
    }

    pub const fn preparation_key(&self) -> u64 {
        self.preparation_key
    }

    pub const fn bytes(&self) -> usize {
        self.bytes
    }
}

/// A fixed-size failure returned by callback-side instrument dispatch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreparedInstrumentError {
    UnsupportedMidiKind { kind: MidiMessageKind },
    UnsupportedMidiSource { source: MidiSource },
    UnsupportedVoiceIdentity,
    UnsupportedExpressionScope,
    DispatchRejected,
    RenderRejected,
    ScalarLayoutMismatch,
    InvalidFrameCapacity,
}

impl fmt::Display for PreparedInstrumentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedMidiKind { kind } => {
                write!(formatter, "prepared instrument does not support {kind:?}")
            }
            Self::UnsupportedMidiSource { source } => {
                write!(
                    formatter,
                    "prepared instrument does not support {source:?} MIDI ownership"
                )
            }
            Self::UnsupportedVoiceIdentity => {
                formatter.write_str("prepared instrument does not support host voice identity")
            }
            Self::UnsupportedExpressionScope => {
                formatter.write_str("prepared instrument does not support scoped master expression")
            }
            Self::DispatchRejected => formatter.write_str("prepared instrument rejected MIDI"),
            Self::RenderRejected => formatter.write_str("prepared instrument rejected rendering"),
            Self::ScalarLayoutMismatch => {
                formatter.write_str("prepared instrument scalar layout is incompatible")
            }
            Self::InvalidFrameCapacity => {
                formatter.write_str("prepared instrument frame capacity is incompatible")
            }
        }
    }
}

impl std::error::Error for PreparedInstrumentError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_status_is_copyable_and_has_no_destructor() {
        fn assert_copy<T: Copy>() {}

        assert_copy::<PreparedInstrumentError>();
        assert_copy::<MidiSource>();
        assert_copy::<OwnedExpressionState>();
        assert_copy::<OwnedVoiceInitialization>();
        assert!(!core::mem::needs_drop::<PreparedInstrumentError>());
        assert!(!core::mem::needs_drop::<MidiSource>());
        assert!(!core::mem::needs_drop::<OwnedExpressionState>());
        assert!(!core::mem::needs_drop::<OwnedVoiceInitialization>());
        assert_ne!(MidiSource::Immediate.index(), MidiSource::Scheduled.index());
    }

    #[test]
    fn owned_initialization_validates_normalized_state_and_positive_attacks() {
        let neutral = OwnedExpressionState::default();
        assert_eq!(
            (neutral.raw14(), neutral.semitones(), neutral.cents()),
            (8_192, 2, 0)
        );
        assert_eq!(
            (neutral.pressure(), neutral.timbre(), neutral.pedal()),
            (None, None, 0)
        );
        for invalid in [
            OwnedExpressionState::new(16_384, 2, 0, None, None, 0),
            OwnedExpressionState::new(8_192, 128, 0, None, None, 0),
            OwnedExpressionState::new(8_192, 2, 100, None, None, 0),
            OwnedExpressionState::new(8_192, 2, 0, Some(128), None, 0),
            OwnedExpressionState::new(8_192, 2, 0, None, Some(128), 0),
            OwnedExpressionState::new(8_192, 2, 0, None, None, 128),
        ] {
            assert_eq!(invalid, Err(PreparedInstrumentError::DispatchRejected));
        }
        let member = OwnedExpressionState::new(16_383, 127, 99, Some(127), Some(127), 127).unwrap();
        let channel = crate::kernel::MidiChannel::new(3).unwrap();
        let attack = MidiMessage::try_new(channel, MidiMessageKind::NoteOn, 60, 100).unwrap();
        let initial = OwnedVoiceInitialization::new(attack, member, neutral).unwrap();
        assert_eq!(initial.message(), attack);
        assert_eq!(initial.member(), member);
        assert_eq!(initial.master(), neutral);
        let json = serde_json::to_value(initial).unwrap();
        assert_eq!(json["member"]["raw14"], 16_383);
        assert_eq!(json["master"]["pressure"], serde_json::Value::Null);
        assert_eq!(
            OwnedVoiceInitialization::new(attack, neutral, member),
            Err(PreparedInstrumentError::UnsupportedExpressionScope)
        );
        for kind in [
            MidiMessageKind::NoteOff,
            MidiMessageKind::ControlChange,
            MidiMessageKind::PitchBend,
        ] {
            assert!(OwnedVoiceInitialization::new(
                MidiMessage::try_new(channel, kind, 60, 100).unwrap(),
                neutral,
                neutral
            )
            .is_err());
        }
        assert!(OwnedVoiceInitialization::new(
            MidiMessage::try_new(channel, MidiMessageKind::NoteOn, 60, 0).unwrap(),
            neutral,
            neutral
        )
        .is_err());
    }

    #[test]
    fn legacy_ports_preserve_immediate_calls_and_reject_unowned_scheduled_midi() {
        struct Legacy {
            patch: PatchId,
            dispatched: usize,
            silenced: usize,
        }
        impl PreparedInstrument for Legacy {
            fn patch_id(&self) -> PatchId {
                self.patch
            }
            fn dispatch(
                &mut self,
                _: MidiMessage,
                _: &RtPatchParameters,
            ) -> Result<(), PreparedInstrumentError> {
                self.dispatched += 1;
                Ok(())
            }
            fn render(
                &mut self,
                _: &mut [f32],
                _: usize,
                _: &RtPatchParameters,
            ) -> Result<(), PreparedInstrumentError> {
                Ok(())
            }
            fn all_notes_off(&mut self) {
                self.silenced += 1;
            }
        }
        let patch = PatchId::new(1).unwrap();
        let mut legacy = Legacy {
            patch,
            dispatched: 0,
            silenced: 0,
        };
        let parameters =
            RtPatchParameters::new(patch, crate::mixer::patch_output::PatchOutput::default());
        let message = MidiMessage::all_notes_off(crate::kernel::MidiChannel::new(0).unwrap());
        let unsupported = PreparedInstrumentError::UnsupportedMidiSource {
            source: MidiSource::Scheduled,
        };
        assert_eq!(
            legacy.dispatch_from(MidiSource::Scheduled, message, &parameters),
            Err(unsupported)
        );
        assert_eq!(
            legacy.all_notes_off_from(MidiSource::Scheduled),
            Err(unsupported)
        );
        assert_eq!((legacy.dispatched, legacy.silenced), (0, 0));
        let attack =
            MidiMessage::try_new(message.channel(), MidiMessageKind::NoteOn, 60, 100).unwrap();
        let initial = OwnedVoiceInitialization::new(
            attack,
            OwnedExpressionState::default(),
            OwnedExpressionState::default(),
        )
        .unwrap();
        for source in MidiSource::ALL {
            assert_eq!(
                legacy.dispatch_initialized_voice_from(
                    source,
                    MidiVoiceId::new(1).unwrap(),
                    initial,
                    &parameters
                ),
                Err(PreparedInstrumentError::UnsupportedExpressionScope)
            );
        }
        assert_eq!((legacy.dispatched, legacy.silenced), (0, 0));
        legacy
            .dispatch_from(MidiSource::Immediate, message, &parameters)
            .unwrap();
        legacy.all_notes_off_from(MidiSource::Immediate).unwrap();
        assert_eq!((legacy.dispatched, legacy.silenced), (1, 1));
    }
}
