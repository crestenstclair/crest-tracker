use crate::mixer::bus_id::BusId;
use crate::mixer::bus_return::BusReturnBank;
use crate::mixer::bus_return::EffectError;
use crate::mixer::mix_engine::MixEngine;
use crate::real_time::graph_revision::GraphRevision;
use crate::real_time::parameter_snapshot::ParameterSnapshot;
use crate::real_time::patch_audio_block::{PatchAudioBlock, PatchAudioBlockError};
use crate::real_time::prepared_graph::{
    PositionCapabilityIdentity, PreparedAuditionSlot, PreparedGraph, PreparedGraphResources,
};
use crate::synth::instrument_capability::CapabilityRegistry;
use crate::synth::instrument_preparer::{InstrumentPreparationError, InstrumentPreparer};
use crate::synth::patch::Patch;
use crate::synth::prepared_engine_rack_builder::{PreparedEngineRackBuilder, RackPreparationError};
use crate::synth::EffectPreparationError;
use crate::synth::{
    EffectCapabilityRegistry, EffectPreparer, EffectRackPreparationError, InstrumentConfig,
    PreparedPostEffectRackBuilder,
};
use core::fmt;
use std::collections::HashSet;

/// Control/worker-side composition service for one complete prepared graph.
pub struct PreparedGraphBuilder<'a> {
    registry: &'a CapabilityRegistry,
    preparers: &'a [Box<dyn InstrumentPreparer>],
    effect_registry: Option<&'a EffectCapabilityRegistry>,
    effect_preparers: &'a [Box<dyn EffectPreparer>],
    returns: Option<&'a BusReturnBank>,
    audition: Option<&'a AuditionPreparationRequest>,
}

/// Worker-owned input for the complete graph's one optional preview slot.
pub struct AuditionPreparationRequest {
    identity: u64,
    patch_id: crate::kernel::PatchId,
    candidate: InstrumentConfig,
}

impl AuditionPreparationRequest {
    pub fn new(
        identity: u64,
        patch_id: crate::kernel::PatchId,
        candidate: InstrumentConfig,
    ) -> Result<Self, GraphPreparationError> {
        if identity == 0 {
            return Err(GraphPreparationError::InvalidAuditionIdentity);
        }
        Ok(Self {
            identity,
            patch_id,
            candidate,
        })
    }
}

impl<'a> PreparedGraphBuilder<'a> {
    pub const fn new(
        registry: &'a CapabilityRegistry,
        preparers: &'a [Box<dyn InstrumentPreparer>],
    ) -> Self {
        Self {
            registry,
            preparers,
            effect_registry: None,
            effect_preparers: &[],
            returns: None,
            audition: None,
        }
    }

    /// Injects the complete installed effect registry and its exact preparers.
    pub const fn with_effects(
        mut self,
        registry: &'a EffectCapabilityRegistry,
        preparers: &'a [Box<dyn EffectPreparer>],
    ) -> Self {
        self.effect_registry = Some(registry);
        self.effect_preparers = preparers;
        self
    }

    /// Injects the canonical bus-return occupancy this graph must prepare.
    ///
    /// Every occupied return is prepared through the injected effect registry
    /// and preparers and installed at its exact bus. Without a bank the
    /// prepared return rack stays empty — occupancy is never installed
    /// implicitly.
    pub const fn with_returns(mut self, returns: &'a BusReturnBank) -> Self {
        self.returns = Some(returns);
        self
    }

    pub const fn with_audition(mut self, audition: &'a AuditionPreparationRequest) -> Self {
        self.audition = Some(audition);
        self
    }

    /// Prepares every graph owner and buffer before returning one atomic
    /// ownership unit. Any error destroys only candidate values on this
    /// control/worker call stack.
    pub fn build(
        &self,
        revision: GraphRevision,
        patches: &[Patch],
        parameters: ParameterSnapshot,
        sample_rate: f32,
        max_frames: usize,
    ) -> Result<PreparedGraph, GraphPreparationError> {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err(GraphPreparationError::InvalidSampleRate);
        }
        if max_frames == 0 {
            return Err(GraphPreparationError::InvalidFrameCapacity);
        }
        if parameters.graph_revision() != revision {
            return Err(GraphPreparationError::RevisionMismatch {
                graph: revision,
                parameters: parameters.graph_revision(),
            });
        }
        if parameters.patch_count() != patches.len()
            || parameters
                .patches()
                .iter()
                .zip(patches)
                .any(|(parameters, patch)| parameters.patch_id() != Some(patch.id()))
            || parameters.patches().iter().any(|patch| {
                patch
                    .sends()
                    .iter()
                    .skip(parameters.returns().len())
                    .any(|send| *send != 0.0)
            })
        {
            return Err(GraphPreparationError::ParameterLayoutMismatch);
        }

