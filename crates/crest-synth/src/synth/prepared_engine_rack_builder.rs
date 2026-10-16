use crate::kernel::patch_id::PatchId;
use crate::real_time::parameter_snapshot::MAX_ACTIVE_PATCHES;
use crate::real_time::prepared_engine_rack::{PreparedEngineRack, PreparedEngineSlot};
use crate::synth::capability_id::CapabilityId;
use crate::synth::instrument_capability::{CapabilityError, CapabilityRegistry};
use crate::synth::instrument_preparer::{InstrumentPreparationError, InstrumentPreparer};
use crate::synth::patch::Patch;
use core::fmt;
use std::collections::HashSet;

const MAX_PREPARED_ASSET_GRAPH_BYTES: usize = crate::synth::MAX_SAMPLE_GRAPH_PCM_BYTES;

/// Builds one complete fixed-capacity prepared rack outside the callback.
pub struct PreparedEngineRackBuilder;

impl PreparedEngineRackBuilder {
    /// Resolves every accepted Patch through the immutable registry and exactly
    /// one capability-matched preparer. Any failure drops temporary prepared
    /// values on the calling control/worker thread and returns no partial rack.
    pub fn build(
        patches: &[Patch],
        registry: &CapabilityRegistry,
        preparers: &[Box<dyn InstrumentPreparer>],
        sample_rate: f32,
        max_frames: usize,
    ) -> Result<PreparedEngineRack, RackPreparationError> {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err(RackPreparationError::InvalidSampleRate);
        }
        if max_frames == 0 {
            return Err(RackPreparationError::InvalidFrameCapacity);
        }
        if patches.len() > MAX_ACTIVE_PATCHES {
            return Err(RackPreparationError::PatchCapacityExceeded {
                count: patches.len(),
                capacity: MAX_ACTIVE_PATCHES,
            });
        }

        for (index, patch) in patches.iter().enumerate() {
            if patches[..index]
                .iter()
                .any(|prior| prior.id() == patch.id())
            {
                return Err(RackPreparationError::DuplicatePatchId {
                    patch_id: patch.id(),
                });
            }
            registry
                .validate_config(patch.instrument_config())
                .map_err(|source| RackPreparationError::InvalidConfiguration {
                    patch_id: patch.id(),
                    source,
                })?;
        }

        for (index, preparer) in preparers.iter().enumerate() {
            if preparers[..index]
                .iter()
                .any(|prior| prior.capability_id() == preparer.capability_id())
            {
                return Err(RackPreparationError::DuplicatePreparer {
                    capability_id: preparer.capability_id().clone(),
                });
            }
            if registry.descriptor(preparer.capability_id()).is_none() {
                return Err(RackPreparationError::ExtraPreparer {
                    capability_id: preparer.capability_id().clone(),
                });
            }
        }

        let mut slots = std::array::from_fn(|_| None);
        let mut prepared_asset_identities = HashSet::new();
        let mut prepared_asset_bytes = 0_usize;
        for (index, patch) in patches.iter().enumerate() {
            let capability_id = patch.instrument_config().capability_id();
            let scalar_count = registry
                .descriptor_for_config(patch.instrument_config())
                .expect("validated Patch capability is installed")
                .scalar_parameter_count();
            let Some(preparer) = preparers
                .iter()
                .find(|preparer| preparer.capability_id() == capability_id)
            else {
                return Err(RackPreparationError::MissingPreparer {
                    capability_id: capability_id.clone(),
                });
            };

            let instrument =
                preparer
                    .prepare(patch, sample_rate, max_frames)
                    .map_err(|source| RackPreparationError::Instrument {
                        patch_id: patch.id(),
                        source,
                    })?;
            let actual = instrument.patch_id();
            if actual != patch.id() {
                return Err(RackPreparationError::PreparedPatchMismatch {
                    expected: patch.id(),
                    actual,
                });
            }
            for footprint in instrument.prepared_asset_footprints() {
                let identity = (footprint.reference().clone(), footprint.preparation_key());
                {
                    let shared = if prepared_asset_identities.insert(identity) {
                        footprint.bytes()
                    } else {
                        0
                    };
                    prepared_asset_bytes = prepared_asset_bytes
                        .checked_add(shared)
                        .and_then(|bytes| bytes.checked_add(footprint.private_bytes()))
                        .ok_or(RackPreparationError::PreparedAssetCapacityExceeded {
                            bytes: usize::MAX,
                            capacity: MAX_PREPARED_ASSET_GRAPH_BYTES,
                        })?;
                    if prepared_asset_bytes > MAX_PREPARED_ASSET_GRAPH_BYTES {
                        return Err(RackPreparationError::PreparedAssetCapacityExceeded {
                            bytes: prepared_asset_bytes,
                            capacity: MAX_PREPARED_ASSET_GRAPH_BYTES,
                        });
                    }
                }
            }
            slots[index] = Some(PreparedEngineSlot::new(
                patch.id(),
                scalar_count,
                instrument,
            ));
        }

