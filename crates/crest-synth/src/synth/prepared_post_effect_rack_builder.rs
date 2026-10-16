use crate::kernel::PatchId;
use crate::real_time::{PreparedPostEffectRack, PreparedPostEffectSlot, MAX_ACTIVE_PATCHES};
use crate::synth::effect_slot_id::MAX_EFFECT_SLOTS;
use crate::synth::{
    EffectCapabilityError, EffectCapabilityId, EffectCapabilityRegistry, EffectPreparationError,
    EffectPreparer, EffectSlotId, Patch,
};
use core::fmt;

pub struct PreparedPostEffectRackBuilder;

impl PreparedPostEffectRackBuilder {
    pub fn build(
        patches: &[Patch],
        registry: &EffectCapabilityRegistry,
        preparers: &[Box<dyn EffectPreparer>],
        sample_rate: f32,
        max_frames: usize,
    ) -> Result<PreparedPostEffectRack, EffectRackPreparationError> {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err(EffectRackPreparationError::InvalidSampleRate);
        }
        if max_frames == 0 {
            return Err(EffectRackPreparationError::InvalidFrameCapacity);
        }
        if patches.len() > MAX_ACTIVE_PATCHES {
            return Err(EffectRackPreparationError::PatchCapacityExceeded {
                count: patches.len(),
                capacity: MAX_ACTIVE_PATCHES,
            });
        }
        for (index, patch) in patches.iter().enumerate() {
            if patches[..index]
                .iter()
                .any(|prior| prior.id() == patch.id())
            {
                return Err(EffectRackPreparationError::DuplicatePatchId {
                    patch_id: patch.id(),
                });
            }
            // Per-position validation: the slot array already bounds the
            // chain, so each occupied position is validated individually and
            // instance identities must stay unique across the Patch's slots.
            for (position, config) in patch.effect_slots().iter().enumerate() {
                let Some(config) = config else {
                    continue;
                };
                if patch.effect_slots()[..position]
                    .iter()
                    .flatten()
                    .any(|prior| prior.slot_id() == config.slot_id())
                {
                    return Err(EffectRackPreparationError::InvalidConfiguration {
                        patch_id: patch.id(),
                        source: EffectCapabilityError::DuplicateSlot(config.slot_id()),
                    });
                }
                registry.validate_config(config).map_err(|source| {
                    EffectRackPreparationError::InvalidConfiguration {
                        patch_id: patch.id(),
                        source,
                    }
                })?;
            }
        }
        for (index, preparer) in preparers.iter().enumerate() {
            if preparers[..index]
                .iter()
                .any(|prior| prior.capability_id() == preparer.capability_id())
            {
                return Err(EffectRackPreparationError::DuplicatePreparer {
                    capability_id: preparer.capability_id().clone(),
                });
            }
            if registry.descriptor(preparer.capability_id()).is_none() {
                return Err(EffectRackPreparationError::ExtraPreparer {
                    capability_id: preparer.capability_id().clone(),
                });
            }
        }
        for descriptor in registry.descriptors() {
            if !preparers
                .iter()
                .any(|preparer| preparer.capability_id() == descriptor.id())
            {
                return Err(EffectRackPreparationError::MissingPreparer {
                    capability_id: descriptor.id().clone(),
                });
            }
        }

