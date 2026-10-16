use crate::kernel::PatchId;
use crate::real_time::patch_effect_observation::PatchEffectSlotObservations;
use crate::real_time::prepared_graph::PositionCapabilityIdentity;
use crate::real_time::{
    ParameterSnapshot, PatchAudioBlock, PatchEffectObservation, RtPostEffectParameters,
    MAX_ACTIVE_PATCHES,
};
use crate::synth::effect_slot_id::MAX_EFFECT_SLOTS;
use crate::synth::{EffectSlotId, PreparedEffectError, PreparedPostEffect};
use core::fmt;

pub(crate) struct PreparedPostEffectSlot {
    patch_id: PatchId,
    slot_id: EffectSlotId,
    scalar_count: usize,
    /// The effect capability identity this grid position was prepared from,
    /// recorded at build time from the validated candidate configuration.
    /// Fixed-size and copyable; compared only at carry-over decision time.
    capability_identity: Option<PositionCapabilityIdentity>,
    effect: Box<dyn PreparedPostEffect>,
    input_scratch: Vec<f32>,
}

impl PreparedPostEffectSlot {
    pub(crate) fn new(
        patch_id: PatchId,
        slot_id: EffectSlotId,
        scalar_count: usize,
        effect: Box<dyn PreparedPostEffect>,
        input_scratch: Vec<f32>,
    ) -> Self {
        Self {
            patch_id,
            slot_id,
            scalar_count,
            capability_identity: None,
            effect,
            input_scratch,
        }
    }
}

/// One Patch's fixed ordered effect positions.
pub(crate) type PreparedPatchEffectSlots = [Option<PreparedPostEffectSlot>; MAX_EFFECT_SLOTS];

/// Fixed-capacity Patch-aligned ownership of the prepared effect grid:
/// `MAX_EFFECT_SLOTS` ordered positions for each of `MAX_ACTIVE_PATCHES` Patches.
///
/// Each position is independently empty or occupied. Occupied positions
/// process their Patch's stem in place in ascending index order, an empty
/// position is skipped without compacting the others, and a fourth position
/// is unrepresentable.
pub struct PreparedPostEffectRack {
    patch_count: usize,
    patch_ids: [Option<PatchId>; MAX_ACTIVE_PATCHES],
    slots: [PreparedPatchEffectSlots; MAX_ACTIVE_PATCHES],
}

impl PreparedPostEffectRack {
    pub(crate) fn from_slots(
        patch_count: usize,
        patch_ids: [Option<PatchId>; MAX_ACTIVE_PATCHES],
        slots: [PreparedPatchEffectSlots; MAX_ACTIVE_PATCHES],
    ) -> Self {
        Self {
            patch_count,
            patch_ids,
            slots,
        }
    }

    pub const fn patch_count(&self) -> usize {
        self.patch_count
    }

    pub fn patch_id(&self, index: usize) -> Option<PatchId> {
        self.patch_ids.get(index).copied().flatten()
    }

    /// Returns the instance identity prepared at one exact grid position.
    pub fn slot_id_at(&self, patch_index: usize, slot_index: usize) -> Option<EffectSlotId> {
        self.slots
            .get(patch_index)
            .and_then(|slots| slots.get(slot_index))
            .and_then(Option::as_ref)
            .map(|slot| slot.slot_id)
    }

    /// Returns the scalar layout prepared at one exact grid position.
    pub fn scalar_count_at(&self, patch_index: usize, slot_index: usize) -> Option<usize> {
        self.slots
            .get(patch_index)
            .and_then(|slots| slots.get(slot_index))
            .and_then(Option::as_ref)
            .map(|slot| slot.scalar_count)
    }

    /// Returns the recorded effect capability identity at one grid position.
    pub fn capability_identity_at(
        &self,
        patch_index: usize,
        slot_index: usize,
    ) -> Option<PositionCapabilityIdentity> {
        self.slots
            .get(patch_index)
            .and_then(|slots| slots.get(slot_index))
            .and_then(Option::as_ref)
            .and_then(|slot| slot.capability_identity)
    }

    /// Records the validated effect capability identity prepared at one grid
    /// position.
    ///
    /// Prepare-time only: the graph builder stamps every occupied position
    /// from the validated candidate configuration right after rack
    /// preparation. Returns whether the position is occupied and carries the
    /// exact Patch and instance identity; a disagreeing position is left
    /// untouched.
    pub(crate) fn record_capability_identity_at(
        &mut self,
        patch_index: usize,
        slot_index: usize,
        patch_id: PatchId,
        slot_id: EffectSlotId,
        identity: PositionCapabilityIdentity,
    ) -> bool {
        if self.patch_ids.get(patch_index).copied().flatten() != Some(patch_id) {
            return false;
        }
        let Some(slot) = self
            .slots
            .get_mut(patch_index)
            .and_then(|slots| slots.get_mut(slot_index))
            .and_then(Option::as_mut)
        else {
            return false;
        };
        if slot.patch_id != patch_id || slot.slot_id != slot_id {
            return false;
        }
        slot.capability_identity = Some(identity);
        true
    }

    /// Returns how many of one Patch's ordered positions are occupied.
    pub fn occupied_slot_count(&self, patch_index: usize) -> usize {
        self.slots
            .get(patch_index)
            .map_or(0, |slots| slots.iter().flatten().count())
    }