        Ok(PreparedEngineRack::from_slots(patches.len(), slots))
    }
}

/// A control/worker-side atomic rack preparation failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RackPreparationError {
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
        capability_id: CapabilityId,
    },
    MissingPreparer {
        capability_id: CapabilityId,
    },
    ExtraPreparer {
        capability_id: CapabilityId,
    },
    InvalidConfiguration {
        patch_id: PatchId,
        source: CapabilityError,
    },
    Instrument {
        patch_id: PatchId,
        source: InstrumentPreparationError,
    },
    PreparedPatchMismatch {
        expected: PatchId,
        actual: PatchId,
    },
    PreparedAssetCapacityExceeded {
        bytes: usize,
        capacity: usize,
    },
}

impl fmt::Display for RackPreparationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSampleRate => {
                formatter.write_str("rack sample rate must be finite and positive")
            }
            Self::InvalidFrameCapacity => {
                formatter.write_str("rack frame capacity must be nonzero")
            }
            Self::PatchCapacityExceeded { count, capacity } => write!(
                formatter,
                "rack has {count} Patches; fixed capacity is {capacity}"
            ),
            Self::DuplicatePatchId { patch_id } => write!(formatter, "duplicate Patch {patch_id}"),
            Self::DuplicatePreparer { capability_id } => {
                write!(formatter, "duplicate preparer for {capability_id}")
            }
            Self::MissingPreparer { capability_id } => {
                write!(formatter, "no preparer is installed for {capability_id}")
            }
            Self::ExtraPreparer { capability_id } => {
                write!(formatter, "preparer {capability_id} has no accepted Patch")
            }
            Self::InvalidConfiguration { patch_id, source } => {
                write!(formatter, "Patch {patch_id} config is invalid: {source}")
            }
            Self::Instrument { patch_id, source } => {
                write!(formatter, "Patch {patch_id} preparation failed: {source}")
            }
            Self::PreparedPatchMismatch { expected, actual } => write!(
                formatter,
                "preparer returned Patch {actual} for expected Patch {expected}"
            ),
            Self::PreparedAssetCapacityExceeded { bytes, capacity } => write!(
                formatter,
                "prepared shared assets use {bytes} bytes; graph capacity is {capacity}"
            ),
        }
    }
}