        let mut patch_ids = [None; MAX_ACTIVE_PATCHES];
        let mut slots: [[Option<PreparedPostEffectSlot>; MAX_EFFECT_SLOTS]; MAX_ACTIVE_PATCHES] =
            std::array::from_fn(|_| std::array::from_fn(|_| None));
        let sample_capacity = max_frames
            .checked_mul(2)
            .ok_or(EffectRackPreparationError::InvalidFrameCapacity)?;
        for (index, patch) in patches.iter().enumerate() {
            patch_ids[index] = Some(patch.id());
            for (position, config) in patch.effect_slots().iter().enumerate() {
                let Some(config) = config else {
                    continue;
                };
                let descriptor = registry
                    .descriptor(config.capability_id())
                    .expect("validated effect config is registered");
                let preparer = preparers
                    .iter()
                    .find(|preparer| preparer.capability_id() == config.capability_id())
                    .expect("exact preparer registration was validated");
                let effect = preparer
                    .prepare(patch.id(), config, sample_rate, max_frames)
                    .map_err(|source| EffectRackPreparationError::Effect {
                        patch_id: patch.id(),
                        source,
                    })?;
                if effect.patch_id() != patch.id() || effect.slot_id() != config.slot_id() {
                    return Err(EffectRackPreparationError::PreparedIdentityMismatch {
                        expected_patch_id: patch.id(),
                        actual_patch_id: effect.patch_id(),
                        expected_slot_id: config.slot_id(),
                        actual_slot_id: effect.slot_id(),
                    });
                }
                // Every occupied position owns its own preallocated input
                // scratch; three occupied slots mean three buffers, so the
                // render path never grows and in-place chaining never shares.
                let mut scratch = Vec::new();
                scratch.try_reserve_exact(sample_capacity).map_err(|_| {
                    EffectRackPreparationError::StorageAllocationFailed {
                        patch_id: patch.id(),
                    }
                })?;
                scratch.resize(sample_capacity, 0.0);
                slots[index][position] = Some(PreparedPostEffectSlot::new(
                    patch.id(),
                    config.slot_id(),
                    descriptor.scalar_parameter_count(),
                    effect,
                    scratch,
                ));
            }
        }
        Ok(PreparedPostEffectRack::from_slots(
            patches.len(),
            patch_ids,
            slots,
        ))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EffectRackPreparationError {
    InvalidSampleRate,
    InvalidFrameCapacity,
    PatchCapacityExceeded {
        count: usize,
        capacity: usize,
    },
    DuplicatePatchId {
        patch_id: PatchId,
    },
    DuplicatePreparer {
        capability_id: EffectCapabilityId,
    },
    MissingPreparer {
        capability_id: EffectCapabilityId,
    },
    ExtraPreparer {
        capability_id: EffectCapabilityId,
    },
    InvalidConfiguration {
        patch_id: PatchId,
        source: EffectCapabilityError,
    },
    Effect {
        patch_id: PatchId,
        source: EffectPreparationError,
    },
    PreparedIdentityMismatch {
        expected_patch_id: PatchId,
        actual_patch_id: PatchId,
        expected_slot_id: EffectSlotId,
        actual_slot_id: EffectSlotId,
    },
    StorageAllocationFailed {
        patch_id: PatchId,
    },
}

impl fmt::Display for EffectRackPreparationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSampleRate => formatter.write_str("effect rack sample rate is invalid"),
            Self::InvalidFrameCapacity => formatter.write_str("effect rack frame capacity is invalid"),
            Self::PatchCapacityExceeded { count, capacity } => {
                write!(formatter, "effect rack has {count} Patches; capacity is {capacity}")
            }
            Self::DuplicatePatchId { patch_id } => write!(formatter, "duplicate Patch {patch_id}"),
            Self::DuplicatePreparer { capability_id } => write!(formatter, "duplicate effect preparer {capability_id}"),
            Self::MissingPreparer { capability_id } => write!(formatter, "missing effect preparer {capability_id}"),
            Self::ExtraPreparer { capability_id } => write!(formatter, "extra effect preparer {capability_id}"),
            Self::InvalidConfiguration { patch_id, source } => write!(formatter, "Patch {patch_id} effect config is invalid: {source}"),
            Self::Effect { patch_id, source } => write!(formatter, "Patch {patch_id} effect preparation failed: {source}"),
            Self::PreparedIdentityMismatch { expected_patch_id, actual_patch_id, expected_slot_id, actual_slot_id } => write!(formatter, "prepared effect identity {actual_patch_id}/{actual_slot_id} does not match {expected_patch_id}/{expected_slot_id}"),
            Self::StorageAllocationFailed { patch_id } => write!(formatter, "Patch {patch_id} effect scratch allocation failed"),
        }
    }
}

impl std::error::Error for EffectRackPreparationError {}

#[cfg(test)]
mod tests {
    use super::PreparedPostEffectRackBuilder;
    use crate::adapter::production_effects::{
        production_effect_preparers, production_effect_registry,
    };
    use crate::kernel::midi_channel::MidiChannel;
    use crate::kernel::patch_id::PatchId;
    use crate::mixer::patch_output::PatchOutput;
    use crate::real_time::MAX_ACTIVE_PATCHES;
    use crate::synth::capability_id::CapabilityId;
    use crate::synth::effect_slot_id::{EffectSlotIndex, MAX_EFFECT_SLOTS};
    use crate::synth::patch::EffectSlotOccupancyError;
    use crate::synth::{
        EffectCapabilityRegistry, EffectSlotId, InstrumentConfig, Patch, PostEffectConfig,
    };

    fn patch(id: u32) -> Patch {
        Patch::new(
            PatchId::new(id).unwrap(),
            format!("Patch {id}"),
            InstrumentConfig::from_parts(
                CapabilityId::new("instrument.test").unwrap(),
                Vec::new(),
                Vec::new(),
            ),
            MidiChannel::new(((id - 1) % 16) as u8).unwrap(),
            PatchOutput::default(),
        )
    }

