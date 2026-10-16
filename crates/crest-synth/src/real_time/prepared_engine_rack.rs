use crate::kernel::midi_message::MidiMessage;
use crate::kernel::patch_id::PatchId;
use crate::real_time::parameter_snapshot::{
    ParameterSnapshot, RtPatchParameters, MAX_ACTIVE_PATCHES,
};
use crate::real_time::patch_audio_block::PatchAudioBlock;
use crate::real_time::prepared_graph::PositionCapabilityIdentity;
use crate::synth::prepared_instrument::{MidiSource, PreparedInstrument, PreparedInstrumentError};
use core::fmt;

pub(crate) struct PreparedEngineSlot {
    patch_id: PatchId,
    scalar_count: usize,
    /// The engine capability identity this position was prepared from,
    /// recorded at build time from the validated candidate configuration.
    /// Fixed-size and copyable; compared only at carry-over decision time.
    capability_identity: Option<PositionCapabilityIdentity>,
    instrument: Box<dyn PreparedInstrument>,
}

impl PreparedEngineSlot {
    pub(crate) fn new(
        patch_id: PatchId,
        scalar_count: usize,
        instrument: Box<dyn PreparedInstrument>,
    ) -> Self {
        Self {
            patch_id,
            scalar_count,
            capability_identity: None,
            instrument,
        }
    }
}

/// Fixed-capacity ordered ownership of one prepared instrument per Patch.
///
/// The rack contains no engine-specific identity or branch. Every callback
/// operation is bounded by `MAX_ACTIVE_PATCHES`, and rendering uses only the matching
/// caller-owned stem.
pub struct PreparedEngineRack {
    patch_count: usize,
    slots: [Option<PreparedEngineSlot>; MAX_ACTIVE_PATCHES],
}

impl PreparedEngineRack {
    pub(crate) fn prepared_asset_footprints(
        &self,
    ) -> impl Iterator<Item = &crate::synth::PreparedAssetFootprint> {
        self.slots[..self.patch_count]
            .iter()
            .filter_map(Option::as_ref)
            .flat_map(|slot| slot.instrument.prepared_asset_footprints())
    }

    pub(crate) fn accepts_note(&self, patch_id: PatchId, note: u8) -> bool {
        self.slots[..self.patch_count]
            .iter()
            .flatten()
            .find(|slot| slot.patch_id == patch_id)
            .is_none_or(|slot| slot.instrument.accepts_note(note))
    }

    pub(crate) fn prepared_sample_visualizations(
        &self,
        patch_id: PatchId,
    ) -> Vec<crate::synth::PreparedSampleVisualization> {
        self.slots[..self.patch_count]
            .iter()
            .flatten()
            .find(|slot| slot.patch_id == patch_id)
            .map(|slot| slot.instrument.prepared_sample_visualizations())
            .unwrap_or_default()
    }

    pub(crate) fn prepared_sample_visualization(
        &self,
        patch_id: PatchId,
    ) -> Option<&crate::synth::PreparedSampleVisualization> {
        self.slots[..self.patch_count]
            .iter()
            .filter_map(Option::as_ref)
            .find(|slot| slot.patch_id == patch_id)
            .and_then(|slot| slot.instrument.prepared_sample_visualization())
    }

    pub(crate) fn from_slots(
        patch_count: usize,
        slots: [Option<PreparedEngineSlot>; MAX_ACTIVE_PATCHES],
    ) -> Self {
        Self { patch_count, slots }
    }

    /// Returns the number of active prepared slots.
    pub const fn patch_count(&self) -> usize {
        self.patch_count
    }

    /// Returns the Patch identity at one canonical slot index.
    pub fn patch_id(&self, index: usize) -> Option<PatchId> {
        self.slots
            .get(index)
            .and_then(Option::as_ref)
            .map(|slot| slot.patch_id)
    }