    /// Proves the snapshot layout and this prepared grid agree exactly.
    ///
    /// Per Patch: identity must match, and every one of the ordered positions
    /// is checked independently against its own live entry — nothing weaker:
    ///
    /// - an empty position requires an inactive entry at that exact index;
    /// - an occupied position requires an active entry at that exact index
    ///   whose `slot_id` and `scalar_count` both match that instance;
    /// - an active entry at an index whose position is empty is a mismatch —
    ///   never repositioned, never partially consumed, never permissively
    ///   accepted.
    pub fn matches_parameters(&self, parameters: &ParameterSnapshot) -> bool {
        parameters.patch_count() == self.patch_count
            && parameters
                .patches()
                .iter()
                .enumerate()
                .all(|(index, parameters)| {
                    if parameters.patch_id() != self.patch_id(index) {
                        return false;
                    }
                    let effects = parameters.effects();
                    self.slots[index]
                        .iter()
                        .zip(effects)
                        .all(|(position, entry)| match position {
                            None => !entry.is_active(),
                            Some(slot) => {
                                entry.slot_id() == Some(slot.slot_id)
                                    && entry.scalar_count() == slot.scalar_count
                            }
                        })
                })
    }

    /// Processes each configured matching stem exactly once, in place, with
    /// every occupied position visited in ascending slot index order.
    pub fn process(
        &mut self,
        block: &mut PatchAudioBlock,
        parameters: &ParameterSnapshot,
        observations: &mut [PatchEffectObservation; MAX_ACTIVE_PATCHES],
    ) -> Result<(), EffectRackProcessError> {
        let mut slot_observations =
            [[PatchEffectObservation::EMPTY; MAX_EFFECT_SLOTS]; MAX_ACTIVE_PATCHES];
        self.process_with_slot_observations(block, parameters, observations, &mut slot_observations)
    }

    /// `process` with per-position measurement: one observation for every
    /// occupied grid position rather than one per Patch.
    pub fn process_with_slot_observations(
        &mut self,
        block: &mut PatchAudioBlock,
        parameters: &ParameterSnapshot,
        observations: &mut [PatchEffectObservation; MAX_ACTIVE_PATCHES],
        slot_observations: &mut PatchEffectSlotObservations,
    ) -> Result<(), EffectRackProcessError> {
        observations.fill(PatchEffectObservation::EMPTY);
        slot_observations.fill([PatchEffectObservation::EMPTY; MAX_EFFECT_SLOTS]);
        if block.patch_count() != self.patch_count {
            return Err(EffectRackProcessError::PatchCountMismatch);
        }
        if !self.matches_parameters(parameters) {
            return Err(EffectRackProcessError::ParameterLayoutMismatch);
        }
        let frame_count = block.frame_count();
        let sample_count = frame_count.saturating_mul(2);
        for index in 0..self.patch_count {
            let patch_id =
                self.patch_ids[index].ok_or(EffectRackProcessError::InactivePatch { index })?;
            if block.storage()[index].patch_id() != Some(patch_id) {
                return Err(EffectRackProcessError::StemIdentityMismatch { index });
            }
            if self.slots[index].iter().all(Option::is_none) {
                continue;
            }
            let stem = block
                .stem_mut(index, patch_id)
                .ok_or(EffectRackProcessError::StemIdentityMismatch { index })?;
            // One live entry per position: each occupied position reads
            // exactly the entry at its own index (`matches_parameters` proved
            // the per-position agreement above).
            let effects = parameters.patches()[index].effects();
            let slot_parameters: [&RtPostEffectParameters; MAX_EFFECT_SLOTS] =
                std::array::from_fn(|position| &effects[position]);
            Self::process_slots_in_place(
                &mut self.slots[index],
                index,
                patch_id,
                stem,
                frame_count,
                &slot_parameters,
                &mut slot_observations[index],
            )?;
            let first_occupied = self.slots[index]
                .iter()
                .flatten()
                .next()
                .ok_or(EffectRackProcessError::InactivePatch { index })?;
            observations[index] = PatchEffectObservation::measured(
                patch_id,
                &first_occupied.input_scratch[..sample_count],
                stem,
            );
        }
        Ok(())
    }

    /// Exchanges the still-live prepared effect instance from the superseded
    /// grid into this replacement at every position the structural delta
    /// leaves unchanged, so unchanged instances keep their delay/LFO/tail
    /// state across block-boundary activation.
    ///
    /// Callback-safe: bounded by `MAX_ACTIVE_PATCHES * MAX_EFFECT_SLOTS`
    /// pointer-sized `mem::swap`s with no allocation, deallocation, locking,
    /// blocking, or destruction. Only the effect box is exchanged; each
    /// position keeps its own preallocated scratch, which is overwritten
    /// before every use.
    ///
    /// `exclude` names the one `(PatchId, position)` the delta changed; that
    /// position keeps its fresh instance (the permitted local restart). Every
    /// exchange requires exact PatchId, instance-identity, scalar-layout, and
    /// recorded effect capability-identity agreement at the same grid
    /// position — a non-matching position keeps its fresh instance rather
    /// than gaining wrong state. The identity check is a fixed-size
    /// comparison made once at this carry-over decision, never per render
    /// block.
    pub(crate) fn carry_live_effects_from(
        &mut self,
        superseded: &mut Self,
        exclude: Option<(PatchId, usize)>,
    ) {
        if self.patch_count < superseded.patch_count
            || self.patch_count > superseded.patch_count.saturating_add(1)
        {
            return;
        }
        for index in 0..superseded.patch_count {
            let Some(patch_id) = self.patch_ids[index] else {
                continue;
            };
            if superseded.patch_ids[index] != Some(patch_id) {
                continue;
            }
            for position in 0..MAX_EFFECT_SLOTS {
                if exclude == Some((patch_id, position)) {
                    continue;
                }
                let (Some(target), Some(source)) = (
                    self.slots[index][position].as_mut(),
                    superseded.slots[index][position].as_mut(),
                ) else {
                    continue;
                };
                if target.patch_id != source.patch_id
                    || target.slot_id != source.slot_id
                    || target.scalar_count != source.scalar_count
                    || target.capability_identity != source.capability_identity
                {
                    continue;
                }
                core::mem::swap(&mut target.effect, &mut source.effect);
            }
        }
    }