impl std::error::Error for RackPreparationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidConfiguration { source, .. } => Some(source),
            Self::Instrument { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PreparedEngineRackBuilder, RackPreparationError};
    use crate::adapter::hidef_soundfont_capability::HIDEF_CAPABILITY_ID;
    use crate::kernel::midi_channel::MidiChannel;
    use crate::kernel::midi_message::{MidiMessage, MidiMessageKind};
    use crate::kernel::patch_id::PatchId;
    use crate::mixer::global_parameters::GlobalParameters;
    use crate::mixer::mixer_state::MixerState;
    use crate::mixer::mixer_track_id::MixerTrackId;
    use crate::mixer::patch_output::PatchOutput;
    use crate::real_time::parameter_snapshot::{
        ParameterSnapshot, RtPatchParameters, MAX_ACTIVE_PATCHES,
    };
    use crate::real_time::patch_audio_block::PatchAudioBlock;
    use crate::real_time::prepared_engine_rack::{RackDispatchError, RackRenderError};
    use crate::synth::capability_id::CapabilityId;
    use crate::synth::instrument_capability::{CapabilityRegistry, InstrumentConfig};
    use crate::synth::instrument_preparer::{InstrumentPreparationError, InstrumentPreparer};
    use crate::synth::patch::Patch;
    use crate::synth::prepared_instrument::{PreparedInstrument, PreparedInstrumentError};
    use crate::synth::sound_font_instrument::SoundFontInstrument;
    use crate::testing::automatic_midi_test::create_soundfont_config;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn patch(id: u32) -> Patch {
        let provider =
            crate::adapter::production_instruments::production_soundfont_capability().unwrap();
        Patch::new(
            PatchId::new(id).unwrap(),
            format!("Patch {id}"),
            create_soundfont_config(
                &provider,
                SoundFontInstrument::new(0, (id % 8) as u8, false).unwrap(),
            )
            .unwrap(),
            MidiChannel::new((id % 16) as u8).unwrap(),
            PatchOutput::to_track(MixerTrackId::new((id % 16) as u8).unwrap()),
        )
    }

    fn registry() -> CapabilityRegistry {
        crate::adapter::production_instruments::production_soundfont_capability()
            .unwrap()
            .registry()
            .unwrap()
    }

    fn message() -> MidiMessage {
        MidiMessage::try_new(
            MidiChannel::new(0).unwrap(),
            MidiMessageKind::NoteOn,
            60,
            100,
        )
        .unwrap()
    }

    fn global_parameters() -> GlobalParameters {
        GlobalParameters::new(0.0).unwrap()
    }

    struct FixturePreparer {
        capability_id: CapabilityId,
        alpha_dispatches: Arc<AtomicUsize>,
        beta_dispatches: Arc<AtomicUsize>,
        drops: Arc<AtomicUsize>,
        fail_patch: Option<PatchId>,
        mismatched_patch: Option<PatchId>,
    }

    impl FixturePreparer {
        fn new(
            alpha_dispatches: Arc<AtomicUsize>,
            beta_dispatches: Arc<AtomicUsize>,
            drops: Arc<AtomicUsize>,
        ) -> Self {
            Self {
                capability_id: CapabilityId::new(HIDEF_CAPABILITY_ID).unwrap(),
                alpha_dispatches,
                beta_dispatches,
                drops,
                fail_patch: None,
                mismatched_patch: None,
            }
        }
    }

    impl InstrumentPreparer for FixturePreparer {
        fn capability_id(&self) -> &CapabilityId {
            &self.capability_id
        }

        fn prepare(
            &self,
            patch: &Patch,
            _sample_rate: f32,
            _max_frames: usize,
        ) -> Result<Box<dyn PreparedInstrument>, InstrumentPreparationError> {
            if self.fail_patch == Some(patch.id()) {
                return Err(InstrumentPreparationError::PreparationFailed {
                    patch_id: patch.id(),
                });
            }
            let patch_id = self.mismatched_patch.unwrap_or(patch.id());
            if patch.id().value() % 2 == 1 {
                Ok(Box::new(AlphaInstrument {
                    patch_id,
                    dispatches: Arc::clone(&self.alpha_dispatches),
                    drops: Arc::clone(&self.drops),
                }))
            } else {
                Ok(Box::new(BetaInstrument {
                    patch_id,
                    dispatches: Arc::clone(&self.beta_dispatches),
                    drops: Arc::clone(&self.drops),
                }))
            }
        }
    }

    struct AlphaInstrument {
        patch_id: PatchId,
        dispatches: Arc<AtomicUsize>,
        drops: Arc<AtomicUsize>,
    }

    impl PreparedInstrument for AlphaInstrument {
        fn patch_id(&self) -> PatchId {
            self.patch_id
        }

        fn dispatch(
            &mut self,
            _message: MidiMessage,
            _parameters: &crate::real_time::RtPatchParameters,
        ) -> Result<(), PreparedInstrumentError> {
            self.dispatches.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }

        fn render(
            &mut self,
            output: &mut [f32],
            _frame_count: usize,
            _parameters: &crate::real_time::RtPatchParameters,
        ) -> Result<(), crate::synth::PreparedInstrumentError> {
            output.fill(0.25);

            Ok(())
        }

        fn all_notes_off(&mut self) {
            self.dispatches.store(0, Ordering::Relaxed);
        }
    }

    impl Drop for AlphaInstrument {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::Relaxed);
        }
    }

    struct BetaInstrument {
        patch_id: PatchId,
        dispatches: Arc<AtomicUsize>,
        drops: Arc<AtomicUsize>,
    }

    impl PreparedInstrument for BetaInstrument {
        fn patch_id(&self) -> PatchId {
            self.patch_id
        }

        fn dispatch(
            &mut self,
            _message: MidiMessage,
            _parameters: &crate::real_time::RtPatchParameters,
        ) -> Result<(), PreparedInstrumentError> {
            self.dispatches.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }

        fn render(
            &mut self,
            output: &mut [f32],
            _frame_count: usize,
            _parameters: &crate::real_time::RtPatchParameters,
        ) -> Result<(), crate::synth::PreparedInstrumentError> {
            output.fill(-0.5);

            Ok(())
        }

        fn all_notes_off(&mut self) {
            self.dispatches.store(0, Ordering::Relaxed);
        }
    }

    impl Drop for BetaInstrument {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn heterogeneous_slots_route_and_render_only_their_exact_patch_stems() {
        let alpha_dispatches = Arc::new(AtomicUsize::new(0));
        let beta_dispatches = Arc::new(AtomicUsize::new(0));
        let drops = Arc::new(AtomicUsize::new(0));
        let preparers: Vec<Box<dyn InstrumentPreparer>> = vec![Box::new(FixturePreparer::new(
            Arc::clone(&alpha_dispatches),
            Arc::clone(&beta_dispatches),
            Arc::clone(&drops),
        ))];
        let patches = [patch(1), patch(2)];
        let mut rack =
            PreparedEngineRackBuilder::build(&patches, &registry(), &preparers, 48_000.0, 8)
                .unwrap();

        let projected = [
            RtPatchParameters::new(patches[0].id(), patches[0].output()),
            RtPatchParameters::new(patches[1].id(), patches[1].output()),
        ];
        rack.dispatch(patches[1].id(), message(), &projected[1])
            .unwrap();
        assert_eq!(alpha_dispatches.load(Ordering::Relaxed), 0);
        assert_eq!(beta_dispatches.load(Ordering::Relaxed), 1);

        let parameters =
            ParameterSnapshot::new(1, global_parameters(), MixerState::default(), &projected)
                .unwrap();
        let mut block = PatchAudioBlock::prepare(8).unwrap();
        block.begin_render(&parameters, 4).unwrap();
        rack.render(&mut block, &parameters).unwrap();

        assert_eq!(block.stem(0, patches[0].id()).unwrap().samples(), [0.25; 8]);
        assert_eq!(block.stem(1, patches[1].id()).unwrap().samples(), [-0.5; 8]);
        assert!(rack.matches_parameters(&parameters));

        let unknown = PatchId::new(99).unwrap();
        assert_eq!(
            rack.dispatch(unknown, message(), &projected[0]),
            Err(RackDispatchError::UnknownPatch { patch_id: unknown })
        );
        assert_eq!(alpha_dispatches.load(Ordering::Relaxed), 0);
        assert_eq!(beta_dispatches.load(Ordering::Relaxed), 1);

        rack.all_notes_off_for(patches[1].id()).unwrap();
        assert_eq!(beta_dispatches.load(Ordering::Relaxed), 0);
        rack.dispatch(patches[0].id(), message(), &projected[0])
            .unwrap();
        rack.dispatch(patches[1].id(), message(), &projected[1])
            .unwrap();
        rack.all_notes_off();
        assert_eq!(alpha_dispatches.load(Ordering::Relaxed), 0);
        assert_eq!(beta_dispatches.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn rack_rejects_mismatched_caller_owned_stems_before_rendering() {
        let preparers: Vec<Box<dyn InstrumentPreparer>> = vec![Box::new(FixturePreparer::new(
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
        ))];
        let patches = [patch(1), patch(2)];
        let mut rack =
            PreparedEngineRackBuilder::build(&patches, &registry(), &preparers, 48_000.0, 8)
                .unwrap();
        let reversed = ParameterSnapshot::new(
            1,
            global_parameters(),
            MixerState::default(),
            &[
                RtPatchParameters::new(patches[1].id(), patches[1].output()),
                RtPatchParameters::new(patches[0].id(), patches[0].output()),
            ],
        )
        .unwrap();
        let mut block = PatchAudioBlock::prepare(8).unwrap();
        block.begin_render(&reversed, 4).unwrap();

        assert_eq!(
            rack.render(&mut block, &reversed),
            Err(RackRenderError::ParameterLayoutMismatch)
        );
        assert!(block
            .stems()
            .iter()
            .all(|stem| stem.samples().iter().all(|sample| *sample == 0.0)));
    }

    #[test]
    fn builder_rejects_invalid_matching_capacity_and_identity_inputs() {
        let counters = || Arc::new(AtomicUsize::new(0));
        let make_preparer = || -> Box<dyn InstrumentPreparer> {
            Box::new(FixturePreparer::new(counters(), counters(), counters()))
        };
        let one = [patch(1)];

        assert!(matches!(
            PreparedEngineRackBuilder::build(&one, &registry(), &[make_preparer()], 0.0, 8),
            Err(RackPreparationError::InvalidSampleRate)
        ));
        assert!(matches!(
            PreparedEngineRackBuilder::build(&one, &registry(), &[make_preparer()], 48_000.0, 0),
            Err(RackPreparationError::InvalidFrameCapacity)
        ));
        assert!(matches!(
            PreparedEngineRackBuilder::build(
                &[patch(1), patch(1)],
                &registry(),
                &[make_preparer()],
                48_000.0,
                8
            ),
            Err(RackPreparationError::DuplicatePatchId { .. })
        ));
        assert!(matches!(
            PreparedEngineRackBuilder::build(&one, &registry(), &[], 48_000.0, 8),
            Err(RackPreparationError::MissingPreparer { .. })
        ));
        assert!(matches!(
            PreparedEngineRackBuilder::build(
                &one,
                &registry(),
                &[make_preparer(), make_preparer()],
                48_000.0,
                8,
            ),
            Err(RackPreparationError::DuplicatePreparer { .. })
        ));

        let other_preparer: Box<dyn InstrumentPreparer> = Box::new(OtherPreparer {
            capability_id: CapabilityId::new("instrument.test.other").unwrap(),
        });
        assert!(matches!(
            PreparedEngineRackBuilder::build(&one, &registry(), &[other_preparer], 48_000.0, 8),
            Err(RackPreparationError::ExtraPreparer { .. })
        ));

        let too_many: Vec<_> = (1..=(MAX_ACTIVE_PATCHES + 1))
            .map(|id| patch(id as u32))
            .collect();
        assert!(matches!(
            PreparedEngineRackBuilder::build(
                &too_many,
                &registry(),
                &[make_preparer()],
                48_000.0,
                8,
            ),
            Err(RackPreparationError::PatchCapacityExceeded { .. })
        ));

        let malformed = Patch::new(
            PatchId::new(7).unwrap(),
            "Malformed".to_owned(),
            InstrumentConfig::from_parts(
                CapabilityId::new(HIDEF_CAPABILITY_ID).unwrap(),
                Vec::new(),
                Vec::new(),
            ),
            MidiChannel::new(0).unwrap(),
            PatchOutput::default(),
        );
        assert!(matches!(
            PreparedEngineRackBuilder::build(
                &[malformed],
                &registry(),
                &[make_preparer()],
                48_000.0,
                8,
            ),
            Err(RackPreparationError::InvalidConfiguration { .. })
        ));

        let mut mismatched = FixturePreparer::new(counters(), counters(), counters());
        mismatched.mismatched_patch = Some(PatchId::new(2).unwrap());
        let mismatched: Vec<Box<dyn InstrumentPreparer>> = vec![Box::new(mismatched)];
        assert!(matches!(
            PreparedEngineRackBuilder::build(&one, &registry(), &mismatched, 48_000.0, 8),
            Err(RackPreparationError::PreparedPatchMismatch { .. })
        ));
    }

    struct OtherPreparer {
        capability_id: CapabilityId,
    }

    impl InstrumentPreparer for OtherPreparer {
        fn capability_id(&self) -> &CapabilityId {
            &self.capability_id
        }

        fn prepare(
            &self,
            patch: &Patch,
            _sample_rate: f32,
            _max_frames: usize,
        ) -> Result<Box<dyn PreparedInstrument>, InstrumentPreparationError> {
            Err(InstrumentPreparationError::UnsupportedCapability {
                patch_id: patch.id(),
            })
        }
    }

    #[test]
    fn partial_preparation_is_destroyed_before_atomic_failure_returns() {
        let drops = Arc::new(AtomicUsize::new(0));
        let mut preparer = FixturePreparer::new(
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
            Arc::clone(&drops),
        );
        preparer.fail_patch = Some(PatchId::new(2).unwrap());
        let preparers: Vec<Box<dyn InstrumentPreparer>> = vec![Box::new(preparer)];

        let error = PreparedEngineRackBuilder::build(
            &[patch(1), patch(2)],
            &registry(),
            &preparers,
            48_000.0,
            8,
        )
        .unwrap_err();

        assert!(matches!(error, RackPreparationError::Instrument { .. }));
        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }

    struct FootprintPreparer {
        capability_id: CapabilityId,
        shared_identity: bool,
    }

    impl InstrumentPreparer for FootprintPreparer {
        fn capability_id(&self) -> &CapabilityId {
            &self.capability_id
        }

        fn prepare(
            &self,
            patch: &Patch,
            _sample_rate: f32,
            _max_frames: usize,
        ) -> Result<Box<dyn PreparedInstrument>, InstrumentPreparationError> {
            let locator = if self.shared_identity {
                "shared.wav".to_owned()
            } else {
                format!("patch-{}.wav", patch.id().value())
            };
            Ok(Box::new(FootprintInstrument {
                patch_id: patch.id(),
                footprint: crate::synth::PreparedAssetFootprint::new(
                    crate::synth::AssetReference::new(crate::synth::AssetKind::Sample, locator)
                        .unwrap(),
                    48_000,
                    300 * 1024 * 1024,
                ),
            }))
        }
    }

    struct FootprintInstrument {
        patch_id: PatchId,
        footprint: crate::synth::PreparedAssetFootprint,
    }

    impl PreparedInstrument for FootprintInstrument {
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
            output.fill(0.0);

            Ok(())
        }

        fn all_notes_off(&mut self) {}

        fn prepared_asset_footprint(&self) -> Option<&crate::synth::PreparedAssetFootprint> {
            Some(&self.footprint)
        }
    }

    #[test]
    fn complete_rack_deduplicates_shared_preparation_identity_and_rejects_aggregate_overflow() {
        let patches = [patch(1), patch(2)];
        let preparers: Vec<Box<dyn InstrumentPreparer>> = vec![Box::new(FootprintPreparer {
            capability_id: CapabilityId::new(HIDEF_CAPABILITY_ID).unwrap(),
            shared_identity: true,
        })];
        PreparedEngineRackBuilder::build(&patches, &registry(), &preparers, 48_000.0, 8).unwrap();

        let preparers: Vec<Box<dyn InstrumentPreparer>> = vec![Box::new(FootprintPreparer {
            capability_id: CapabilityId::new(HIDEF_CAPABILITY_ID).unwrap(),
            shared_identity: false,
        })];
        assert!(matches!(
            PreparedEngineRackBuilder::build(&patches, &registry(), &preparers, 48_000.0, 8),
            Err(RackPreparationError::PreparedAssetCapacityExceeded { .. })
        ));
    }
}