    /// Returns the fixed descriptor-ordered Scalar layout for one slot.
    pub fn scalar_count(&self, index: usize) -> Option<usize> {
        self.slots
            .get(index)
            .and_then(Option::as_ref)
            .map(|slot| slot.scalar_count)
    }

    /// Returns the recorded engine capability identity at one slot index.
    pub fn capability_identity(&self, index: usize) -> Option<PositionCapabilityIdentity> {
        self.slots
            .get(index)
            .and_then(Option::as_ref)
            .and_then(|slot| slot.capability_identity)
    }

    /// Records the validated engine capability identity prepared at one slot.
    ///
    /// Prepare-time only: the graph builder stamps every position from the
    /// validated candidate configuration right after rack preparation.
    /// Returns whether the position is active and carries the exact Patch
    /// identity; a disagreeing position is left untouched.
    pub(crate) fn record_capability_identity(
        &mut self,
        index: usize,
        patch_id: PatchId,
        identity: PositionCapabilityIdentity,
    ) -> bool {
        let Some(slot) = self.slots.get_mut(index).and_then(Option::as_mut) else {
            return false;
        };
        if slot.patch_id != patch_id {
            return false;
        }
        slot.capability_identity = Some(identity);
        true
    }

    /// Returns whether a parameter snapshot has the exact rack revision and
    /// ordered Patch identities.
    pub fn matches_parameters(&self, parameters: &ParameterSnapshot) -> bool {
        parameters.patch_count() == self.patch_count
            && parameters
                .patches()
                .iter()
                .enumerate()
                .all(|(index, patch)| {
                    patch.patch_id() == self.patch_id(index)
                        && Some(patch.instrument().count()) == self.scalar_count(index)
                })
    }

    /// Routes one message to only the slot with the exact Patch identity.
    pub fn dispatch(
        &mut self,
        patch_id: PatchId,
        message: MidiMessage,
        parameters: &RtPatchParameters,
    ) -> Result<(), RackDispatchError> {
        self.dispatch_from(MidiSource::Immediate, patch_id, message, parameters)
    }

    /// Routes one source-owned message without changing the other MIDI lane.
    pub fn dispatch_from(
        &mut self,
        midi_source: MidiSource,
        patch_id: PatchId,
        message: MidiMessage,
        parameters: &RtPatchParameters,
    ) -> Result<(), RackDispatchError> {
        self.dispatch_voice_from(midi_source, None, patch_id, message, parameters)
    }
    pub fn dispatch_voice_from(
        &mut self,
        midi_source: MidiSource,
        voice: Option<crate::synth::MidiVoiceId>,
        patch_id: PatchId,
        message: MidiMessage,
        parameters: &RtPatchParameters,
    ) -> Result<(), RackDispatchError> {
        self.dispatch_scoped_voice_from(
            midi_source,
            voice,
            crate::synth::MidiExpressionScope::Member,
            patch_id,
            message,
            parameters,
        )
    }
    pub fn dispatch_scoped_voice_from(
        &mut self,
        midi_source: MidiSource,
        voice: Option<crate::synth::MidiVoiceId>,
        scope: crate::synth::MidiExpressionScope,
        patch_id: PatchId,
        message: MidiMessage,
        parameters: &RtPatchParameters,
    ) -> Result<(), RackDispatchError> {
        let Some(slot) = self.find_slot_mut(patch_id) else {
            return Err(RackDispatchError::UnknownPatch { patch_id });
        };
        if parameters.patch_id() != Some(patch_id)
            || parameters.instrument().count() != slot.scalar_count
        {
            return Err(RackDispatchError::ParameterLayoutMismatch { patch_id });
        }
        let result = match voice {
            Some(voice) => slot.instrument.dispatch_scoped_voice_from(
                midi_source,
                voice,
                scope,
                message,
                parameters,
            ),
            None if scope == crate::synth::MidiExpressionScope::Member => slot
                .instrument
                .dispatch_from(midi_source, message, parameters),
            None => Err(PreparedInstrumentError::UnsupportedExpressionScope),
        };
        result.map_err(|source| RackDispatchError::Instrument { patch_id, source })
    }