    /// The one production chain loop: visits one Patch's ordered positions in
    /// ascending index, and each occupied position copies its live input to
    /// its own scratch then processes the stem in place — so position 1
    /// receives position 0's output.
    fn process_slots_in_place(
        slots: &mut PreparedPatchEffectSlots,
        index: usize,
        patch_id: PatchId,
        stem: &mut [f32],
        frame_count: usize,
        slot_parameters: &[&RtPostEffectParameters; MAX_EFFECT_SLOTS],
        slot_observations: &mut [PatchEffectObservation; MAX_EFFECT_SLOTS],
    ) -> Result<(), EffectRackProcessError> {
        let sample_count = frame_count.saturating_mul(2);
        for (position, slot) in slots.iter_mut().enumerate() {
            let Some(slot) = slot.as_mut() else {
                continue;
            };
            if slot.patch_id != patch_id {
                return Err(EffectRackProcessError::StemIdentityMismatch { index });
            }
            if sample_count > slot.input_scratch.len() {
                return Err(EffectRackProcessError::FrameCapacityExceeded);
            }
            slot.input_scratch[..sample_count].copy_from_slice(stem);
            slot.effect
                .process(stem, frame_count, slot_parameters[position])
                .map_err(|source| EffectRackProcessError::Effect { patch_id, source })?;
            slot_observations[position] = PatchEffectObservation::measured(
                patch_id,
                &slot.input_scratch[..sample_count],
                stem,
            );
        }
        Ok(())
    }
}

impl fmt::Debug for PreparedPostEffectRack {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedPostEffectRack")
            .field("patch_count", &self.patch_count)
            .field("patch_ids", &&self.patch_ids[..self.patch_count])
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectRackProcessError {
    PatchCountMismatch,
    ParameterLayoutMismatch,
    InactivePatch {
        index: usize,
    },
    StemIdentityMismatch {
        index: usize,
    },
    FrameCapacityExceeded,
    Effect {
        patch_id: PatchId,
        source: PreparedEffectError,
    },
}

impl fmt::Display for EffectRackProcessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PatchCountMismatch => formatter.write_str("effect rack Patch count mismatch"),
            Self::ParameterLayoutMismatch => {
                formatter.write_str("effect rack parameter layout mismatch")
            }
            Self::InactivePatch { index } => {
                write!(formatter, "effect rack Patch {index} is inactive")
            }
            Self::StemIdentityMismatch { index } => {
                write!(
                    formatter,
                    "effect rack stem {index} has the wrong Patch identity"
                )
            }
            Self::FrameCapacityExceeded => {
                formatter.write_str("effect rack frame capacity was exceeded")
            }
            Self::Effect { patch_id, source } => {
                write!(formatter, "Patch {patch_id} effect failed: {source}")
            }
        }
    }
}

impl std::error::Error for EffectRackProcessError {}

#[cfg(test)]
mod tests {
    use super::{
        EffectRackProcessError, PreparedPatchEffectSlots, PreparedPostEffectRack,
        PreparedPostEffectSlot,
    };
    use crate::adapter::production_effects::{
        production_effect_preparers, production_effect_registry,
    };
    use crate::kernel::midi_channel::MidiChannel;
    use crate::kernel::patch_id::PatchId;
    use crate::mixer::global_parameters::GlobalParameters;
    use crate::mixer::mixer_state::MixerState;
    use crate::mixer::mixer_track_id::MixerTrackId;
    use crate::mixer::patch_output::PatchOutput;
    use crate::real_time::prepared_graph::PositionCapabilityIdentity;
    use crate::real_time::{
        ParameterSnapshot, PatchAudioBlock, PatchEffectObservation, RtInstrumentParameters,
        RtPatchParameters, RtPostEffectParameters, MAX_ACTIVE_PATCHES,
    };
    use crate::synth::capability_id::CapabilityId;
    use crate::synth::effect_slot_id::{EffectSlotIndex, MAX_EFFECT_SLOTS};
    use crate::synth::voice_envelope::VoiceEnvelope;
    use crate::synth::{EffectCapabilityId, PreparedEffectError, PreparedPostEffect};
    use crate::synth::{
        EffectCapabilityRegistry, EffectPreparer, EffectSlotId, InstrumentConfig, Patch,
        PostEffectConfig, PreparedPostEffectRackBuilder,
    };

    const SAMPLE_RATE: f32 = 48_000.0;
    const MAX_FRAMES: usize = 256;