        let mut engine_rack = PreparedEngineRackBuilder::build(
            patches,
            self.registry,
            self.preparers,
            sample_rate,
            max_frames,
        )
        .map_err(GraphPreparationError::Rack)?;
        if !engine_rack.matches_parameters(&parameters) {
            return Err(GraphPreparationError::ParameterLayoutMismatch);
        }
        // Record every engine position's capability identity from the
        // validated candidate configuration. Carry-over later requires exact
        // per-position identity agreement, so the identity is captured here
        // at build time, never re-derived.
        for (index, patch) in patches.iter().enumerate() {
            let identity = PositionCapabilityIdentity::from_capability_id(
                patch.instrument_config().capability_id(),
            )
            .ok_or(GraphPreparationError::UnrecordableCapabilityIdentity)?;
            if !engine_rack.record_capability_identity(index, patch.id(), identity) {
                return Err(GraphPreparationError::ParameterLayoutMismatch);
            }
        }

        let empty_effect_registry = EffectCapabilityRegistry::default();
        let effect_registry = self.effect_registry.unwrap_or(&empty_effect_registry);
        let mut effect_rack = PreparedPostEffectRackBuilder::build(
            patches,
            effect_registry,
            self.effect_preparers,
            sample_rate,
            max_frames,
        )
        .map_err(GraphPreparationError::EffectRack)?;
        if !effect_rack.matches_parameters(&parameters) {
            return Err(GraphPreparationError::ParameterLayoutMismatch);
        }
        // Record every occupied effect position's capability identity from
        // the candidate's canonical per-position chain; empty positions stay
        // explicitly empty.
        for (index, patch) in patches.iter().enumerate() {
            for (position, occupant) in patch.effect_slots().iter().enumerate() {
                let Some(config) = occupant else {
                    continue;
                };
                let identity =
                    PositionCapabilityIdentity::from_effect_capability_id(config.capability_id())
                        .ok_or(GraphPreparationError::UnrecordableCapabilityIdentity)?;
                if !effect_rack.record_capability_identity_at(
                    index,
                    position,
                    patch.id(),
                    config.slot_id(),
                    identity,
                ) {
                    return Err(GraphPreparationError::ParameterLayoutMismatch);
                }
            }
        }