    pub fn dispatch_initialized_voice_from(
        &mut self,
        source: MidiSource,
        voice: crate::synth::MidiVoiceId,
        patch_id: PatchId,
        initial: crate::synth::OwnedVoiceInitialization,
        parameters: &RtPatchParameters,
    ) -> Result<(), RackDispatchError> {
        let slot = self
            .find_slot_mut(patch_id)
            .ok_or(RackDispatchError::UnknownPatch { patch_id })?;
        if parameters.patch_id() != Some(patch_id)
            || parameters.instrument().count() != slot.scalar_count
        {
            return Err(RackDispatchError::ParameterLayoutMismatch { patch_id });
        }
        slot.instrument
            .dispatch_initialized_voice_from(source, voice, initial, parameters)
            .map_err(|source| RackDispatchError::Instrument { patch_id, source })
    }

    pub fn active_note_count(&self, patch_id: PatchId) -> Option<u32> {
        self.slots
            .iter()
            .flatten()
            .find(|slot| slot.patch_id == patch_id)
            .and_then(|slot| slot.instrument.active_note_count())
    }

    /// Silences only the slot with the exact Patch identity.
    pub fn all_notes_off_for(&mut self, patch_id: PatchId) -> Result<(), RackDispatchError> {
        let Some(slot) = self.find_slot_mut(patch_id) else {
            return Err(RackDispatchError::UnknownPatch { patch_id });
        };
        slot.instrument.all_notes_off();
        Ok(())
    }

    /// Silences one delivery lane in one exact Patch, including sustained
    /// voices. Unsupported source ownership remains a typed dispatch failure.
    pub fn all_notes_off_for_source(
        &mut self,
        midi_source: MidiSource,
        patch_id: PatchId,
    ) -> Result<(), RackDispatchError> {
        let Some(slot) = self.find_slot_mut(patch_id) else {
            return Err(RackDispatchError::UnknownPatch { patch_id });
        };
        slot.instrument
            .all_notes_off_from(midi_source)
            .map_err(|source| RackDispatchError::Instrument { patch_id, source })
    }

    /// Silences every active instrument with work bounded by `MAX_ACTIVE_PATCHES`.
    pub fn all_notes_off(&mut self) {
        for slot in self.slots[..self.patch_count].iter_mut().flatten() {
            slot.instrument.all_notes_off();
        }
    }

    /// Clears and fills every exact caller-owned Patch stem once.
    pub fn render(
        &mut self,
        block: &mut PatchAudioBlock,
        parameters: &ParameterSnapshot,
    ) -> Result<(), RackRenderError> {
        if block.patch_count() != self.patch_count {
            return Err(RackRenderError::PatchCountMismatch {
                rack: self.patch_count,
                stems: block.patch_count(),
            });
        }
        if !self.matches_parameters(parameters) {
            return Err(RackRenderError::ParameterLayoutMismatch);
        }

        for (index, slot) in self.slots[..self.patch_count].iter().enumerate() {
            let Some(slot) = slot else {
                return Err(RackRenderError::InactiveSlot { index });
            };
            if block.storage()[index].patch_id() != Some(slot.patch_id) {
                return Err(RackRenderError::StemIdentityMismatch {
                    index,
                    expected: slot.patch_id,
                    actual: block.storage()[index].patch_id(),
                });
            }
        }

        block.clear();
        let frame_count = block.frame_count();
        for (index, slot) in self.slots[..self.patch_count].iter_mut().enumerate() {
            let Some(slot) = slot else {
                return Err(RackRenderError::InactiveSlot { index });
            };
            let Some(stem) = block.stem_mut(index, slot.patch_id) else {
                return Err(RackRenderError::StemIdentityMismatch {
                    index,
                    expected: slot.patch_id,
                    actual: None,
                });
            };
            slot.instrument
                .render(stem, frame_count, &parameters.patches()[index])
                .map_err(|source| RackRenderError::Instrument {
                    patch_id: slot.patch_id,
                    source,
                })?;
        }
        Ok(())
    }