    fn patch_fixture(id: u32, occupancy: &[(usize, PostEffectConfig)]) -> Patch {
        let mut patch = Patch::new(
            PatchId::new(id).unwrap(),
            format!("Patch {id}"),
            InstrumentConfig::from_parts(
                CapabilityId::new("instrument.test").unwrap(),
                Vec::new(),
                Vec::new(),
            ),
            MidiChannel::new(((id - 1) % 16) as u8).unwrap(),
            PatchOutput::default(),
        );
        for (position, config) in occupancy {
            patch
                .set_slot_occupancy(
                    EffectSlotIndex::new(*position).unwrap(),
                    Some(config.clone()),
                )
                .unwrap();
        }
        patch
    }

    fn entry_config(
        registry: &EffectCapabilityRegistry,
        capability: &str,
        slot_id: u16,
    ) -> PostEffectConfig {
        registry
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.id().as_str() == capability)
            .expect("production registry entry exists")
            .default_config(EffectSlotId::new(slot_id).unwrap())
            .unwrap()
    }

    fn rt_effect(
        registry: &EffectCapabilityRegistry,
        config: &PostEffectConfig,
    ) -> RtPostEffectParameters {
        let descriptor = registry.descriptor(config.capability_id()).unwrap();
        let scalars: Vec<f32> = descriptor
            .scalar_parameters()
            .map(|spec| spec.scalar_value(config.value(spec.id()).unwrap()).unwrap())
            .collect();
        RtPostEffectParameters::new(config.slot_id(), &scalars).unwrap()
    }

    /// Builds one Patch's ordered live entries with effects at exact positions.
    fn effects_at(
        occupancy: &[(usize, RtPostEffectParameters)],
    ) -> [RtPostEffectParameters; MAX_EFFECT_SLOTS] {
        let mut effects = [const { RtPostEffectParameters::EMPTY }; MAX_EFFECT_SLOTS];
        for (position, effect) in occupancy {
            effects[*position] = effect.clone();
        }
        effects
    }

    fn snapshot_with_effects(
        generation: u64,
        entries: &[(u32, PatchOutput, [RtPostEffectParameters; MAX_EFFECT_SLOTS])],
    ) -> ParameterSnapshot {
        let patches: Vec<_> = entries
            .iter()
            .map(|(id, output, effects)| {
                RtPatchParameters::projected_with_effects(
                    PatchId::new(*id).unwrap(),
                    *output,
                    VoiceEnvelope::default(),
                    RtInstrumentParameters::EMPTY,
                    effects.clone(),
                )
            })
            .collect();
        ParameterSnapshot::new(
            generation,
            GlobalParameters::new(-3.0).unwrap(),
            MixerState::default(),
            &patches,
        )
        .unwrap()
    }

    fn build_rack(
        patches: &[Patch],
        registry: &EffectCapabilityRegistry,
        preparers: &[Box<dyn EffectPreparer>],
    ) -> PreparedPostEffectRack {
        PreparedPostEffectRackBuilder::build(patches, registry, preparers, SAMPLE_RATE, MAX_FRAMES)
            .unwrap()
    }

    fn standalone_instance(
        preparers: &[Box<dyn EffectPreparer>],
        patch_id: PatchId,
        config: &PostEffectConfig,
    ) -> Box<dyn crate::synth::PreparedPostEffect> {
        preparers
            .iter()
            .find(|preparer| preparer.capability_id() == config.capability_id())
            .expect("preparer registered")
            .prepare(patch_id, config, SAMPLE_RATE, MAX_FRAMES)
            .unwrap()
    }

    fn fill_signal(stem: &mut [f32], seed: usize) {
        for (index, frame) in stem.chunks_exact_mut(2).enumerate() {
            let value = (((index * 31 + seed * 17) % 97) as f32 - 48.0) * 0.01;
            frame[0] = value;
            frame[1] = -value * 0.5;
        }
    }

    fn rms_difference(a: &[f32], b: &[f32]) -> f32 {
        assert_eq!(a.len(), b.len());
        let energy = a.iter().zip(b).fold(0.0_f64, |sum, (left, right)| {
            let difference = f64::from(left - right);
            sum + difference * difference
        });
        ((energy / a.len().max(1) as f64).sqrt()) as f32
    }

    /// The widened check is exact PER POSITION; each deliberately mismatched
    /// layout is rejected, never repositioned, never partially consumed.
    /// Extends the original single-position negatives — none are weakened.
    #[test]
    fn matches_parameters_stays_exact_at_every_slot_position() {
        let registry = production_effect_registry().unwrap();
        let preparers = production_effect_preparers().unwrap();
        let chorus = entry_config(&registry, "effect.chorus", 1);
        let chorus_params = rt_effect(&registry, &chorus);
        let output = PatchOutput::default();
        let inactive = effects_at(&[]);

        // Occupied at position 0: exact match, wrong scalar_count rejected.
        let rack0 = build_rack(
            &[patch_fixture(1, &[(0, chorus.clone())])],
            &registry,
            &preparers,
        );
        assert!(rack0.matches_parameters(&snapshot_with_effects(
            1,
            &[(1, output, effects_at(&[(0, chorus_params.clone())]))]
        )));
        let wrong_scalars =
            RtPostEffectParameters::new(EffectSlotId::new(1).unwrap(), &[0.5]).unwrap();
        assert!(
            !rack0.matches_parameters(&snapshot_with_effects(
                2,
                &[(1, output, effects_at(&[(0, wrong_scalars)]))]
            )),
            "wrong scalar_count at position 0 must be rejected"
        );
        assert!(
            !rack0.matches_parameters(&snapshot_with_effects(3, &[(1, output, inactive.clone())])),
            "occupied position 0 facing inactive parameters must be rejected"
        );
        assert!(
            !rack0.matches_parameters(&snapshot_with_effects(
                4,
                &[(1, output, effects_at(&[(1, chorus_params.clone())]))]
            )),
            "the matching entry at the WRONG position must be rejected, not repositioned"
        );

        // Occupied at position 1: per-position match; occupied-vs-inactive and
        // right-entry-wrong-position rejected.
        let rack1 = build_rack(
            &[patch_fixture(1, &[(1, chorus.clone())])],
            &registry,
            &preparers,
        );
        assert!(rack1.matches_parameters(&snapshot_with_effects(
            5,
            &[(1, output, effects_at(&[(1, chorus_params.clone())]))]
        )));
        assert!(
            !rack1.matches_parameters(&snapshot_with_effects(6, &[(1, output, inactive.clone())])),
            "occupied position 1 facing inactive parameters must be rejected"
        );
        assert!(
            !rack1.matches_parameters(&snapshot_with_effects(
                7,
                &[(1, output, effects_at(&[(0, chorus_params.clone())]))]
            )),
            "an entry at position 0 must not attest an instance prepared at position 1"
        );

        // Occupied at position 2: wrong slot identity rejected.
        let chorus_at_two = entry_config(&registry, "effect.chorus", 5);
        let rack2 = build_rack(
            &[patch_fixture(2, &[(2, chorus_at_two.clone())])],
            &registry,
            &preparers,
        );
        assert!(rack2.matches_parameters(&snapshot_with_effects(
            8,
            &[(
                2,
                output,
                effects_at(&[(2, rt_effect(&registry, &chorus_at_two))])
            )]
        )));
        assert!(
            !rack2.matches_parameters(&snapshot_with_effects(
                9,
                &[(2, output, effects_at(&[(2, chorus_params.clone())]))]
            )),
            "wrong slot_id at position 2 must be rejected"
        );

        // Empty chain: an active parameter entry has no prepared position.
        let empty_rack = build_rack(&[patch_fixture(1, &[])], &registry, &preparers);
        assert!(empty_rack
            .matches_parameters(&snapshot_with_effects(10, &[(1, output, inactive.clone())])));
        assert!(
            !empty_rack.matches_parameters(&snapshot_with_effects(
                11,
                &[(1, output, effects_at(&[(0, chorus_params.clone())]))]
            )),
            "active parameters facing an empty chain must be rejected"
        );

        // Two occupied positions: only the exact two-position attestation
        // matches. Every partial or repositioned claim stays rejected — the
        // single-position negatives hold verbatim at the widened size.
        let reverb = entry_config(&registry, "effect.reverb", 2);
        let reverb_params = rt_effect(&registry, &reverb);
        let rack_multi = build_rack(
            &[patch_fixture(
                1,
                &[(0, chorus.clone()), (1, reverb.clone())],
            )],
            &registry,
            &preparers,
        );
        assert!(rack_multi.matches_parameters(&snapshot_with_effects(
            12,
            &[(
                1,
                output,
                effects_at(&[(0, chorus_params.clone()), (1, reverb_params.clone())])
            )]
        )));
        assert!(
            !rack_multi.matches_parameters(&snapshot_with_effects(
                13,
                &[(1, output, effects_at(&[(0, chorus_params.clone())]))]
            )),
            "naming only one of two occupied positions must be rejected"
        );
        assert!(!rack_multi.matches_parameters(&snapshot_with_effects(
            14,
            &[(1, output, effects_at(&[(1, reverb_params.clone())]))]
        )));
        assert!(!rack_multi
            .matches_parameters(&snapshot_with_effects(15, &[(1, output, inactive.clone())])));
        assert!(
            !rack_multi.matches_parameters(&snapshot_with_effects(
                16,
                &[(
                    1,
                    output,
                    effects_at(&[(0, reverb_params), (1, chorus_params.clone())])
                )]
            )),
            "exchanging the two entries across positions must be rejected"
        );

        // Patch identity and count stay part of the proof.
        assert!(!rack0.matches_parameters(&snapshot_with_effects(
            17,
            &[(2, output, effects_at(&[(0, chorus_params.clone())]))]
        )));
        assert!(!rack0.matches_parameters(&snapshot_with_effects(
            18,
            &[
                (1, output, effects_at(&[(0, chorus_params)])),
                (2, output, inactive)
            ]
        )));
    }

    /// T017/T018: each of the three positions independently carries a working
    /// effect through the public render path.
    #[test]
    fn public_process_applies_a_single_effect_at_each_position() {
        let registry = production_effect_registry().unwrap();
        let preparers = production_effect_preparers().unwrap();
        for position in 0..MAX_EFFECT_SLOTS {
            let chorus = entry_config(&registry, "effect.chorus", 1);
            let mut rack = build_rack(
                &[patch_fixture(1, &[(position, chorus.clone())])],
                &registry,
                &preparers,
            );
            let parameters = snapshot_with_effects(
                1,
                &[(
                    1,
                    PatchOutput::default(),
                    effects_at(&[(position, rt_effect(&registry, &chorus))]),
                )],
            );
            let mut block = PatchAudioBlock::prepare(MAX_FRAMES).unwrap();
            block.begin_render(&parameters, MAX_FRAMES).unwrap();
            let stem = block.stem_mut(0, PatchId::new(1).unwrap()).unwrap();
            fill_signal(stem, position);
            let input: Vec<f32> = stem.to_vec();

            let mut observations = [PatchEffectObservation::EMPTY; MAX_ACTIVE_PATCHES];
            let mut slot_observations =
                [[PatchEffectObservation::EMPTY; MAX_EFFECT_SLOTS]; MAX_ACTIVE_PATCHES];
            rack.process_with_slot_observations(
                &mut block,
                &parameters,
                &mut observations,
                &mut slot_observations,
            )
            .unwrap();

            let output = block
                .stem(0, PatchId::new(1).unwrap())
                .unwrap()
                .samples()
                .to_vec();
            assert!(
                rms_difference(&input, &output) > 1.0e-6,
                "position {position} must audibly process the stem"
            );
            assert_eq!(observations[0].patch_id(), Some(PatchId::new(1).unwrap()));
            assert!(observations[0].difference_rms() > 1.0e-6);
            for other in (0..MAX_EFFECT_SLOTS).filter(|other| *other != position) {
                assert_eq!(
                    slot_observations[0][other],
                    PatchEffectObservation::EMPTY,
                    "unoccupied position {other} must not be measured"
                );
            }
            assert_eq!(
                slot_observations[0][position].patch_id(),
                Some(PatchId::new(1).unwrap())
            );
        }
    }

    /// T017/T018: slots process in ascending index order, in place, so
    /// exchanging two genuinely different processors changes the sound, and
    /// each chain equals the sample-exact composition of its standalones.
    #[test]
    fn slot_order_is_render_order_and_exchanging_slots_changes_the_sound() {
        let registry = production_effect_registry().unwrap();
        let preparers = production_effect_preparers().unwrap();
        let patch_id = PatchId::new(1).unwrap();
        let chorus = entry_config(&registry, "effect.chorus", 1);
        let reverb = entry_config(&registry, "effect.reverb", 2);
        let chorus_params = rt_effect(&registry, &chorus);
        let reverb_params = rt_effect(&registry, &reverb);

        let mut rack_ab = build_rack(
            &[patch_fixture(
                1,
                &[(0, chorus.clone()), (1, reverb.clone())],
            )],
            &registry,
            &preparers,
        );
        let mut rack_ba = build_rack(
            &[patch_fixture(
                1,
                &[(0, reverb.clone()), (1, chorus.clone())],
            )],
            &registry,
            &preparers,
        );
        let mut standalone_chorus_ab = standalone_instance(&preparers, patch_id, &chorus);
        let mut standalone_reverb_ab = standalone_instance(&preparers, patch_id, &reverb);
        let mut standalone_reverb_ba = standalone_instance(&preparers, patch_id, &reverb);
        let mut standalone_chorus_ba = standalone_instance(&preparers, patch_id, &chorus);

        // Enough blocks for the reverb tail and chorus modulation to evolve;
        // over a few milliseconds both stages are near-linear and the two
        // orders would spuriously agree.
        let mut total_order_difference = 0.0_f32;
        for block_index in 0..40 {
            let mut input = vec![0.0_f32; MAX_FRAMES * 2];
            fill_signal(&mut input, block_index);

            let mut stem_ab = input.clone();
            let mut observations_ab = [PatchEffectObservation::EMPTY; MAX_EFFECT_SLOTS];
            PreparedPostEffectRack::process_slots_in_place(
                &mut rack_ab.slots[0],
                0,
                patch_id,
                &mut stem_ab,
                MAX_FRAMES,
                &[
                    &chorus_params,
                    &reverb_params,
                    &RtPostEffectParameters::EMPTY,
                ],
                &mut observations_ab,
            )
            .unwrap();
            let mut expected_ab = input.clone();
            standalone_chorus_ab
                .process(&mut expected_ab, MAX_FRAMES, &chorus_params)
                .unwrap();
            let chorus_stage_output = expected_ab.clone();
            standalone_reverb_ab
                .process(&mut expected_ab, MAX_FRAMES, &reverb_params)
                .unwrap();
            assert_eq!(
                stem_ab, expected_ab,
                "block {block_index}: [A, B] must equal B(A(x)) sample-exactly"
            );
            // In place: position 1's measured input is position 0's output.
            assert_eq!(
                observations_ab[1].input_rms(),
                observations_ab[0].output_rms(),
                "slot 1 must receive slot 0's output"
            );
            assert_eq!(
                observations_ab[0].output_rms(),
                PatchEffectObservation::measured(patch_id, &input, &chorus_stage_output)
                    .output_rms()
            );

            let mut stem_ba = input.clone();
            let mut observations_ba = [PatchEffectObservation::EMPTY; MAX_EFFECT_SLOTS];
            PreparedPostEffectRack::process_slots_in_place(
                &mut rack_ba.slots[0],
                0,
                patch_id,
                &mut stem_ba,
                MAX_FRAMES,
                &[
                    &reverb_params,
                    &chorus_params,
                    &RtPostEffectParameters::EMPTY,
                ],
                &mut observations_ba,
            )
            .unwrap();
            let mut expected_ba = input.clone();
            standalone_reverb_ba
                .process(&mut expected_ba, MAX_FRAMES, &reverb_params)
                .unwrap();
            standalone_chorus_ba
                .process(&mut expected_ba, MAX_FRAMES, &chorus_params)
                .unwrap();
            assert_eq!(
                stem_ba, expected_ba,
                "block {block_index}: [B, A] must equal A(B(x)) sample-exactly"
            );

            total_order_difference = total_order_difference.max(rms_difference(&stem_ab, &stem_ba));
        }
        assert!(
            total_order_difference > 1.0e-6,
            "exchanging two effects must measurably change the sound"
        );
    }

    /// T018/FR-005: two instances of the same registry entry on one Patch keep
    /// disjoint delay-line, LFO, and tail state — the chain equals the
    /// composition of two independently running standalones on every block,
    /// including tail blocks after the input falls silent.
    #[test]
    fn same_registry_entry_instances_keep_disjoint_state() {
        let registry = production_effect_registry().unwrap();
        let preparers = production_effect_preparers().unwrap();
        let patch_id = PatchId::new(1).unwrap();
        let first = entry_config(&registry, "effect.chorus", 1);
        let second = entry_config(&registry, "effect.chorus", 2);
        let first_params = rt_effect(&registry, &first);
        let second_params = rt_effect(&registry, &second);

        let mut rack = build_rack(
            &[patch_fixture(1, &[(0, first.clone()), (1, second.clone())])],
            &registry,
            &preparers,
        );
        let mut standalone_first = standalone_instance(&preparers, patch_id, &first);
        let mut standalone_second = standalone_instance(&preparers, patch_id, &second);

        for block_index in 0..4 {
            let mut input = vec![0.0_f32; MAX_FRAMES * 2];
            if block_index < 2 {
                fill_signal(&mut input, block_index);
            }

            let mut chain = input.clone();
            let mut observations = [PatchEffectObservation::EMPTY; MAX_EFFECT_SLOTS];
            PreparedPostEffectRack::process_slots_in_place(
                &mut rack.slots[0],
                0,
                patch_id,
                &mut chain,
                MAX_FRAMES,
                &[
                    &first_params,
                    &second_params,
                    &RtPostEffectParameters::EMPTY,
                ],
                &mut observations,
            )
            .unwrap();

            let mut expected = input.clone();
            standalone_first
                .process(&mut expected, MAX_FRAMES, &first_params)
                .unwrap();
            let first_stage_output = expected.clone();
            standalone_second
                .process(&mut expected, MAX_FRAMES, &second_params)
                .unwrap();

            assert_eq!(
                chain, expected,
                "block {block_index}: instance state must stay disjoint"
            );
            assert_eq!(
                observations[0].output_rms(),
                PatchEffectObservation::measured(patch_id, &input, &first_stage_output)
                    .output_rms(),
                "block {block_index}: the second instance must not alter the first's output"
            );
        }
    }

    /// T018/FR-018: rerouting the Patch's output changes no effect output —
    /// the chain, its values, and its instance state follow the Patch.
    #[test]
    fn rerouting_the_patch_output_preserves_chain_and_instance_state() {
        let registry = production_effect_registry().unwrap();
        let preparers = production_effect_preparers().unwrap();
        let chorus = entry_config(&registry, "effect.chorus", 1);
        let chorus_params = rt_effect(&registry, &chorus);
        let patches = [patch_fixture(1, &[(0, chorus.clone())])];
        let mut stable_rack = build_rack(&patches, &registry, &preparers);
        let mut rerouted_rack = build_rack(&patches, &registry, &preparers);

        let original = PatchOutput::default();
        let rerouted = PatchOutput::new(MixerTrackId::new(9).unwrap(), -6.0).unwrap();
        let stable_snapshot = snapshot_with_effects(
            1,
            &[(1, original, effects_at(&[(0, chorus_params.clone())]))],
        );
        let rerouted_snapshot =
            snapshot_with_effects(2, &[(1, rerouted, effects_at(&[(0, chorus_params)]))]);
        assert!(stable_rack.matches_parameters(&rerouted_snapshot));

        for block_index in 0..3 {
            // The rerouted rack switches output routing on the final block;
            // the stable rack never does. Effect output must stay identical.
            let rerouted_parameters = if block_index == 2 {
                &rerouted_snapshot
            } else {
                &stable_snapshot
            };
            let mut outputs = Vec::new();
            for (rack, parameters) in [
                (&mut stable_rack, &stable_snapshot),
                (&mut rerouted_rack, rerouted_parameters),
            ] {
                let mut block = PatchAudioBlock::prepare(MAX_FRAMES).unwrap();
                block.begin_render(parameters, MAX_FRAMES).unwrap();
                let stem = block.stem_mut(0, PatchId::new(1).unwrap()).unwrap();
                fill_signal(stem, block_index);
                let mut observations = [PatchEffectObservation::EMPTY; MAX_ACTIVE_PATCHES];
                rack.process(&mut block, parameters, &mut observations)
                    .unwrap();
                outputs.push(
                    block
                        .stem(0, PatchId::new(1).unwrap())
                        .unwrap()
                        .samples()
                        .to_vec(),
                );
            }
            assert_eq!(
                outputs[0], outputs[1],
                "block {block_index}: rerouting must not alter the effect chain"
            );
        }
    }

    struct MarkerPostEffect {
        patch_id: PatchId,
        slot_id: EffectSlotId,
        add: f32,
    }

    impl PreparedPostEffect for MarkerPostEffect {
        fn patch_id(&self) -> PatchId {
            self.patch_id
        }

        fn slot_id(&self) -> EffectSlotId {
            self.slot_id
        }

        fn process(
            &mut self,
            interleaved_stereo: &mut [f32],
            _frame_count: usize,
            _parameters: &RtPostEffectParameters,
        ) -> Result<(), PreparedEffectError> {
            for sample in interleaved_stereo.iter_mut() {
                *sample += self.add;
            }
            Ok(())
        }
    }

    /// One Patch with one marker occupant at position 1, its recorded effect
    /// capability identity stamped as the graph builder does at build time.
    fn marker_rack(add: f32, capability: &str) -> PreparedPostEffectRack {
        let patch_id = PatchId::new(1).unwrap();
        let slot_id = EffectSlotId::new(7).unwrap();
        let mut patch_ids = [None; MAX_ACTIVE_PATCHES];
        patch_ids[0] = Some(patch_id);
        let mut slots: [PreparedPatchEffectSlots; MAX_ACTIVE_PATCHES] =
            std::array::from_fn(|_| std::array::from_fn(|_| None));
        slots[0][1] = Some(PreparedPostEffectSlot::new(
            patch_id,
            slot_id,
            0,
            Box::new(MarkerPostEffect {
                patch_id,
                slot_id,
                add,
            }),
            vec![0.0; MAX_FRAMES * 2],
        ));
        let mut rack = PreparedPostEffectRack::from_slots(1, patch_ids, slots);
        let identity = PositionCapabilityIdentity::from_effect_capability_id(
            &EffectCapabilityId::new(capability).unwrap(),
        )
        .unwrap();
        assert!(rack.record_capability_identity_at(0, 1, patch_id, slot_id, identity));
        rack
    }

    fn processed_marker(rack: &mut PreparedPostEffectRack) -> f32 {
        let patch_id = PatchId::new(1).unwrap();
        let mut stem = vec![0.0_f32; MAX_FRAMES * 2];
        let mut observations = [PatchEffectObservation::EMPTY; MAX_EFFECT_SLOTS];
        PreparedPostEffectRack::process_slots_in_place(
            &mut rack.slots[0],
            0,
            patch_id,
            &mut stem,
            MAX_FRAMES,
            &[
                &RtPostEffectParameters::EMPTY,
                &RtPostEffectParameters::EMPTY,
                &RtPostEffectParameters::EMPTY,
            ],
            &mut observations,
        )
        .unwrap();
        stem[0]
    }

    /// A candidate agreeing on Patch, instance identity, and scalar layout
    /// but carrying a different recorded effect capability identity at the
    /// same grid position is refused: the freshly prepared instance stays
    /// and nothing panics.
    #[test]
    fn carry_over_capability_identity_mismatch_keeps_the_fresh_effect_instance() {
        let mut fresh = marker_rack(0.25, "effect.alpha");
        let mut superseded = marker_rack(0.75, "effect.beta");

        fresh.carry_live_effects_from(&mut superseded, None);

        assert_eq!(processed_marker(&mut fresh), 0.25);
        assert_eq!(processed_marker(&mut superseded), 0.75);
    }

    /// Exact per-position agreement — Patch, instance identity, scalar
    /// layout, and recorded capability identity — still carries the live
    /// effect instance and its state over.
    #[test]
    fn carry_over_capability_identity_agreement_still_carries_the_live_effect() {
        let mut fresh = marker_rack(0.25, "effect.alpha");
        let mut superseded = marker_rack(0.75, "effect.alpha");

        fresh.carry_live_effects_from(&mut superseded, None);

        assert_eq!(processed_marker(&mut fresh), 0.75);
        assert_eq!(processed_marker(&mut superseded), 0.25);
    }

    #[test]
    fn append_carries_existing_effects_even_though_the_candidate_has_one_more_patch() {
        let mut fresh = marker_rack(0.25, "effect.alpha");
        let mut superseded = marker_rack(0.75, "effect.alpha");
        fresh.patch_count = 2;
        fresh.patch_ids[1] = Some(PatchId::new(2).unwrap());

        fresh.carry_live_effects_from(&mut superseded, None);

        assert_eq!(processed_marker(&mut fresh), 0.75);
        assert_eq!(processed_marker(&mut superseded), 0.25);
    }

    /// Every occupied position enforces its own frame capacity.
    #[test]
    fn each_slot_enforces_its_own_frame_capacity() {
        let registry = production_effect_registry().unwrap();
        let preparers = production_effect_preparers().unwrap();
        let chorus = entry_config(&registry, "effect.chorus", 1);
        let mut rack = PreparedPostEffectRackBuilder::build(
            &[patch_fixture(1, &[(0, chorus.clone())])],
            &registry,
            &preparers,
            SAMPLE_RATE,
            128,
        )
        .unwrap();
        let parameters = snapshot_with_effects(
            1,
            &[(
                1,
                PatchOutput::default(),
                effects_at(&[(0, rt_effect(&registry, &chorus))]),
            )],
        );
        let mut block = PatchAudioBlock::prepare(256).unwrap();
        block.begin_render(&parameters, 256).unwrap();
        let mut observations = [PatchEffectObservation::EMPTY; MAX_ACTIVE_PATCHES];

        assert_eq!(
            rack.process(&mut block, &parameters, &mut observations),
            Err(EffectRackProcessError::FrameCapacityExceeded)
        );
    }
}