        let patch_audio =
            PatchAudioBlock::prepare(max_frames).map_err(GraphPreparationError::PatchAudio)?;
        let mut mixer = MixEngine::new();
        mixer
            .prepare_with_returns(sample_rate, max_frames, parameters.returns().len())
            .map_err(GraphPreparationError::Effects)?;
        // Return occupancy comes exclusively from the injected canonical
        // bank. `prepare` empties the rack, so occupancy is (re-)installed
        // after every preparation; a bank the injected registry or preparers
        // cannot satisfy refuses the complete graph with the failing bus.
        if let Some(bank) = self.returns {
            for bus_return in bank.returns() {
                for config in bus_return.effects() {
                    let bus = bus_return.id();
                    let descriptor = effect_registry.descriptor(config.capability_id()).ok_or(
                        GraphPreparationError::BusReturn {
                            bus,
                            source: BusReturnPreparationError::UnknownRegistryEntry,
                        },
                    )?;
                    let invalid = |_| GraphPreparationError::BusReturn {
                        bus,
                        source: BusReturnPreparationError::InvalidConfiguration,
                    };
                    let scalars = descriptor
                        .scalar_parameters()
                        .map(|spec| {
                            let value = config.value(spec.id()).ok_or(
                                GraphPreparationError::BusReturn {
                                    bus,
                                    source: BusReturnPreparationError::InvalidConfiguration,
                                },
                            )?;
                            spec.scalar_value(value).map_err(invalid)
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    let live =
                        crate::real_time::RtPostEffectParameters::new(config.slot_id(), &scalars)
                            .map_err(|_| GraphPreparationError::BusReturn {
                            bus,
                            source: BusReturnPreparationError::InvalidConfiguration,
                        })?;
                    let preparer = self
                        .effect_preparers
                        .iter()
                        .find(|preparer| preparer.capability_id() == config.capability_id())
                        .ok_or(GraphPreparationError::BusReturn {
                            bus,
                            source: BusReturnPreparationError::MissingPreparer,
                        })?;
                    let prepared = preparer
                        .prepare(RETURN_OCCUPANT_PATCH_ID, config, sample_rate, max_frames)
                        .map_err(|source| GraphPreparationError::BusReturn {
                            bus,
                            source: BusReturnPreparationError::Preparation(source),
                        })?;
                    mixer
                        .bus_returns_mut()
                        .append(bus, prepared, live, bus_return.return_level())
                        .map_err(|source| GraphPreparationError::BusReturn {
                            bus,
                            source: BusReturnPreparationError::Install(source),
                        })?;
                    // Record the occupant's capability identity from the
                    // validated candidate bank entry, exactly as the engine and
                    // effect positions record theirs.
                    let identity = PositionCapabilityIdentity::from_effect_capability_id(
                        config.capability_id(),
                    )
                    .ok_or(GraphPreparationError::UnrecordableCapabilityIdentity)?;
                    if !mixer.bus_returns_mut().record_occupant_identity(
                        bus,
                        config.slot_id(),
                        identity,
                    ) {
                        return Err(GraphPreparationError::ParameterLayoutMismatch);
                    }
                }
            }
        }
        if !mixer.bus_returns().matches_parameters(&parameters) {
            return Err(GraphPreparationError::ParameterLayoutMismatch);
        }

        let prepared_audition = if let Some(audition) = self.audition {
            if !patches.iter().any(|patch| patch.id() == audition.patch_id)
                || self.registry.validate_config(&audition.candidate).is_err()
            {
                return Err(GraphPreparationError::InvalidAuditionConfiguration);
            }
            let preparer = self
                .preparers
                .iter()
                .find(|preparer| preparer.capability_id() == audition.candidate.capability_id())
                .ok_or(GraphPreparationError::AuditionPreparerMissing)?;
            let prepared = preparer
                .prepare_audition(
                    audition.patch_id,
                    &audition.candidate,
                    sample_rate,
                    max_frames,
                )
                .map_err(GraphPreparationError::Audition)?;
            if prepared.patch_id() != audition.patch_id {
                return Err(GraphPreparationError::InvalidAuditionConfiguration);
            }
            let mut identities = HashSet::new();
            let mut bytes = 0_usize;
            for footprint in engine_rack
                .prepared_asset_footprints()
                .chain(prepared.prepared_asset_footprint())
            {
                let identity = (footprint.reference().clone(), footprint.preparation_key());
                {
                    let shared = if identities.insert(identity) {
                        footprint.bytes()
                    } else {
                        0
                    };
                    bytes = bytes
                        .checked_add(shared)
                        .and_then(|bytes| bytes.checked_add(footprint.private_bytes()))
                        .ok_or({
                            GraphPreparationError::Rack(
                                RackPreparationError::PreparedAssetCapacityExceeded {
                                    bytes: usize::MAX,
                                    capacity: crate::synth::MAX_SAMPLE_GRAPH_PCM_BYTES,
                                },
                            )
                        })?;
                    if bytes > crate::synth::MAX_SAMPLE_GRAPH_PCM_BYTES {
                        return Err(GraphPreparationError::Rack(
                            RackPreparationError::PreparedAssetCapacityExceeded {
                                bytes,
                                capacity: crate::synth::MAX_SAMPLE_GRAPH_PCM_BYTES,
                            },
                        ));
                    }
                }
            }
            Some(PreparedAuditionSlot::new(audition.identity, prepared))
        } else {
            None
        };
        let resources = PreparedGraphResources::new(engine_rack, effect_rack, patch_audio, mixer);
        let resources = if let Some(audition) = prepared_audition {
            resources.with_audition(audition)
        } else {
            resources
        };

        Ok(PreparedGraph::new(
            revision,
            sample_rate,
            max_frames,
            parameters,
            resources,
        ))
    }
}

/// The stable synthetic Patch identity carried by prepared return occupants.
///
/// Bus returns are not owned by any Patch; the prepared-effect port requires
/// a Patch identity, so returns use the reserved maximum, exactly as the
/// retired production bridge did.
const RETURN_OCCUPANT_PATCH_ID: crate::kernel::PatchId = match crate::kernel::PatchId::new(u32::MAX)
{
    Ok(patch_id) => patch_id,
    Err(_) => panic!("the reserved bus-return Patch id is non-zero"),
};

/// A typed atomic complete-graph preparation failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GraphPreparationError {
    InvalidSampleRate,
    InvalidFrameCapacity,
    InvalidAuditionIdentity,
    InvalidAuditionConfiguration,
    AuditionPreparerMissing,
    Audition(InstrumentPreparationError),
    RevisionMismatch {
        graph: GraphRevision,
        parameters: GraphRevision,
    },
    ParameterLayoutMismatch,
    /// One prepared position's capability identity exceeds the fixed
    /// per-position identity record; the candidate is refused, never
    /// truncated.
    UnrecordableCapabilityIdentity,
    Rack(RackPreparationError),
    EffectRack(EffectRackPreparationError),
    PatchAudio(PatchAudioBlockError),
    Effects(EffectError),
    /// One bus-return occupant could not be prepared; the failure names its
    /// exact bus so the refusal stays attributable to that position.
    BusReturn {
        bus: BusId,
        source: BusReturnPreparationError,
    },
}