    fn find_slot_mut(&mut self, patch_id: PatchId) -> Option<&mut PreparedEngineSlot> {
        self.slots[..self.patch_count]
            .iter_mut()
            .flatten()
            .find(|slot| slot.patch_id == patch_id)
    }

    /// Exchanges the still-live prepared instrument from the superseded rack
    /// into this replacement at every slot the structural delta leaves
    /// unchanged, so sounding voices survive block-boundary activation.
    ///
    /// Callback-safe: the work is bounded by `MAX_ACTIVE_PATCHES` pointer-sized
    /// `mem::swap`s with no allocation, deallocation, locking, blocking, or
    /// destruction. The freshly prepared (never sounded) instruments ride
    /// into the superseded rack, which retires off-callback as before.
    ///
    /// `exclude` names the one Patch whose engine identity the delta changed;
    /// that slot keeps its fresh instrument (the permitted local restart).
    /// Every exchange requires exact PatchId, scalar-layout, and recorded
    /// engine capability-identity agreement at the same index — a
    /// non-matching position keeps its fresh instrument rather than
    /// substituting anything. The identity check is a fixed-size comparison
    /// made once at this carry-over decision, never per render block.
    pub(crate) fn carry_live_instruments_from(
        &mut self,
        superseded: &mut Self,
        exclude: Option<PatchId>,
    ) {
        if self.patch_count < superseded.patch_count
            || self.patch_count > superseded.patch_count.saturating_add(1)
        {
            return;
        }
        for index in 0..superseded.patch_count {
            let (Some(target), Some(source)) =
                (self.slots[index].as_mut(), superseded.slots[index].as_mut())
            else {
                continue;
            };
            if target.patch_id != source.patch_id
                || Some(target.patch_id) == exclude
                || target.scalar_count != source.scalar_count
                || target.capability_identity != source.capability_identity
            {
                continue;
            }
            core::mem::swap(&mut target.instrument, &mut source.instrument);
        }
    }
}

impl fmt::Debug for PreparedEngineRack {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedEngineRack")
            .field("patch_count", &self.patch_count)
            .field(
                "patch_ids",
                &&self.slots[..self.patch_count]
                    .iter()
                    .map(|slot| slot.as_ref().map(|slot| slot.patch_id))
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// Fixed-size callback status for targeted rack dispatch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RackDispatchError {
    UnknownPatch {
        patch_id: PatchId,
    },
    Instrument {
        patch_id: PatchId,
        source: PreparedInstrumentError,
    },
    ParameterLayoutMismatch {
        patch_id: PatchId,
    },
}

impl fmt::Display for RackDispatchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownPatch { patch_id } => {
                write!(formatter, "prepared rack has no Patch {patch_id}")
            }
            Self::Instrument { patch_id, source } => {
                write!(
                    formatter,
                    "prepared Patch {patch_id} rejected MIDI: {source}"
                )
            }
            Self::ParameterLayoutMismatch { patch_id } => write!(
                formatter,
                "prepared Patch {patch_id} received an incompatible parameter projection"
            ),
        }
    }
}

impl std::error::Error for RackDispatchError {}

/// Fixed-size callback status for caller-owned stem validation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RackRenderError {
    Instrument {
        patch_id: PatchId,
        source: PreparedInstrumentError,
    },
    PatchCountMismatch {
        rack: usize,
        stems: usize,
    },
    InactiveSlot {
        index: usize,
    },
    StemIdentityMismatch {
        index: usize,
        expected: PatchId,
        actual: Option<PatchId>,
    },
    ParameterLayoutMismatch,
}

