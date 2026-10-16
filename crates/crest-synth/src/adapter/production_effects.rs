use crate::adapter::chorus_capability::ChorusCapability;
use crate::adapter::chorus_preparer::ChorusPreparer;
use crate::adapter::delay_capability::DelayCapability;
use crate::adapter::delay_preparer::DelayPreparer;
use crate::adapter::reverb_capability::ReverbCapability;
use crate::adapter::reverb_preparer::ReverbPreparer;
use crate::kernel::PatchId;
use crate::mixer::bus_id::BusId;
use crate::mixer::bus_return::{BusReturnBank, BusReturnError};
use crate::real_time::RtPostEffectParameters;
use crate::synth::{
    compose_effect_registry, EffectCapabilityError, EffectCapabilityId, EffectCapabilityProvider,
    EffectCapabilityRegistry, EffectCompositionError, EffectPreparationError, EffectPreparer,
    EffectSlotId, PostEffectConfig, PreparedPostEffect,
};

#[derive(Clone, Debug, thiserror::Error)]
pub enum ProductionEffectCompositionError {
    #[error(transparent)]
    Upstream(#[from] super::upstream_audio::CatalogError),
    #[error("failed to construct the production effect capability: {0}")]
    Capability(EffectCapabilityError),
    #[error("failed to construct the production effect preparer: {0}")]
    Preparation(EffectPreparationError),
    #[error("failed to compose the production effect registry: {0}")]
    Composition(EffectCompositionError),
    #[error("failed to occupy a default bus return: {0}")]
    ReturnOccupancy(BusReturnError),
    #[error("failed to project a default bus return: {0}")]
    ReturnProjection(crate::real_time::ParameterSnapshotError),
}

/// The one stable declared production registration order: Chorus, Reverb,
/// Delay. Reverb and delay are ordinary registry entries, peers of Chorus;
/// nothing about an entry records whether it will occupy a Patch effect slot
/// or a bus return.
pub fn production_effect_providers(
) -> Result<Vec<Box<dyn EffectCapabilityProvider>>, ProductionEffectCompositionError> {
    let mut ports: Vec<Box<dyn EffectCapabilityProvider>> = vec![
        Box::new(ChorusCapability::new().map_err(ProductionEffectCompositionError::Capability)?),
        Box::new(ReverbCapability::new().map_err(ProductionEffectCompositionError::Capability)?),
        Box::new(DelayCapability::new().map_err(ProductionEffectCompositionError::Capability)?),
    ];
    ports.extend(
        super::upstream_audio::effect_ports()?
            .into_iter()
            .map(|p| Box::new(p) as Box<dyn EffectCapabilityProvider>),
    );
    Ok(ports)
}

pub fn production_effect_preparers(
) -> Result<Vec<Box<dyn EffectPreparer>>, ProductionEffectCompositionError> {
    let mut ports: Vec<Box<dyn EffectPreparer>> = vec![
        Box::new(ChorusPreparer::new().map_err(ProductionEffectCompositionError::Preparation)?),
        Box::new(ReverbPreparer::new().map_err(ProductionEffectCompositionError::Preparation)?),
        Box::new(DelayPreparer::new().map_err(ProductionEffectCompositionError::Preparation)?),
    ];
    ports.extend(
        super::upstream_audio::effect_ports()?
            .into_iter()
            .map(|p| Box::new(p) as Box<dyn EffectPreparer>),
    );
    Ok(ports)
}

pub fn production_effect_registry(
) -> Result<EffectCapabilityRegistry, ProductionEffectCompositionError> {
    let providers = production_effect_providers()?;
    let preparers = production_effect_preparers()?;
    compose_effect_registry(&providers, &preparers)
        .map_err(ProductionEffectCompositionError::Composition)
}

/// The default production occupancy of the bus-return bank.
///
/// BusId 0 begins occupied by `effect.reverb` and BusId 1 by `effect.delay`,
/// carrying the behavior of the retired mixer-owned global effects; returns 2
/// through 7 begin unoccupied. This is default occupancy — a composition
/// choice made here in the adapter — not identity: every return changes
/// contents through the same `SetReturnOccupancy` gesture, and nothing in the
/// domain names an effect.
pub fn production_default_bus_returns(
    registry: &EffectCapabilityRegistry,
) -> Result<BusReturnBank, ProductionEffectCompositionError> {
    let reverb =
        EffectCapabilityId::new("effect.reverb").expect("static production id is namespaced");
    let delay =
        EffectCapabilityId::new("effect.delay").expect("static production id is namespaced");
    let mut bank = BusReturnBank::default();
    bank.set_return_occupancy(registry, BusId::ALL[0], Some(&reverb))
        .map_err(ProductionEffectCompositionError::ReturnOccupancy)?;
    bank.set_return_occupancy(registry, BusId::ALL[1], Some(&delay))
        .map_err(ProductionEffectCompositionError::ReturnOccupancy)?;
    Ok(bank)
}

/// A new production document starts with an empty INIT send bank.
/// Effects are selected from the shared registry through prepared graph edits.
pub fn production_startup_bus_returns(
    _registry: &EffectCapabilityRegistry,
) -> Result<BusReturnBank, ProductionEffectCompositionError> {
    Ok(BusReturnBank::default())
}

/// Compatibility factory for test fixtures using the same empty startup bank.
/// Registry content does not select default occupants.
pub fn startup_bus_returns(_registry: &EffectCapabilityRegistry) -> BusReturnBank {
    BusReturnBank::default()
}

pub fn production_chorus_config(
    slot_id: EffectSlotId,
) -> Result<PostEffectConfig, ProductionEffectCompositionError> {
    ChorusCapability::new()
        .map_err(ProductionEffectCompositionError::Capability)?
        .default_config(slot_id)
        .map_err(ProductionEffectCompositionError::Capability)
}

/// One prepared occupant of the production default bus-return bank, ready to
/// install into a mix engine's return rack.
pub struct PreparedDefaultBusReturn {
    pub bus: BusId,
    pub effect: Box<dyn PreparedPostEffect>,
    /// Install-time structural attestation: the occupying instance's identity
    /// and descriptor-ordered default scalars.
    pub parameters: RtPostEffectParameters,
    pub return_level: f32,
}

/// Prepares one registry instance per occupied return of
/// [`production_default_bus_returns`] — reverb on bus 0 and delay on bus 1 —
/// for installation into a prepared graph's return rack.
///
/// This replaces the retired `GlobalReverbDelay` bridge: the same registry
/// entries the bridge prepared, now installed as ordinary return occupancy.
pub fn prepare_production_default_bus_returns(
    sample_rate: f32,
    max_frames: usize,
) -> Result<Vec<PreparedDefaultBusReturn>, ProductionEffectCompositionError> {
    let registry = production_effect_registry()?;
    let preparers = production_effect_preparers()?;
    let bank = production_default_bus_returns(&registry)?;
    let patch_id = PatchId::new(u32::MAX).expect("the static bus-return Patch id is non-zero");

    let mut prepared = Vec::new();
    for bus_return in bank.returns() {
        let Some(config) = bus_return.effect() else {
            continue;
        };
        let rejected = || {
            ProductionEffectCompositionError::ReturnOccupancy(
                BusReturnError::ConfigurationRejected {
                    id: config.capability_id().clone(),
                },
            )
        };
        let descriptor = registry
            .descriptor(config.capability_id())
            .ok_or_else(rejected)?;
        let scalars = descriptor
            .scalar_parameters()
            .map(|spec| {
                let value = config.value(spec.id()).ok_or_else(rejected)?;
                spec.scalar_value(value).map_err(|_| rejected())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let parameters = RtPostEffectParameters::new(config.slot_id(), &scalars)
            .map_err(ProductionEffectCompositionError::ReturnProjection)?;
        let preparer = preparers
            .iter()
            .find(|preparer| preparer.capability_id() == config.capability_id())
            .ok_or_else(rejected)?;
        let effect = preparer
            .prepare(patch_id, config, sample_rate, max_frames)
            .map_err(ProductionEffectCompositionError::Preparation)?;
        prepared.push(PreparedDefaultBusReturn {
            bus: bus_return.id(),
            effect,
            parameters,
            return_level: bus_return.return_level(),
        });
    }
    Ok(prepared)
}

#[cfg(test)]
mod tests {
    use super::{
        production_effect_preparers, production_effect_providers, production_effect_registry,
    };
    use crate::kernel::PatchId;
    use crate::synth::EffectSlotId;

    #[test]
    fn registry_installs_chorus_reverb_and_delay_in_stable_order() {
        let registry = production_effect_registry().unwrap();

        let ids: Vec<_> = registry
            .descriptors()
            .iter()
            .map(|descriptor| descriptor.id().as_str().to_owned())
            .collect();
        let mut expected = vec![
            "effect.chorus".to_owned(),
            "effect.reverb".to_owned(),
            "effect.delay".to_owned(),
        ];
        expected.extend(
            crate::adapter::upstream_audio::effect_ports()
                .unwrap()
                .iter()
                .map(|port| {
                    crate::synth::EffectCapabilityProvider::descriptor(port)
                        .id()
                        .as_str()
                        .to_owned()
                }),
        );
        assert_eq!(ids, expected);
    }

    #[test]
    fn every_production_entry_prepares_from_its_descriptor_defaults() {
        // C-ER-1: each registered entry resolves and prepares without any
        // caller-supplied role hint.
        let registry = production_effect_registry().unwrap();
        let preparers = production_effect_preparers().unwrap();
        let patch_id = PatchId::new(1).unwrap();
        let slot_id = EffectSlotId::new(1).unwrap();

        for descriptor in registry.descriptors() {
            let config = descriptor.default_config(slot_id).unwrap();
            let preparer = preparers
                .iter()
                .find(|preparer| preparer.capability_id() == descriptor.id())
                .expect("every registered entry has exactly one preparer");
            let prepared = preparer
                .prepare(patch_id, &config, 48_000.0, 64)
                .expect("descriptor defaults prepare");
            assert_eq!(prepared.slot_id(), slot_id);
        }
    }

    #[test]
    fn default_bus_returns_occupy_zero_and_one_and_leave_the_rest_empty() {
        let registry = production_effect_registry().unwrap();
        let bank = super::production_default_bus_returns(&registry).unwrap();

        let occupants: Vec<Option<String>> = bank
            .returns()
            .iter()
            .map(|bus_return| {
                bus_return
                    .effect()
                    .map(|config| config.capability_id().as_str().to_owned())
            })
            .collect();
        assert_eq!(occupants[0].as_deref(), Some("effect.reverb"));
        assert_eq!(occupants[1].as_deref(), Some("effect.delay"));
        assert!(occupants[2..].iter().all(Option::is_none));
        for bus_return in bank.returns() {
            assert_eq!(bus_return.return_level(), 0.5);
        }
    }

    #[test]
    fn startup_returns_are_sixteen_empty_init_chains_for_every_registry() {
        use crate::adapter::chorus_capability::ChorusCapability;
        use crate::adapter::chorus_preparer::ChorusPreparer;
        use crate::synth::{compose_effect_registry, EffectCapabilityProvider, EffectPreparer};

        let empty = compose_effect_registry(&[], &[]).unwrap();
        let providers: Vec<Box<dyn EffectCapabilityProvider>> =
            vec![Box::new(ChorusCapability::new().unwrap())];
        let preparers: Vec<Box<dyn EffectPreparer>> =
            vec![Box::new(ChorusPreparer::new().unwrap())];
        let partial = compose_effect_registry(&providers, &preparers).unwrap();
        let full = production_effect_registry().unwrap();

        for registry in [&empty, &partial, &full] {
            let bank = super::production_startup_bus_returns(registry).unwrap();
            assert_eq!(bank.len(), 16);
            for (index, bus_return) in bank.returns().iter().enumerate() {
                assert_eq!(bus_return.id().index(), index);
                assert_eq!(bus_return.name(), "INIT");
                assert!(bus_return.effects().is_empty());
                assert_eq!(bus_return.return_level(), 0.5);
            }
            assert_eq!(super::startup_bus_returns(registry), bank);
        }

        // Legacy demo composition still requires its declared effects.
        assert!(matches!(
            super::production_default_bus_returns(&partial),
            Err(super::ProductionEffectCompositionError::ReturnOccupancy(_))
        ));
    }

    #[test]
    fn providers_and_preparers_stay_paired() {
        let providers = production_effect_providers().unwrap();
        let preparers = production_effect_preparers().unwrap();

        assert_eq!(providers.len(), preparers.len());
        for (provider, preparer) in providers.iter().zip(&preparers) {
            assert_eq!(provider.descriptor().id(), preparer.capability_id());
        }
    }

    #[test]
    fn default_occupancy_prepares_reverb_and_delay_with_their_descriptor_defaults() {
        let prepared = super::prepare_production_default_bus_returns(48_000.0, 64).unwrap();

        assert_eq!(prepared.len(), 2);
        assert_eq!(
            prepared[0].bus,
            crate::mixer::bus_id::BusId::new(0).unwrap()
        );
        assert_eq!(
            prepared[1].bus,
            crate::mixer::bus_id::BusId::new(1).unwrap()
        );
        assert_eq!(
            prepared[0].parameters.slot_id(),
            Some(EffectSlotId::new(1).unwrap())
        );
        assert_eq!(
            prepared[1].parameters.slot_id(),
            Some(EffectSlotId::new(2).unwrap())
        );
        assert_eq!(prepared[0].parameters.scalar_count(), 2);
        assert_eq!(prepared[1].parameters.scalar_count(), 2);
        for occupant in &prepared {
            assert_eq!(occupant.return_level, 0.5);
        }
    }

    /// The retired mixer-owned `GlobalReverbDelay` implementation, transcribed
    /// verbatim as the migration-fidelity reference (moved here from the
    /// deleted bridge file `adapter/global_reverb_delay.rs`). The racked
    /// production returns must match it sample for sample on identical input.
    mod retired_reference {
        /// The retired global values, carried explicitly now that
        /// `GlobalParameters` holds only master gain. The defaults are the
        /// production descriptor defaults the racked returns install.
        #[derive(Clone, Copy)]
        pub struct RetiredGlobalValues {
            pub reverb_room_size: f32,
            pub reverb_damping: f32,
            pub reverb_return: f32,
            pub delay_milliseconds: f32,
            pub delay_feedback: f32,
            pub delay_return: f32,
        }

        impl RetiredGlobalValues {
            pub const PRODUCTION_DEFAULTS: Self = Self {
                reverb_room_size: 0.5,
                reverb_damping: 0.5,
                reverb_return: 0.5,
                delay_milliseconds: 250.0,
                delay_feedback: 0.5,
                delay_return: 0.5,
            };
        }

        const REVERB_LEFT_MILLISECONDS: f32 = 29.7;
        const REVERB_RIGHT_MILLISECONDS: f32 = 37.1;

        /// The retired shared reverb, split per return now that the retired
        /// two-input port is deleted: the coupling of the two returns
        /// was only structural — the reverb and delay states were always
        /// independent — so processing them through separate rack returns
        /// changes no arithmetic. Overwrites the buffer with wet only; the
        /// bus-return rack applies the return level and the single wet sum,
        /// preserving the retired `output + (wet_a + wet_b)` association.
        pub struct RetiredReverbReturn {
            reverb: StereoReverb,
        }

        impl RetiredReverbReturn {
            pub fn prepared(sample_rate: f32) -> Self {
                Self {
                    reverb: StereoReverb::prepared(sample_rate),
                }
            }

            pub fn process_wet(&mut self, interleaved: &mut [f32], values: &RetiredGlobalValues) {
                let frame_count = interleaved.len() / 2;
                for frame in 0..frame_count {
                    let left = frame * 2;
                    let right = left + 1;
                    let (wet_left, wet_right) = self.reverb.process(
                        interleaved[left],
                        interleaved[right],
                        values.reverb_room_size,
                        values.reverb_damping,
                    );
                    interleaved[left] = wet_left;
                    interleaved[right] = wet_right;
                }
            }
        }

        /// The retired shared delay as its own rack return; see
        /// [`RetiredReverbReturn`].
        pub struct RetiredDelayReturn {
            delay: StereoFeedbackDelay,
        }

        impl RetiredDelayReturn {
            pub fn prepared(sample_rate: f32, max_delay_milliseconds: f32) -> Self {
                Self {
                    delay: StereoFeedbackDelay::prepared(sample_rate, max_delay_milliseconds),
                }
            }

            pub fn process_wet(&mut self, interleaved: &mut [f32], values: &RetiredGlobalValues) {
                let frame_count = interleaved.len() / 2;
                for frame in 0..frame_count {
                    let left = frame * 2;
                    let right = left + 1;
                    let (wet_left, wet_right) = self.delay.process(
                        interleaved[left],
                        interleaved[right],
                        values.delay_milliseconds,
                        values.delay_feedback,
                    );
                    interleaved[left] = wet_left;
                    interleaved[right] = wet_right;
                }
            }
        }

        #[derive(Default)]
        struct StereoReverb {
            left_buffer: Vec<f32>,
            right_buffer: Vec<f32>,
            left_index: usize,
            right_index: usize,
            left_damping_state: f32,
            right_damping_state: f32,
        }

        impl StereoReverb {
            fn prepared(sample_rate: f32) -> Self {
                let left_length = milliseconds_to_samples(sample_rate, REVERB_LEFT_MILLISECONDS);
                let right_length = milliseconds_to_samples(sample_rate, REVERB_RIGHT_MILLISECONDS);

                Self {
                    left_buffer: vec![0.0; left_length],
                    right_buffer: vec![0.0; right_length],
                    left_index: 0,
                    right_index: 0,
                    left_damping_state: 0.0,
                    right_damping_state: 0.0,
                }
            }

            fn process(
                &mut self,
                left_input: f32,
                right_input: f32,
                room_size: f32,
                damping: f32,
            ) -> (f32, f32) {
                let left_delayed = self.left_buffer[self.left_index];
                let right_delayed = self.right_buffer[self.right_index];
                let undamped = 1.0 - damping;

                let left_filtered = left_delayed * undamped + self.left_damping_state * damping;
                let right_filtered = right_delayed * undamped + self.right_damping_state * damping;
                self.left_damping_state = left_filtered;
                self.right_damping_state = right_filtered;

                let feedback = 0.2 + room_size * 0.75;
                self.left_buffer[self.left_index] = left_input + left_filtered * feedback;
                self.right_buffer[self.right_index] = right_input + right_filtered * feedback;

                self.left_index += 1;
                if self.left_index == self.left_buffer.len() {
                    self.left_index = 0;
                }
                self.right_index += 1;
                if self.right_index == self.right_buffer.len() {
                    self.right_index = 0;
                }

                (left_filtered, right_filtered)
            }
        }

        #[derive(Default)]
        struct StereoFeedbackDelay {
            left_buffer: Vec<f32>,
            right_buffer: Vec<f32>,
            write_index: usize,
            sample_rate: f32,
        }

        impl StereoFeedbackDelay {
            fn prepared(sample_rate: f32, max_delay_milliseconds: f32) -> Self {
                let max_delay_samples =
                    milliseconds_to_samples(sample_rate, max_delay_milliseconds);
                let buffer_length = max_delay_samples + 1;

                Self {
                    left_buffer: vec![0.0; buffer_length],
                    right_buffer: vec![0.0; buffer_length],
                    write_index: 0,
                    sample_rate,
                }
            }

            fn process(
                &mut self,
                left_input: f32,
                right_input: f32,
                delay_milliseconds: f32,
                feedback: f32,
            ) -> (f32, f32) {
                let requested_delay =
                    (delay_milliseconds * self.sample_rate / 1000.0).round() as usize;
                let delay_samples = requested_delay.clamp(1, self.left_buffer.len() - 1);
                let read_index = (self.write_index + self.left_buffer.len() - delay_samples)
                    % self.left_buffer.len();

                let left_delayed = self.left_buffer[read_index];
                let right_delayed = self.right_buffer[read_index];

                self.left_buffer[self.write_index] = left_input + left_delayed * feedback;
                self.right_buffer[self.write_index] = right_input + right_delayed * feedback;

                self.write_index += 1;
                if self.write_index == self.left_buffer.len() {
                    self.write_index = 0;
                }

                (left_delayed, right_delayed)
            }
        }

        fn milliseconds_to_samples(sample_rate: f32, milliseconds: f32) -> usize {
            let sample_count = (f64::from(sample_rate) * f64::from(milliseconds) / 1000.0).ceil();
            (sample_count as usize).max(1)
        }
    }

    /// Rack-installable shims speaking the retired DSP verbatim, standing in
    /// for the deleted two-input port as the comparison baseline. Each shim
    /// runs at the retired production defaults regardless of live scalar
    /// values, exactly as the retired mixer-owned processor did.
    struct RetiredReverbShim {
        inner: retired_reference::RetiredReverbReturn,
        slot_id: EffectSlotId,
    }

    impl crate::synth::PreparedPostEffect for RetiredReverbShim {
        fn patch_id(&self) -> PatchId {
            PatchId::new(u32::MAX).unwrap()
        }

        fn slot_id(&self) -> EffectSlotId {
            self.slot_id
        }

        fn process(
            &mut self,
            interleaved_stereo: &mut [f32],
            frame_count: usize,
            _parameters: &crate::real_time::RtPostEffectParameters,
        ) -> Result<(), crate::synth::PreparedEffectError> {
            self.inner.process_wet(
                &mut interleaved_stereo[..frame_count * 2],
                &retired_reference::RetiredGlobalValues::PRODUCTION_DEFAULTS,
            );
            Ok(())
        }
    }

    struct RetiredDelayShim {
        inner: retired_reference::RetiredDelayReturn,
        slot_id: EffectSlotId,
    }

    impl crate::synth::PreparedPostEffect for RetiredDelayShim {
        fn patch_id(&self) -> PatchId {
            PatchId::new(u32::MAX).unwrap()
        }

        fn slot_id(&self) -> EffectSlotId {
            self.slot_id
        }

        fn process(
            &mut self,
            interleaved_stereo: &mut [f32],
            frame_count: usize,
            _parameters: &crate::real_time::RtPostEffectParameters,
        ) -> Result<(), crate::synth::PreparedEffectError> {
            self.inner.process_wet(
                &mut interleaved_stereo[..frame_count * 2],
                &retired_reference::RetiredGlobalValues::PRODUCTION_DEFAULTS,
            );
            Ok(())
        }
    }

    fn full_mix_snapshot(
        global: crate::mixer::global_parameters::GlobalParameters,
    ) -> crate::real_time::ParameterSnapshot {
        use crate::mixer::mixer_state::MixerState;
        use crate::mixer::mixer_track_id::MixerTrackId;
        use crate::mixer::mixer_track_parameters::MixerTrackParameters;
        use crate::mixer::patch_output::PatchOutput;
        use crate::real_time::RtPatchParameters;

        let registry = super::production_effect_registry().unwrap();
        let bank = super::production_default_bus_returns(&registry).unwrap();
        let returns =
            crate::real_time::ParameterSnapshot::project_returns(&registry, &bank).unwrap();
        crate::real_time::ParameterSnapshot::new(
            1,
            global,
            MixerState::default().with_track(
                MixerTrackId::default(),
                MixerTrackParameters::from_values(
                    -6.0,
                    0.25,
                    false,
                    false,
                    [0.7, 0.55, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                )
                .unwrap(),
            ),
            &[RtPatchParameters::new(
                PatchId::new(1).unwrap(),
                PatchOutput::default(),
            )],
        )
        .unwrap()
        .with_returns(returns)
    }

    /// Migration-fidelity proof (successor of the deleted bridge's
    /// `bus_return_rack_matches_the_bridge_sample_for_sample`): the production
    /// mix path — default return occupancy, live scalar values from the
    /// snapshot's indexed return entries — must match the retired mixer-owned
    /// processor sample for sample on identical stems, for every rendered
    /// block. Both paths run through the one bus-return rack now that the
    /// retired two-input port is deleted; the retired DSP rides
    /// per-return shims, preserving the retired `output + (wet_a + wet_b)`
    /// association. This is also the double-wet evidence: the wet signal
    /// appears exactly once.
    #[test]
    fn racked_default_returns_match_the_retired_processor_sample_for_sample() {
        use crate::mixer::bus_id::BusId;
        use crate::mixer::global_parameters::GlobalParameters;
        use crate::mixer::mix_engine::MixEngine;
        use crate::real_time::PatchAudioBlock;

        const SAMPLE_RATE: f32 = 1_000.0;
        const FRAME_COUNT: usize = 64;
        const BLOCKS: usize = 8;
        /// The delay bound the engine handed the retired port in `prepare`.
        const RETIRED_MAX_DELAY_MILLISECONDS: f32 = 2000.0;

        let parameter_sets = [
            GlobalParameters::new(0.0).unwrap(),
            GlobalParameters::new(0.0).unwrap(),
            GlobalParameters::new(-3.0).unwrap(),
            GlobalParameters::new(0.0).unwrap(),
        ];

        for global in parameter_sets {
            let snapshot = full_mix_snapshot(global);
            let patch_id = PatchId::new(1).unwrap();

            let mut retired = MixEngine::new();
            retired.prepare(SAMPLE_RATE, FRAME_COUNT).unwrap();
            let attested = super::prepare_production_default_bus_returns(SAMPLE_RATE, FRAME_COUNT)
                .unwrap()
                .into_iter()
                .map(|occupant| (occupant.bus, occupant.parameters, occupant.return_level))
                .collect::<Vec<_>>();
            // The retired return levels now live on the rack: the production
            // bank installs exactly the values the retired processor used.
            let retired_values = retired_reference::RetiredGlobalValues::PRODUCTION_DEFAULTS;
            for (bus, parameters, return_level) in &attested {
                let expected_level = if *bus == BusId::new(0).unwrap() {
                    retired_values.reverb_return
                } else {
                    retired_values.delay_return
                };
                assert_eq!(*return_level, expected_level);
                let effect: Box<dyn crate::synth::PreparedPostEffect> =
                    if *bus == BusId::new(0).unwrap() {
                        Box::new(RetiredReverbShim {
                            inner: retired_reference::RetiredReverbReturn::prepared(SAMPLE_RATE),
                            slot_id: parameters.slot_id().unwrap(),
                        })
                    } else {
                        Box::new(RetiredDelayShim {
                            inner: retired_reference::RetiredDelayReturn::prepared(
                                SAMPLE_RATE,
                                RETIRED_MAX_DELAY_MILLISECONDS,
                            ),
                            slot_id: parameters.slot_id().unwrap(),
                        })
                    };
                retired
                    .install_bus_return(*bus, effect, parameters.clone(), *return_level)
                    .unwrap();
            }

            let mut racked = MixEngine::new();
            racked.prepare(SAMPLE_RATE, FRAME_COUNT).unwrap();
            for occupant in
                super::prepare_production_default_bus_returns(SAMPLE_RATE, FRAME_COUNT).unwrap()
            {
                racked
                    .install_bus_return(
                        occupant.bus,
                        occupant.effect,
                        occupant.parameters,
                        occupant.return_level,
                    )
                    .unwrap();
            }

            // Deterministic pseudo-random stems shared by both paths.
            let mut state: u32 = 0x51F0_A9C3;
            let mut noise = move || {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 8) as f32 / 16_777_216.0 - 0.5
            };

            for block_index in 0..BLOCKS {
                let stem: Vec<f32> = (0..FRAME_COUNT * 2).map(|_| noise()).collect();
                let mut block = PatchAudioBlock::prepare(FRAME_COUNT).unwrap();
                block.begin_render(&snapshot, FRAME_COUNT).unwrap();
                block.stem_mut(0, patch_id).unwrap().copy_from_slice(&stem);

                let mut retired_output = vec![0.0; FRAME_COUNT * 2];
                let mut racked_output = vec![0.0; FRAME_COUNT * 2];
                retired.mix(&block, &snapshot, &mut retired_output);
                racked.mix(&block, &snapshot, &mut racked_output);

                assert_eq!(
                    retired_output, racked_output,
                    "block {block_index}: the racked returns must sound identical to the retired processor"
                );
                assert!(racked_output.iter().any(|sample| sample.abs() > 0.0));
            }
        }
    }

    /// Moved from the deleted bridge file: every MIXER global row — master
    /// gain plus the six return-owned values it aliases — must still alter
    /// measured output through the production racked path. The return-owned
    /// values travel as the snapshot's indexed return entries.
    #[test]
    fn global_effects_parameter_sensitivity() {
        use crate::mixer::global_parameters::GlobalParameters;
        use crate::mixer::mix_engine::MixEngine;
        use crate::real_time::{ParameterSnapshot, PatchAudioBlock, RtBusReturnParameters};

        fn render(global: GlobalParameters, returns: Vec<RtBusReturnParameters>) -> Vec<f32> {
            const FRAME_COUNT: usize = 256;
            let patch_id = PatchId::new(1).unwrap();
            let snapshot = full_mix_snapshot(global).with_returns(returns);
            let mut block = PatchAudioBlock::prepare(FRAME_COUNT).unwrap();
            block.begin_render(&snapshot, FRAME_COUNT).unwrap();
            let stem = block.stem_mut(0, patch_id).unwrap();
            stem[0] = 0.2;
            stem[1] = -0.1;

            let mut mixer = MixEngine::new();
            mixer.prepare(1_000.0, FRAME_COUNT).unwrap();
            for occupant in
                super::prepare_production_default_bus_returns(1_000.0, FRAME_COUNT).unwrap()
            {
                mixer
                    .install_bus_return(
                        occupant.bus,
                        occupant.effect,
                        occupant.parameters,
                        occupant.return_level,
                    )
                    .unwrap();
            }
            let mut output = vec![0.0; FRAME_COUNT * 2];
            mixer.mix(&block, &snapshot, &mut output);
            output
        }

        fn with_scalar(
            returns: Vec<RtBusReturnParameters>,
            bus: usize,
            position: usize,
            value: f32,
        ) -> Vec<RtBusReturnParameters> {
            let mut edited = returns.clone();
            let entry = &returns[bus];
            let mut scalars: Vec<f32> = entry.scalars().to_vec();
            scalars[position] = value;
            edited[bus] = RtBusReturnParameters::new(
                entry.slot_id().unwrap(),
                &scalars,
                entry.return_level(),
            )
            .unwrap();
            edited
        }

        fn with_level(
            returns: Vec<RtBusReturnParameters>,
            bus: usize,
            value: f32,
        ) -> Vec<RtBusReturnParameters> {
            let mut edited = returns.clone();
            let entry = &returns[bus];
            edited[bus] =
                RtBusReturnParameters::new(entry.slot_id().unwrap(), entry.scalars(), value)
                    .unwrap();
            edited
        }

        let registry = super::production_effect_registry().unwrap();
        let bank = super::production_default_bus_returns(&registry).unwrap();
        // The retired baseline values, expressed as live return entries: a
        // short 10 ms delay keeps feedback recurrences inside one block.
        let mut defaults = ParameterSnapshot::project_returns(&registry, &bank).unwrap();
        defaults = with_scalar(defaults, 0, 1, 0.3);
        defaults = with_level(defaults, 0, 0.6);
        defaults = with_scalar(defaults, 1, 0, 10.0);
        defaults = with_scalar(defaults, 1, 1, 0.4);
        defaults = with_level(defaults, 1, 0.6);
        let baseline = GlobalParameters::new(0.0).unwrap();
        let baseline_output = render(baseline, defaults.clone());

        let variants: [(&str, GlobalParameters, Vec<RtBusReturnParameters>); 7] = [
            (
                "masterGainDb",
                GlobalParameters::new(6.0).unwrap(),
                defaults.clone(),
            ),
            (
                "returns[0].scalars[0] (room size)",
                baseline,
                with_scalar(defaults.clone(), 0, 0, 0.6),
            ),
            (
                "returns[0].scalars[1] (damping)",
                baseline,
                with_scalar(defaults.clone(), 0, 1, 0.4),
            ),
            (
                "returns[0].returnLevel",
                baseline,
                with_level(defaults.clone(), 0, 0.7),
            ),
            (
                "returns[1].scalars[0] (milliseconds)",
                baseline,
                with_scalar(defaults.clone(), 1, 0, 110.0),
            ),
            (
                "returns[1].scalars[1] (feedback)",
                baseline,
                with_scalar(defaults.clone(), 1, 1, 0.5),
            ),
            (
                "returns[1].returnLevel",
                baseline,
                with_level(defaults, 1, 0.7),
            ),
        ];
        for (name, global, returns) in variants {
            let variant_output = render(global, returns);
            assert!(
                baseline_output
                    .iter()
                    .zip(&variant_output)
                    .any(|(left, right)| (left - right).abs() > 1.0e-7),
                "{name} must change measured output from identical fresh effect state"
            );
        }
    }
}