/// Why one bus-return occupant refused preparation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BusReturnPreparationError {
    /// The occupying entry is not installed in the injected effect registry.
    UnknownRegistryEntry,
    /// The occupant's configuration does not satisfy its descriptor.
    InvalidConfiguration,
    /// No injected preparer accepts the occupying registry entry.
    MissingPreparer,
    /// The preparer refused to build the instance.
    Preparation(EffectPreparationError),
    /// The prepared instance could not be installed into the return rack.
    Install(EffectError),
}

impl fmt::Display for BusReturnPreparationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownRegistryEntry => {
                formatter.write_str("occupying entry is not in the effect registry")
            }
            Self::InvalidConfiguration => {
                formatter.write_str("occupant configuration does not satisfy its descriptor")
            }
            Self::MissingPreparer => formatter.write_str("no preparer accepts the occupying entry"),
            Self::Preparation(source) => write!(formatter, "preparation refused: {source}"),
            Self::Install(source) => write!(formatter, "installation refused: {source}"),
        }
    }
}

impl fmt::Display for GraphPreparationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSampleRate => {
                formatter.write_str("prepared graph sample rate must be finite and positive")
            }
            Self::InvalidFrameCapacity => {
                formatter.write_str("prepared graph frame capacity must be nonzero")
            }
            Self::InvalidAuditionIdentity => {
                formatter.write_str("audition identity must be nonzero")
            }
            Self::InvalidAuditionConfiguration => {
                formatter.write_str("audition configuration is incompatible with its origin Patch")
            }
            Self::AuditionPreparerMissing => {
                formatter.write_str("no installed preparer supports the audition capability")
            }
            Self::Audition(source) => write!(formatter, "prepared audition failed: {source}"),
            Self::RevisionMismatch { graph, parameters } => write!(
                formatter,
                "prepared graph revision {graph} does not match parameters {parameters}"
            ),
            Self::ParameterLayoutMismatch => formatter.write_str(
                "prepared graph Patch count or ordered identities do not match parameters",
            ),
            Self::UnrecordableCapabilityIdentity => formatter.write_str(
                "a prepared position's capability identity exceeds the fixed identity record",
            ),
            Self::Rack(source) => write!(formatter, "prepared engine rack failed: {source}"),
            Self::EffectRack(source) => {
                write!(formatter, "prepared post-effect rack failed: {source}")
            }
            Self::PatchAudio(source) => {
                write!(formatter, "Patch stem preparation failed: {source}")
            }
            Self::Effects(source) => {
                write!(formatter, "global effect preparation failed: {source}")
            }
            Self::BusReturn { bus, source } => {
                write!(formatter, "bus return {bus} preparation failed: {source}")
            }
        }
    }
}