impl fmt::Display for RackRenderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Instrument { patch_id, source } => {
                write!(formatter, "Patch {patch_id}: {source}")
            }
            Self::PatchCountMismatch { rack, stems } => {
                write!(
                    formatter,
                    "prepared rack has {rack} Patches but block has {stems} stems"
                )
            }
            Self::InactiveSlot { index } => {
                write!(formatter, "prepared rack slot {index} is empty")
            }
            Self::StemIdentityMismatch {
                index,
                expected,
                actual,
            } => write!(
                formatter,
                "prepared rack slot {index} expects Patch {expected}, got {actual:?}"
            ),
            Self::ParameterLayoutMismatch => {
                formatter.write_str("prepared rack parameter layout is incompatible")
            }
        }
    }
}

impl std::error::Error for RackRenderError {}

#[cfg(test)]
mod tests {
    use super::{PreparedEngineRack, PreparedEngineSlot, RackDispatchError, RackRenderError};
    use crate::kernel::midi_message::MidiMessage;
    use crate::kernel::patch_id::PatchId;
    use crate::mixer::global_parameters::GlobalParameters;
    use crate::mixer::mixer_state::MixerState;
    use crate::mixer::patch_output::PatchOutput;
    use crate::real_time::parameter_snapshot::{
        ParameterSnapshot, RtPatchParameters, MAX_ACTIVE_PATCHES,
    };
    use crate::real_time::patch_audio_block::PatchAudioBlock;
    use crate::real_time::prepared_graph::PositionCapabilityIdentity;
    use crate::synth::capability_id::CapabilityId;
    use crate::synth::prepared_instrument::{
        MidiSource, PreparedInstrument, PreparedInstrumentError,
    };

    const FRAMES: usize = 8;

    struct MarkerInstrument {
        patch_id: PatchId,
        fill: f32,
    }

    impl PreparedInstrument for MarkerInstrument {
        fn patch_id(&self) -> PatchId {
            self.patch_id
        }

        fn dispatch(
            &mut self,
            _message: MidiMessage,
            _parameters: &RtPatchParameters,
        ) -> Result<(), PreparedInstrumentError> {
            Ok(())
        }

        fn render(
            &mut self,
            output: &mut [f32],
            _frame_count: usize,
            _parameters: &RtPatchParameters,
        ) -> Result<(), crate::synth::PreparedInstrumentError> {
            output.fill(self.fill);

            Ok(())
        }

        fn all_notes_off(&mut self) {}
    }

    fn identity(value: &str) -> PositionCapabilityIdentity {
        PositionCapabilityIdentity::from_capability_id(&CapabilityId::new(value).unwrap()).unwrap()
    }

    fn rack(fill: f32, capability: &str) -> PreparedEngineRack {
        let patch_id = PatchId::new(1).unwrap();
        let mut slots: [Option<PreparedEngineSlot>; MAX_ACTIVE_PATCHES] =
            std::array::from_fn(|_| None);
        slots[0] = Some(PreparedEngineSlot::new(
            patch_id,
            0,
            Box::new(MarkerInstrument { patch_id, fill }),
        ));
        let mut rack = PreparedEngineRack::from_slots(1, slots);
        assert!(rack.record_capability_identity(0, patch_id, identity(capability)));
        rack
    }

    fn rendered_fills(rack: &mut PreparedEngineRack) -> Vec<f32> {
        let patch_parameters = (0..rack.patch_count())
            .map(|index| {
                RtPatchParameters::new(rack.patch_id(index).unwrap(), PatchOutput::default())
            })
            .collect::<Vec<_>>();
        let parameters = ParameterSnapshot::new(
            1,
            GlobalParameters::new(0.0).unwrap(),
            MixerState::default(),
            &patch_parameters,
        )
        .unwrap();
        let mut block = PatchAudioBlock::prepare(FRAMES).unwrap();
        block.begin_render(&parameters, FRAMES).unwrap();
        rack.render(&mut block, &parameters).unwrap();
        block.stems().iter().map(|stem| stem.samples()[0]).collect()
    }