    fn default_config(
        registry: &EffectCapabilityRegistry,
        descriptor_index: usize,
        slot_id: u16,
    ) -> PostEffectConfig {
        registry.descriptors()[descriptor_index]
            .default_config(EffectSlotId::new(slot_id).unwrap())
            .unwrap()
    }

    #[test]
    fn a_fully_occupied_sixteen_by_three_grid_prepares_with_every_position_owned() {
        let registry = production_effect_registry().unwrap();
        let preparers = production_effect_preparers().unwrap();
        let descriptor_count = registry.descriptors().len();
        let patches: Vec<_> = (1..=MAX_ACTIVE_PATCHES as u32)
            .map(|id| {
                let mut patch = patch(id);
                for position in 0..MAX_EFFECT_SLOTS {
                    let descriptor_index = (id as usize + position) % descriptor_count;
                    patch
                        .set_slot_occupancy(
                            EffectSlotIndex::new(position).unwrap(),
                            Some(default_config(
                                &registry,
                                descriptor_index,
                                position as u16 + 1,
                            )),
                        )
                        .unwrap();
                }
                patch
            })
            .collect();

        let rack =
            PreparedPostEffectRackBuilder::build(&patches, &registry, &preparers, 48_000.0, 128)
                .unwrap();

        assert_eq!(rack.patch_count(), MAX_ACTIVE_PATCHES);
        for patch_index in 0..MAX_ACTIVE_PATCHES {
            assert_eq!(rack.occupied_slot_count(patch_index), MAX_EFFECT_SLOTS);
            for position in 0..MAX_EFFECT_SLOTS {
                assert_eq!(
                    rack.slot_id_at(patch_index, position),
                    Some(EffectSlotId::new(position as u16 + 1).unwrap())
                );
                let descriptor =
                    &registry.descriptors()[(patch_index + 1 + position) % descriptor_count];
                assert_eq!(
                    rack.scalar_count_at(patch_index, position),
                    Some(descriptor.scalar_parameters().count())
                );
            }
        }
    }

    #[test]
    fn empty_positions_stay_empty_without_compacting_occupied_neighbors() {
        let registry = production_effect_registry().unwrap();
        let preparers = production_effect_preparers().unwrap();
        let mut gapped = patch(1);
        gapped
            .set_slot_occupancy(
                EffectSlotIndex::new(0).unwrap(),
                Some(default_config(&registry, 0, 1)),
            )
            .unwrap();
        gapped
            .set_slot_occupancy(
                EffectSlotIndex::new(2).unwrap(),
                Some(default_config(&registry, 1, 2)),
            )
            .unwrap();

        let rack =
            PreparedPostEffectRackBuilder::build(&[gapped], &registry, &preparers, 48_000.0, 128)
                .unwrap();

        assert_eq!(rack.slot_id_at(0, 0), Some(EffectSlotId::new(1).unwrap()));
        assert_eq!(rack.slot_id_at(0, 1), None);
        assert_eq!(rack.slot_id_at(0, 2), Some(EffectSlotId::new(2).unwrap()));
        assert_eq!(rack.occupied_slot_count(0), 2);
    }

    #[test]
    fn duplicate_instance_identity_across_positions_never_reaches_preparation() {
        let registry = production_effect_registry().unwrap();
        let preparers = production_effect_preparers().unwrap();
        let mut duplicated = patch(1);
        duplicated
            .set_slot_occupancy(
                EffectSlotIndex::new(0).unwrap(),
                Some(default_config(&registry, 0, 1)),
            )
            .unwrap();

        // The aggregate refuses the second position outright, so no chain
        // carrying a duplicated instance identity can ever be handed to the
        // builder: the identity is rejected, never silently replaced.
        assert_eq!(
            duplicated.set_slot_occupancy(
                EffectSlotIndex::new(1).unwrap(),
                Some(default_config(&registry, 1, 1)),
            ),
            Err(EffectSlotOccupancyError::DuplicateSlotId(
                EffectSlotId::new(1).unwrap()
            ))
        );

        let rack = PreparedPostEffectRackBuilder::build(
            &[duplicated],
            &registry,
            &preparers,
            48_000.0,
            128,
        )
        .unwrap();

        assert_eq!(rack.slot_id_at(0, 0), Some(EffectSlotId::new(1).unwrap()));
        assert_eq!(rack.slot_id_at(0, 1), None);
        assert_eq!(rack.occupied_slot_count(0), 1);
    }
}