impl std::error::Error for GraphPreparationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Rack(source) => Some(source),
            Self::Audition(source) => Some(source),
            Self::EffectRack(source) => Some(source),
            Self::PatchAudio(source) => Some(source),
            Self::Effects(source) => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{GraphPreparationError, PreparedGraphBuilder};
    use crate::adapter::braids_capability::BRAIDS_CAPABILITY_ID;
    use crate::adapter::hidef_soundfont_capability::HIDEF_CAPABILITY_ID;
    use crate::adapter::production_effects::{
        production_effect_preparers, production_effect_registry,
    };
    use crate::adapter::production_instruments::{
        production_capability_registry, production_instrument_preparers,
        production_instrument_providers,
    };
    use crate::kernel::midi_channel::MidiChannel;
    use crate::kernel::midi_message::MidiMessage;
    use crate::kernel::patch_id::PatchId;
    use crate::mixer::bus_id::BusId;
    use crate::mixer::bus_return::BusReturnBank;
    use crate::mixer::global_parameters::GlobalParameters;
    use crate::mixer::mixer_state::MixerState;
    use crate::mixer::mixer_track_id::MixerTrackId;
    use crate::mixer::patch_output::PatchOutput;
    use crate::real_time::graph_revision::{GraphRevision, GraphRevisionError};
    use crate::real_time::parameter_snapshot::{ParameterSnapshot, RtPatchParameters};
    use crate::real_time::prepared_graph::MAX_CAPABILITY_IDENTITY_BYTES;
    use crate::real_time::PreparedGraphRefreshError;
    use crate::synth::capability_id::CapabilityId;
    use crate::synth::effect_slot_id::EffectSlotIndex;
    use crate::synth::instrument_capability::{CapabilityDescriptor, CapabilityRegistry};
    use crate::synth::instrument_capability_provider::InstrumentCapabilityProvider;
    use crate::synth::instrument_preparer::{InstrumentPreparationError, InstrumentPreparer};
    use crate::synth::patch::Patch;
    use crate::synth::prepared_engine_rack_builder::RackPreparationError;
    use crate::synth::prepared_instrument::{PreparedInstrument, PreparedInstrumentError};
    use crate::synth::sound_font_instrument::SoundFontInstrument;
    use crate::synth::{DescriptorDefaultConfigFactory, EffectCapabilityId, EffectSlotId};
    use crate::testing::automatic_midi_test::create_soundfont_config;

    fn patch(id: u32) -> Patch {
        let provider =
            crate::adapter::production_instruments::production_soundfont_capability().unwrap();
        Patch::new(
            PatchId::new(id).unwrap(),
            format!("Patch {id}"),
            create_soundfont_config(
                &provider,
                SoundFontInstrument::new(0, (id - 1) as u8, false).unwrap(),
            )
            .unwrap(),
            MidiChannel::new((id - 1) as u8).unwrap(),
            PatchOutput::to_track(MixerTrackId::new((id - 1) as u8).unwrap()),
        )
    }

    fn globals() -> GlobalParameters {
        GlobalParameters::new(0.0).unwrap()
    }

    fn parameters(revision: GraphRevision, patches: &[Patch]) -> ParameterSnapshot {
        let patches: Vec<_> = patches
            .iter()
            .map(|patch| RtPatchParameters::new(patch.id(), patch.output()))
            .collect();
        ParameterSnapshot::for_graph(1, revision, globals(), MixerState::default(), &patches)
            .unwrap()
    }

    struct FixturePreparer {
        capability_id: CapabilityId,
        fail: bool,
    }

    impl FixturePreparer {
        fn boxed(fail: bool) -> Box<dyn InstrumentPreparer> {
            Self::boxed_for(CapabilityId::new(HIDEF_CAPABILITY_ID).unwrap(), fail)
        }

        fn boxed_for(capability_id: CapabilityId, fail: bool) -> Box<dyn InstrumentPreparer> {
            Box::new(Self {
                capability_id,
                fail,
            })
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
            if self.fail {
                Err(InstrumentPreparationError::PreparationFailed {
                    patch_id: patch.id(),
                })
            } else {
                Ok(Box::new(FixtureInstrument {
                    patch_id: patch.id(),
                }))
            }
        }
    }

    struct FixtureInstrument {
        patch_id: PatchId,
    }

    impl PreparedInstrument for FixtureInstrument {
        fn patch_id(&self) -> PatchId {
            self.patch_id
        }

        fn dispatch(
            &mut self,
            _message: MidiMessage,
            _parameters: &crate::real_time::RtPatchParameters,
        ) -> Result<(), PreparedInstrumentError> {
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

        fn all_notes_off(&mut self) {}
    }

    /// The builder records the exact capability identity of every prepared
    /// position for a mixed configuration — two different engines, duplicate
    /// effect entries in two slots with a gap between them, and one occupied
    /// return among empty ones — with an explicit empty everywhere
    /// unoccupied. The recorded identities surface identically through the
    /// racks and through the replacement layout the coordinator attests.
    #[test]
    fn builder_records_carry_over_capability_identity_for_every_position() {
        let registry = production_capability_registry().unwrap();
        let preparers = production_instrument_preparers().unwrap();
        let effect_registry = production_effect_registry().unwrap();
        let effect_preparers = production_effect_preparers().unwrap();

        let chorus_at = |slot_id: u16| {
            effect_registry
                .descriptors()
                .iter()
                .find(|descriptor| descriptor.id().as_str() == "effect.chorus")
                .expect("production chorus entry exists")
                .default_config(EffectSlotId::new(slot_id).unwrap())
                .unwrap()
        };
        let mut first = patch(1);
        first
            .set_slot_occupancy(EffectSlotIndex::new(0).unwrap(), Some(chorus_at(1)))
            .unwrap();
        first
            .set_slot_occupancy(EffectSlotIndex::new(2).unwrap(), Some(chorus_at(2)))
            .unwrap();
        let second = Patch::new(
            PatchId::new(2).unwrap(),
            "Patch 2".to_owned(),
            DescriptorDefaultConfigFactory::new(
                production_capability_registry().unwrap(),
                production_instrument_providers().unwrap(),
            )
            .create(&CapabilityId::new(BRAIDS_CAPABILITY_ID).unwrap())
            .unwrap(),
            MidiChannel::new(1).unwrap(),
            PatchOutput::to_track(MixerTrackId::new(1).unwrap()),
        );
        let patches = [first, second];

        let mut bank = BusReturnBank::default();
        let reverb_bus = BusId::new(2).unwrap();
        bank.set_return_occupancy(
            &effect_registry,
            reverb_bus,
            Some(&EffectCapabilityId::new("effect.reverb").unwrap()),
        )
        .unwrap();

        let revision = GraphRevision::new(7).unwrap();
        let parameters = ParameterSnapshot::project_patches_with_effects_and_returns(
            1,
            revision,
            globals(),
            MixerState::default(),
            &patches,
            &registry,
            &effect_registry,
            &bank,
        )
        .unwrap();
        let graph = PreparedGraphBuilder::new(&registry, &preparers)
            .with_effects(&effect_registry, &effect_preparers)
            .with_returns(&bank)
            .build(revision, &patches, parameters, 48_000.0, 128)
            .unwrap();

        // Engine positions: each Patch records its own engine capability,
        // and positions beyond the Patch count stay explicitly empty.
        let engine_rack = graph.engine_rack();
        assert_eq!(
            engine_rack.capability_identity(0).unwrap().as_str(),
            HIDEF_CAPABILITY_ID
        );
        assert_eq!(
            engine_rack.capability_identity(1).unwrap().as_str(),
            BRAIDS_CAPABILITY_ID
        );
        assert_eq!(engine_rack.capability_identity(2), None);

        // Effect grid: both duplicate entries record the shared capability at
        // their own positions; the gap and the effect-free Patch stay empty.
        let effect_rack = graph.effect_rack();
        assert_eq!(
            effect_rack.capability_identity_at(0, 0).unwrap().as_str(),
            "effect.chorus"
        );
        assert_eq!(effect_rack.capability_identity_at(0, 1), None);
        assert_eq!(
            effect_rack.capability_identity_at(0, 2).unwrap().as_str(),
            "effect.chorus"
        );
        for position in 0..crate::synth::effect_slot_id::MAX_EFFECT_SLOTS {
            assert_eq!(effect_rack.capability_identity_at(1, position), None);
        }

        // Bus returns: the occupied return records its occupant; every empty
        // return stays explicitly empty.
        let returns = graph.bus_return_rack();
        assert_eq!(
            returns.capability_identity(reverb_bus).unwrap().as_str(),
            "effect.reverb"
        );
        for bus in BusId::ALL {
            if bus != reverb_bus {
                assert_eq!(returns.capability_identity(bus), None);
            }
        }

        // The replacement layout carries the same per-position record.
        let layout = graph.layout();
        assert_eq!(
            layout.engine_capability_identity(0).unwrap().as_str(),
            HIDEF_CAPABILITY_ID
        );
        assert_eq!(
            layout.engine_capability_identity(1).unwrap().as_str(),
            BRAIDS_CAPABILITY_ID
        );
        assert_eq!(
            layout.effect_capability_identity(0, 2).unwrap().as_str(),
            "effect.chorus"
        );
        assert_eq!(layout.effect_capability_identity(0, 1), None);
        assert_eq!(
            layout
                .return_capability_identity(reverb_bus.index())
                .unwrap()
                .as_str(),
            "effect.reverb"
        );
        assert_eq!(layout.return_capability_identity(0), None);
    }

    /// Builds one complete graph whose single Patch declares `identifier` as
    /// its engine capability.
    ///
    /// The descriptor is the production SoundFont descriptor carrying a
    /// different identity, so the recorded capability identity is the only
    /// thing that varies between the at-capacity and beyond-capacity runs —
    /// every other preparation step sees the same inputs.
    fn build_with_engine_capability(
        identifier: &str,
        revision: GraphRevision,
    ) -> Result<super::PreparedGraph, GraphPreparationError> {
        let provider =
            crate::adapter::production_instruments::production_soundfont_capability().unwrap();
        let canonical = provider.descriptor();
        let capability_id = CapabilityId::new(identifier).unwrap();
        let descriptor = CapabilityDescriptor::new(
            capability_id.clone(),
            canonical.label(),
            canonical.semantic_accent(),
            canonical.sections().to_vec(),
            canonical.asset_requirements().to_vec(),
            canonical.voice_policy(),
            canonical.supported_midi_kinds().to_vec(),
        )
        .unwrap();
        let canonical_config =
            create_soundfont_config(&provider, SoundFontInstrument::new(0, 0, false).unwrap())
                .unwrap();
        let config = descriptor
            .create_config(
                canonical_config.values(),
                canonical_config.asset_references(),
            )
            .unwrap();
        assert_eq!(config.capability_id().as_str(), identifier);
        let patches = [Patch::new(
            PatchId::new(1).unwrap(),
            "Boundary capability Patch".to_owned(),
            config,
            MidiChannel::new(0).unwrap(),
            PatchOutput::to_track(MixerTrackId::new(0).unwrap()),
        )];
        let registry = CapabilityRegistry::new(vec![descriptor]).unwrap();
        let preparers = vec![FixturePreparer::boxed_for(capability_id, false)];
        PreparedGraphBuilder::new(&registry, &preparers).build(
            revision,
            &patches,
            parameters(revision, &patches),
            48_000.0,
            128,
        )
    }

    /// A capability identity one byte past the fixed per-position record
    /// refuses the whole graph instead of recording a truncated one, and the
    /// same configuration at exactly the capacity builds and records the
    /// identity in full.
    ///
    /// Pinning both sides of the edge is what makes truncation
    /// non-reintroducible: two capabilities sharing a
    /// `MAX_CAPABILITY_IDENTITY_BYTES` prefix would compare equal under a
    /// truncating record, so a live instance from the wrong capability would
    /// be carried over. The refusal is whole-graph — `build` returns no graph
    /// at all, so no position can reach a carry-over decision holding a
    /// truncated or missing identity.
    #[test]
    fn carry_over_capability_identity_beyond_the_record_refuses_the_whole_graph() {
        const PREFIX: &str = "instrument.fixture.";
        let at_capacity = format!(
            "{PREFIX}{}",
            "a".repeat(MAX_CAPABILITY_IDENTITY_BYTES - PREFIX.len())
        );
        let beyond_capacity = format!("{at_capacity}b");
        assert_eq!(at_capacity.len(), MAX_CAPABILITY_IDENTITY_BYTES);
        assert_eq!(beyond_capacity.len(), MAX_CAPABILITY_IDENTITY_BYTES + 1);

        let revision = GraphRevision::new(3).unwrap();

        // Exactly at the capacity the graph builds, and both the rack and the
        // layout the coordinator attests carry the complete identity.
        let recorded = build_with_engine_capability(&at_capacity, revision).unwrap();
        assert_eq!(
            recorded
                .engine_rack()
                .capability_identity(0)
                .unwrap()
                .as_str(),
            at_capacity
        );
        assert_eq!(
            recorded
                .layout()
                .engine_capability_identity(0)
                .unwrap()
                .as_str(),
            at_capacity
        );

        // One byte beyond the capacity the complete graph is refused.
        assert_eq!(
            build_with_engine_capability(&beyond_capacity, revision).unwrap_err(),
            GraphPreparationError::UnrecordableCapabilityIdentity
        );

        // The refusal stays attributable through its own visible surface.
        assert_eq!(
            GraphPreparationError::UnrecordableCapabilityIdentity.to_string(),
            "a prepared position's capability identity exceeds the fixed identity record"
        );
    }

    #[test]
    fn builder_returns_one_complete_ordered_callback_ready_graph() {
        let provider =
            crate::adapter::production_instruments::production_soundfont_capability().unwrap();
        let registry = provider.registry().unwrap();
        let preparers = vec![FixturePreparer::boxed(false)];
        let builder = PreparedGraphBuilder::new(&registry, &preparers);
        let revision = GraphRevision::new(7).unwrap();
        let patches = [patch(1), patch(2)];

        let graph = builder
            .build(
                revision,
                &patches,
                parameters(revision, &patches),
                48_000.0,
                256,
            )
            .unwrap();

        assert_eq!(graph.revision(), revision);
        assert_eq!(graph.sample_rate(), 48_000.0);
        assert_eq!(graph.max_frames(), 256);
        assert_eq!(graph.engine_rack().patch_count(), 2);
        assert_eq!(graph.engine_rack().patch_id(0), Some(patches[0].id()));
        assert_eq!(graph.engine_rack().patch_id(1), Some(patches[1].id()));
        assert_eq!(graph.patch_audio().max_frames(), 256);
        assert!(graph
            .engine_rack()
            .matches_parameters(graph.initial_parameters()));
    }

    #[test]
    fn prepared_graph_refreshes_only_an_exact_target_revision_and_engine_layout() {
        let provider =
            crate::adapter::production_instruments::production_soundfont_capability().unwrap();
        let registry = provider.registry().unwrap();
        let preparers = vec![FixturePreparer::boxed(false)];
        let builder = PreparedGraphBuilder::new(&registry, &preparers);
        let revision = GraphRevision::new(7).unwrap();
        let patches = [patch(1), patch(2)];
        let mut graph = builder
            .build(
                revision,
                &patches,
                parameters(revision, &patches),
                48_000.0,
                256,
            )
            .unwrap();

        let mut edited_patches = patches.clone();
        edited_patches[0]
            .set_output(PatchOutput::new(MixerTrackId::new(3).unwrap(), -9.0).unwrap());
        let refreshed = parameters(revision, &edited_patches).with_generation(99);
        graph.refresh_initial_parameters(refreshed.clone()).unwrap();
        assert_eq!(graph.initial_parameters(), &refreshed);
        assert_eq!(
            graph
                .initial_parameters()
                .patch(edited_patches[0].id())
                .unwrap()
                .output(),
            edited_patches[0].output()
        );

        let retained = graph.initial_parameters().clone();
        assert_eq!(
            graph.refresh_initial_parameters(parameters(GraphRevision::new(8).unwrap(), &patches)),
            Err(PreparedGraphRefreshError::RevisionMismatch)
        );
        assert_eq!(graph.initial_parameters(), &retained);
        assert_eq!(
            graph.refresh_initial_parameters(parameters(revision, &[patch(2), patch(1)])),
            Err(PreparedGraphRefreshError::LayoutMismatch)
        );
        assert_eq!(graph.initial_parameters(), &retained);
    }

    #[test]
    fn candidate_failures_leave_an_existing_graph_unchanged() {
        let provider =
            crate::adapter::production_instruments::production_soundfont_capability().unwrap();
        let registry = provider.registry().unwrap();
        let good_preparers = vec![FixturePreparer::boxed(false)];
        let builder = PreparedGraphBuilder::new(&registry, &good_preparers);
        let first_revision = GraphRevision::new(1).unwrap();
        let patches = [patch(1), patch(2)];
        let existing = builder
            .build(
                first_revision,
                &patches,
                parameters(first_revision, &patches),
                48_000.0,
                64,
            )
            .unwrap();

        assert_eq!(GraphRevision::new(0), Err(GraphRevisionError::Zero));
        assert!(matches!(
            builder.build(
                GraphRevision::new(2).unwrap(),
                &patches,
                parameters(first_revision, &patches),
                48_000.0,
                64,
            ),
            Err(GraphPreparationError::RevisionMismatch { .. })
        ));
        assert!(matches!(
            builder.build(
                first_revision,
                &patches,
                parameters(first_revision, &[patch(2), patch(1)]),
                48_000.0,
                64,
            ),
            Err(GraphPreparationError::ParameterLayoutMismatch)
        ));
        assert_eq!(
            builder
                .build(
                    first_revision,
                    &patches,
                    parameters(first_revision, &patches),
                    f32::NAN,
                    64,
                )
                .unwrap_err(),
            GraphPreparationError::InvalidSampleRate
        );
        assert_eq!(
            builder
                .build(
                    first_revision,
                    &patches,
                    parameters(first_revision, &patches),
                    48_000.0,
                    0,
                )
                .unwrap_err(),
            GraphPreparationError::InvalidFrameCapacity
        );

        let failing_preparers = vec![FixturePreparer::boxed(true)];
        let failing_builder = PreparedGraphBuilder::new(&registry, &failing_preparers);
        assert!(matches!(
            failing_builder.build(
                first_revision,
                &patches,
                parameters(first_revision, &patches),
                48_000.0,
                64,
            ),
            Err(GraphPreparationError::Rack(
                RackPreparationError::Instrument { .. }
            ))
        ));

        // An occupied bank the builder cannot prepare refuses the complete
        // graph and names the failing bus; nothing is silently substituted.
        let empty_preparers: Vec<Box<dyn InstrumentPreparer>> = Vec::new();
        let effect_registry =
            crate::adapter::production_effects::production_effect_registry().unwrap();
        let effect_preparers =
            crate::adapter::production_effects::production_effect_preparers().unwrap();
        let bank =
            crate::adapter::production_effects::production_default_bus_returns(&effect_registry)
                .unwrap();
        let return_parameters =
            ParameterSnapshot::for_graph(1, first_revision, globals(), MixerState::default(), &[])
                .unwrap()
                .with_returns(ParameterSnapshot::project_returns(&effect_registry, &bank).unwrap());
        let effects_builder = PreparedGraphBuilder::new(&registry, &empty_preparers)
            .with_effects(&effect_registry, &effect_preparers)
            .with_returns(&bank);
        assert!(matches!(
            effects_builder
                .build(first_revision, &[], return_parameters.clone(), f32::MAX, 1)
                .unwrap_err(),
            GraphPreparationError::BusReturn { bus, .. }
                if bus == crate::mixer::bus_id::BusId::new(0).unwrap()
        ));
        // Without its registry the same occupied bank is refused up front.
        let no_registry_builder =
            PreparedGraphBuilder::new(&registry, &empty_preparers).with_returns(&bank);
        assert!(matches!(
            no_registry_builder
                .build(first_revision, &[], return_parameters, 48_000.0, 1)
                .unwrap_err(),
            GraphPreparationError::BusReturn {
                source: super::BusReturnPreparationError::UnknownRegistryEntry,
                ..
            }
        ));

        assert_eq!(existing.revision(), first_revision);
        assert_eq!(existing.engine_rack().patch_count(), 2);
        assert_eq!(existing.engine_rack().patch_id(0), Some(patches[0].id()));
        assert_eq!(existing.engine_rack().patch_id(1), Some(patches[1].id()));
    }
}