    fn rendered_fill(rack: &mut PreparedEngineRack) -> f32 {
        rendered_fills(rack)[0]
    }

    #[test]
    fn source_dispatch_and_cleanup_preserve_explicit_unsupported_failures() {
        let mut rack = rack(0.25, "instrument.alpha");
        let patch_id = PatchId::new(1).unwrap();
        let parameters = RtPatchParameters::new(patch_id, PatchOutput::default());
        let message = MidiMessage::all_notes_off(crate::kernel::MidiChannel::new(0).unwrap());
        let expected = RackDispatchError::Instrument {
            patch_id,
            source: PreparedInstrumentError::UnsupportedMidiSource {
                source: MidiSource::Scheduled,
            },
        };
        assert_eq!(
            rack.dispatch_from(MidiSource::Scheduled, patch_id, message, &parameters),
            Err(expected)
        );
        assert_eq!(
            rack.all_notes_off_for_source(MidiSource::Scheduled, patch_id),
            Err(expected)
        );
        assert_eq!(rack.dispatch(patch_id, message, &parameters), Ok(()));
        assert_eq!(
            rack.all_notes_off_for_source(MidiSource::Immediate, patch_id),
            Ok(())
        );
    }

    /// A candidate agreeing on Patch identity and scalar layout but carrying
    /// a different recorded engine capability identity is refused: the
    /// freshly prepared instrument stays, nothing panics, and the live
    /// instance never crosses the capability boundary.
    #[test]
    fn carry_over_capability_identity_mismatch_keeps_the_fresh_engine_instance() {
        let mut fresh = rack(0.25, "instrument.alpha");
        let mut superseded = rack(0.75, "instrument.beta");

        fresh.carry_live_instruments_from(&mut superseded, None);

        assert_eq!(rendered_fill(&mut fresh), 0.25);
        assert_eq!(rendered_fill(&mut superseded), 0.75);
    }

    /// Exact per-position agreement — Patch, scalar layout, and recorded
    /// capability identity — still carries the live instrument over.
    #[test]
    fn carry_over_capability_identity_agreement_still_carries_the_live_engine() {
        let mut fresh = rack(0.25, "instrument.alpha");
        let mut superseded = rack(0.75, "instrument.alpha");

        fresh.carry_live_instruments_from(&mut superseded, None);

        assert_eq!(rendered_fill(&mut fresh), 0.75);
        assert_eq!(rendered_fill(&mut superseded), 0.25);
    }

    #[test]
    fn append_carries_the_existing_prefix_and_leaves_the_new_engine_fresh() {
        let mut fresh = rack(0.25, "instrument.alpha");
        let mut superseded = rack(0.75, "instrument.alpha");
        let appended_id = PatchId::new(2).unwrap();
        fresh.slots[1] = Some(PreparedEngineSlot::new(
            appended_id,
            0,
            Box::new(MarkerInstrument {
                patch_id: appended_id,
                fill: 0.5,
            }),
        ));
        fresh.patch_count = 2;
        assert!(fresh.record_capability_identity(1, appended_id, identity("instrument.alpha")));

        fresh.carry_live_instruments_from(&mut superseded, None);

        assert_eq!(rendered_fills(&mut fresh), [0.75, 0.5]);
        assert_eq!(rendered_fill(&mut superseded), 0.25);
        assert_eq!(fresh.slots[1].as_ref().unwrap().patch_id, appended_id);
    }

    #[test]
    fn callback_statuses_are_copyable_and_have_no_destructors() {
        fn assert_copy<T: Copy>() {}

        assert_copy::<RackDispatchError>();
        assert_copy::<RackRenderError>();
        assert!(!core::mem::needs_drop::<RackDispatchError>());
        assert!(!core::mem::needs_drop::<RackRenderError>());
    }
}
