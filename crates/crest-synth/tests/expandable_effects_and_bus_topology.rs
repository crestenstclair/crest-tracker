//! The declared goal witness for the expandable-effects-and-bus-topology
//! phase (`witnesses.expandable_effects_and_bus_topology`): measures every
//! declared observation field against the production reducer, deterministic
//! preparation worker, coordinator, renderer, and mixer, and emits the
//! `CREST_EFFECTS_AND_BUSES_OBSERVATION` marker.
//!
//! Every field is measured, never asserted into existence.
//! `clearedSlotPreservedHeldNotes` is measured through the production
//! observation across a real cleared-slot activation with a note held: WP10's
//! voice carry-over exchanges the still-live prepared instances into the
//! replacement at the block boundary, so the measurement now honestly reads
//! `true` — and still reads `false` if the mechanism regresses, because
//! nothing here re-sounds the note after activation.
//!
//! Schema v2 (WP09) adds two measured fields. `fourthEntryEndToEndExercised`
//! is the conjunction of five stages driven for a FOURTH registry entry that
//! exists only in this file: slot occupancy, return occupancy, preparation,
//! projection, and render. The entire witness runs on that four-entry
//! registry, so every pre-existing field above is measured with the fourth
//! entry installed and still reads what it read with three — which is what
//! "adding a registry entry changes zero structure" actually means.
//! `carryOverWrongEngineIdentityRefused` measures WP08's per-position
//! capability-identity requirement at admission time; see
//! `measure_carry_over_identity_refusal` for exactly which of the two
//! observable refusal layers it witnesses and why.

use core::alloc::{GlobalAlloc, Layout};
use core::cell::Cell;
use crest_synth::adapter::atomic_audio_observation::AtomicAudioObservation;
use crest_synth::adapter::lock_free_audio_boundary::LockFreeAudioBoundary;
use crest_synth::adapter::lock_free_structural_graph_boundary::LockFreeStructuralGraphBoundary;
use crest_synth::adapter::production_effects::{
    production_default_bus_returns, production_effect_preparers, production_effect_providers,
    production_effect_registry,
};
use crest_synth::adapter::production_instruments::{
    production_capability_registry, production_instrument_preparers,
    production_instrument_providers, production_soundfont_capability,
};
use crest_synth::control::app_event::AppEvent;
use crest_synth::control::app_loop::AppLoop;
use crest_synth::control::app_state::AppState;
use crest_synth::control::event_record::EventSource;
use crest_synth::control::state_projector::StateProjector;
use crest_synth::control::{
    EngineSelectionStatusKind, EventRejection, PatchControlId, PatchPageEffectSlot,
    PatchPageSection, PatchPageSlotOccupancy, SemanticAction,
};
use crest_synth::kernel::midi_channel::MidiChannel;
use crest_synth::kernel::midi_message::{MidiMessage, MidiMessageKind};
use crest_synth::kernel::PatchId;
use crest_synth::mixer::bus_id::{BusId, MAX_BUS_RETURNS};
use crest_synth::mixer::global_parameters::GlobalParameters;
use crest_synth::mixer::mix_engine::MixEngine;
use crest_synth::mixer::mixer_state::MixerState;
use crest_synth::mixer::mixer_track_id::MixerTrackId;
use crest_synth::mixer::mixer_track_parameters::MixerTrackParameters;
use crest_synth::mixer::patch_output::PatchOutput;
use crest_synth::real_time::audio_boundary::AudioBoundary;
use crest_synth::real_time::audio_observation::{AudioObservation, ControlAudioObservation};
use crest_synth::real_time::audio_observation_snapshot::AudioObservationSnapshot;
use crest_synth::real_time::audio_renderer::AudioRenderer;
use crest_synth::real_time::graph_revision::GraphRevision;
use crest_synth::real_time::prepared_graph::PositionCapabilityIdentity;
use crest_synth::real_time::prepared_graph_builder::PreparedGraphBuilder;
use crest_synth::real_time::{
    GraphHandoffStatus, GraphPublicationFailure, GraphReplacementScope, GraphStageOutcome,
    ParameterSnapshot, PatchAudioBlock, RtBusReturnParameters, RtPatchParameters,
    RtPostEffectParameters, StructuralGraphBoundary, StructuralGraphCoordinator,
};
use crest_synth::shell::audio_output::{AudioDeviceConfig, AudioSampleFormat};
use crest_synth::synth::effect_slot_id::EffectSlotIndex;
use crest_synth::synth::sound_font_instrument::SoundFontInstrument;
use crest_synth::synth::{
    compose_effect_registry, AssetAssignment, CapabilitySection, DescriptorDefaultConfigFactory,
    EffectCapabilityDescriptor, EffectCapabilityError, EffectCapabilityId,
    EffectCapabilityProvider, EffectCapabilityRegistry, EffectPreparationError, EffectPreparer,
    EffectSlotId, ParameterAssignment, ParameterDefault, ParameterId, ParameterKind,
    ParameterRange, ParameterSpec, ParameterUpdate, ParameterValue, Patch, PatchInteraction,
    PostEffectConfig, PreparedEffectError, PreparedPostEffect,
};
use crest_synth::testing::automatic_midi_test::create_soundfont_config;
use crest_synth::testing::deterministic_graph_preparation_worker::{
    DeterministicGraphPreparationHandle, DeterministicGraphPreparationWorker,
};
use serde::Serialize;
use std::alloc::System;

const SAMPLE_RATE: f32 = 48_000.0;
const FRAME_COUNT: usize = 128;
const SAMPLE_COUNT: usize = FRAME_COUNT * 2;
const SILENCE_DBFS: f64 = -200.0;

thread_local! {
    static COUNT_MEMORY: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<u64> = const { Cell::new(0) };
    static DEALLOCATIONS: Cell<u64> = const { Cell::new(0) };
}

struct WitnessAllocator;

#[global_allocator]
static WITNESS_ALLOCATOR: WitnessAllocator = WitnessAllocator;

unsafe impl GlobalAlloc for WitnessAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNT_MEMORY.with(Cell::get) {
            ALLOCATIONS.with(|count| count.set(count.get().saturating_add(1)));
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        if COUNT_MEMORY.with(Cell::get) {
            DEALLOCATIONS.with(|count| count.set(count.get().saturating_add(1)));
        }
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct EffectsAndBusesObservation {
    schema_version: u32,
    ordered_slot_cases_exercised: usize,
    slot_order_exchange_distinct: bool,
    same_entry_instances_independent: bool,
    cleared_slot_preserved_held_notes: bool,
    cleared_slot_focus_recovered: bool,
    addressable_returns: usize,
    default_return_occupancy_exact: bool,
    max_off_target_bus_dbfs: f64,
    muted_or_solo_excluded_wet_contribution: f64,
    unoccupied_return_silent: bool,
    return_content_change_dry_uninterrupted: bool,
    topology_rejections_exercised: usize,
    rejection_preserved_active_graph: bool,
    rejection_reason_attributable: bool,
    post_rejection_valid_change_accepted: bool,
    partially_applied_topology_blocks: u64,
    registry_entry_addition_structural_changes: usize,
    fourth_entry_end_to_end_exercised: bool,
    carry_over_wrong_engine_identity_refused: bool,
    callback_allocations: u64,
    callback_deallocations: u64,
    callback_destructions: u64,
    retired_graphs_collected_off_callback: u64,
    active_notes_at_exit: u32,
    two_run_trace_equal: bool,
}

type FixtureRenderer = AudioRenderer<
    crest_synth::adapter::lock_free_audio_boundary::LockFreeAudioHandle,
    crest_synth::adapter::lock_free_structural_graph_boundary::LockFreeStructuralAudioHandle,
    crest_synth::adapter::atomic_audio_observation::AtomicAudioObservationWriter,
>;

struct Fixture {
    app_loop: AppLoop<crest_synth::adapter::lock_free_audio_boundary::LockFreeControlHandle>,
    renderer: FixtureRenderer,
    worker: DeterministicGraphPreparationHandle,
    observation: crest_synth::adapter::atomic_audio_observation::AtomicAudioObservationReader,
}

impl Fixture {
    fn counted_render(&mut self, output: &mut [f32], trace: &mut Vec<u8>) -> (u64, u64) {
        ALLOCATIONS.with(|count| count.set(0));
        DEALLOCATIONS.with(|count| count.set(0));
        COUNT_MEMORY.with(|enabled| enabled.set(true));
        self.renderer.render(output);
        COUNT_MEMORY.with(|enabled| enabled.set(false));
        for sample in output.iter() {
            trace.extend_from_slice(&sample.to_le_bytes());
        }
        (ALLOCATIONS.with(Cell::get), DEALLOCATIONS.with(Cell::get))
    }

    fn note_on(&mut self, note: u8) {
        self.app_loop
            .dispatch_from(
                AppEvent::Midi {
                    patch_id: PatchId::new(1).unwrap(),
                    message: MidiMessage::try_new(
                        MidiChannel::new(0).unwrap(),
                        MidiMessageKind::NoteOn,
                        note,
                        112,
                    )
                    .unwrap(),
                },
                EventSource::System,
            )
            .unwrap();
    }

    fn all_notes_off(&mut self) {
        self.app_loop
            .dispatch_from(
                AppEvent::Midi {
                    patch_id: PatchId::new(1).unwrap(),
                    message: MidiMessage::all_notes_off(MidiChannel::new(0).unwrap()),
                },
                EventSource::System,
            )
            .unwrap();
    }

    /// Drives one accepted occupancy request through preparation, atomic
    /// block-boundary activation, and acknowledgement; returns collected
    /// retirements plus the count of partially-applied blocks observed and
    /// the allocator counts across the activation renders.
    fn complete_change(
        &mut self,
        output: &mut [f32],
        trace: &mut Vec<u8>,
        counters: &mut Counters,
    ) -> u64 {
        for expected in [
            EngineSelectionStatusKind::Validating,
            EngineSelectionStatusKind::Preparing,
        ] {
            let admission = self.app_loop.advance_structural().unwrap();
            assert_eq!(
                admission.engine_selection_lifecycle_advanced(),
                Some(expected)
            );
        }
        assert!(self.worker.advance(), "worker advances one request");
        let source = self.renderer.active_revision();
        let staged = self.app_loop.advance_structural().unwrap();
        assert!(staged.graph_stage().is_some(), "graph stages off-callback");
        let (allocations, deallocations) = self.counted_render(output, trace);
        counters.allocations += allocations;
        counters.deallocations += deallocations;
        let target = self.renderer.active_revision();
        assert!(target > source, "block-boundary activation");
        let observed = self.observation.read_latest_on_control();
        if observed.active_graph_revision() != source && observed.active_graph_revision() != target
        {
            counters.partial_blocks += 1;
        }
        let ack = self.app_loop.advance_structural().unwrap();
        assert!(ack.activation_acknowledged().is_some());
        assert_eq!(
            self.app_loop.engine_selection_status().kind(),
            EngineSelectionStatusKind::Ready
        );
        ack.collected_count()
    }

    /// Renders one deterministic fresh-note chain trace of the current
    /// topology: all notes off, one note on, then the given block count.
    fn chain_trace(
        &mut self,
        note: u8,
        blocks: usize,
        counters: &mut Counters,
        trace: &mut Vec<u8>,
    ) -> Vec<u8> {
        self.all_notes_off();
        let mut output = vec![0.0_f32; SAMPLE_COUNT];
        let (allocations, deallocations) = self.counted_render(&mut output, trace);
        counters.allocations += allocations;
        counters.deallocations += deallocations;
        self.note_on(note);
        let mut chain = Vec::with_capacity(blocks * SAMPLE_COUNT * 4);
        for _ in 0..blocks {
            let (allocations, deallocations) = self.counted_render(&mut output, trace);
            counters.allocations += allocations;
            counters.deallocations += deallocations;
            for sample in &output {
                assert!(sample.is_finite(), "rendered audio stays finite");
                chain.extend_from_slice(&sample.to_le_bytes());
            }
        }
        self.all_notes_off();
        let (allocations, deallocations) = self.counted_render(&mut output, trace);
        counters.allocations += allocations;
        counters.deallocations += deallocations;
        chain
    }

    /// Holds one note across a settled run of blocks and returns the
    /// production observation published from the last of them.
    fn sustained_observation(
        &mut self,
        note: u8,
        counters: &mut Counters,
        trace: &mut Vec<u8>,
    ) -> AudioObservationSnapshot {
        self.all_notes_off();
        let mut output = vec![0.0_f32; SAMPLE_COUNT];
        let (allocations, deallocations) = self.counted_render(&mut output, trace);
        counters.allocations += allocations;
        counters.deallocations += deallocations;
        self.note_on(note);
        for _ in 0..24 {
            let (allocations, deallocations) = self.counted_render(&mut output, trace);
            counters.allocations += allocations;
            counters.deallocations += deallocations;
        }
        let observed = self.observation.read_latest_on_control();
        self.all_notes_off();
        let (allocations, deallocations) = self.counted_render(&mut output, trace);
        counters.allocations += allocations;
        counters.deallocations += deallocations;
        observed
    }
}

#[derive(Default)]
struct Counters {
    allocations: u64,
    deallocations: u64,
    partial_blocks: u64,
}

// ---------------------------------------------------------------------------
// T035 — the FOURTH registry entry.
//
// SC-008 was graded PARTIAL because openness was proved by structural absence
// (leaf-schema scan, name-enumeration guard, occupant-generic projection) and
// never by an executable registration. This entry closes that gap: it is a
// complete, descriptor-driven effect capability declared only in this test
// file, composed through the very same `compose_effect_registry` seam the
// three production entries use, with an ordinary provider/preparer pair.
//
// Zero structural change: nothing outside this file knows it exists, and
// nothing outside this file was edited to admit it. Its DSP is deliberately
// trivial — the proof is structural citizenship, not sonic character. Its
// declared shape deliberately mirrors a built-in's (two Continuous/Scalar
// parameters), which lets the projection stage compare the two projected
// shapes and lets T037 isolate a capability-identity mismatch from every
// other layout dimension.
// ---------------------------------------------------------------------------

const FOURTH_CAPABILITY_ID: &str = "effect.witness-tilt";
const FOURTH_BLEND_PARAMETER_ID: &str = "witness-tilt.blend";
const FOURTH_TRIM_PARAMETER_ID: &str = "witness-tilt.trim";

struct WitnessTiltCapability {
    descriptor: EffectCapabilityDescriptor,
}

impl WitnessTiltCapability {
    fn new() -> Result<Self, EffectCapabilityError> {
        let descriptor = EffectCapabilityDescriptor::new(
            EffectCapabilityId::new(FOURTH_CAPABILITY_ID).expect("the fourth entry id is valid"),
            "Witness Tilt",
            FOURTH_CAPABILITY_ID,
            vec![CapabilitySection::new(
                "witness-tilt.controls",
                "Witness Tilt",
                vec![
                    fourth_entry_parameter(FOURTH_BLEND_PARAMETER_ID, "Blend")?,
                    fourth_entry_parameter(FOURTH_TRIM_PARAMETER_ID, "Trim")?,
                ],
            )?],
            Vec::new(),
        )?;
        Ok(Self { descriptor })
    }
}

impl EffectCapabilityProvider for WitnessTiltCapability {
    fn descriptor(&self) -> EffectCapabilityDescriptor {
        self.descriptor.clone()
    }

    fn create_config(
        &self,
        slot_id: EffectSlotId,
        values: &[ParameterAssignment],
        asset_references: &[AssetAssignment],
    ) -> Result<PostEffectConfig, EffectCapabilityError> {
        self.descriptor
            .create_config(slot_id, values, asset_references)
    }
}

fn fourth_entry_parameter(id: &str, label: &str) -> Result<ParameterSpec, EffectCapabilityError> {
    Ok(ParameterSpec::new_with_patch_interaction(
        ParameterId::new(id).expect("the fourth entry parameter id is valid"),
        label,
        ParameterKind::Continuous,
        ParameterUpdate::Scalar,
        PatchInteraction::ScalarEdit,
        ParameterDefault::Value(ParameterValue::continuous(0.5)?),
        Some(ParameterRange::new(0.0, 1.0)?),
        Vec::new(),
        Some(0.01),
        Some(0.1),
        None,
        "normalized",
        None,
        None,
    )?)
}

struct WitnessTiltPreparer {
    capability_id: EffectCapabilityId,
}

impl WitnessTiltPreparer {
    fn new() -> Self {
        Self {
            capability_id: EffectCapabilityId::new(FOURTH_CAPABILITY_ID)
                .expect("the fourth entry id is valid"),
        }
    }
}

impl EffectPreparer for WitnessTiltPreparer {
    fn capability_id(&self) -> &EffectCapabilityId {
        &self.capability_id
    }

    fn prepare(
        &self,
        patch_id: PatchId,
        config: &PostEffectConfig,
        sample_rate: f32,
        max_frames: usize,
    ) -> Result<Box<dyn PreparedPostEffect>, EffectPreparationError> {
        if sample_rate != SAMPLE_RATE {
            return Err(EffectPreparationError::InvalidSampleRate);
        }
        if max_frames == 0 {
            return Err(EffectPreparationError::InvalidFrameCapacity);
        }
        if config.capability_id() != &self.capability_id {
            return Err(EffectPreparationError::UnsupportedCapability { patch_id });
        }
        let capability = WitnessTiltCapability::new()
            .map_err(|_| EffectPreparationError::InvalidConfiguration { patch_id })?;
        let canonical = capability
            .create_config(config.slot_id(), config.values(), config.asset_references())
            .map_err(|_| EffectPreparationError::InvalidConfiguration { patch_id })?;
        if &canonical != config {
            return Err(EffectPreparationError::InvalidConfiguration { patch_id });
        }
        // Every preparation yields an independent instance with its own
        // state, exactly like the production preparers.
        Ok(Box::new(PreparedWitnessTilt {
            patch_id,
            slot_id: config.slot_id(),
            max_frames,
            memory: [0.0; 2],
        }))
    }
}

struct PreparedWitnessTilt {
    patch_id: PatchId,
    slot_id: EffectSlotId,
    max_frames: usize,
    memory: [f32; 2],
}

impl PreparedPostEffect for PreparedWitnessTilt {
    fn patch_id(&self) -> PatchId {
        self.patch_id
    }

    fn slot_id(&self) -> EffectSlotId {
        self.slot_id
    }

    fn process(
        &mut self,
        interleaved_stereo: &mut [f32],
        frame_count: usize,
        parameters: &RtPostEffectParameters,
    ) -> Result<(), PreparedEffectError> {
        if frame_count > self.max_frames || interleaved_stereo.len() != frame_count * 2 {
            return Err(PreparedEffectError::FrameCapacityExceeded);
        }
        if parameters.slot_id() != Some(self.slot_id) || parameters.scalar_count() != 2 {
            return Err(PreparedEffectError::ScalarLayoutMismatch);
        }
        let blend = parameters
            .scalar(0)
            .filter(|value| value.is_finite() && (0.0..=1.0).contains(value))
            .ok_or(PreparedEffectError::ScalarLayoutMismatch)?;
        let trim = parameters
            .scalar(1)
            .filter(|value| value.is_finite() && (0.0..=1.0).contains(value))
            .ok_or(PreparedEffectError::ScalarLayoutMismatch)?;
        let gain = 0.25 + 0.75 * trim;
        for (index, sample) in interleaved_stereo.iter_mut().enumerate() {
            let channel = index % 2;
            let input = *sample;
            let tilted = input.mul_add(1.0 - blend, self.memory[channel] * blend);
            self.memory[channel] = input;
            *sample = tilted * gain;
        }
        Ok(())
    }
}

/// The production registration order, plus the fourth entry appended after
/// it. Appending keeps positional entries 0..2 (Chorus, Reverb, Delay) exactly
/// where every pre-existing measurement expects them, so the fourth entry adds
/// a citizen without renumbering anyone.
fn witness_effect_providers() -> Vec<Box<dyn EffectCapabilityProvider>> {
    let mut providers = production_effect_providers().unwrap();
    providers.push(Box::new(WitnessTiltCapability::new().unwrap()));
    providers
}

fn witness_effect_preparers() -> Vec<Box<dyn EffectPreparer>> {
    let mut preparers = production_effect_preparers().unwrap();
    preparers.push(Box::new(WitnessTiltPreparer::new()));
    preparers
}

/// The registry this witness runs on: composed through the production
/// `compose_effect_registry` seam, with no registry-side special case for the
/// fourth entry. Composition itself is the first proof — an entry that needed
/// a structural change could not be composed here at all.
fn witness_effect_registry() -> EffectCapabilityRegistry {
    compose_effect_registry(&witness_effect_providers(), &witness_effect_preparers()).unwrap()
}

/// Erases every leaf value, keeping only the recursive key and array
/// structure, so two projections can be compared for shape alone.
fn json_shape(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.iter()
                .map(|(key, entry)| (key.clone(), json_shape(entry)))
                .collect(),
        ),
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(json_shape).collect())
        }
        serde_json::Value::Null => serde_json::Value::String("null".to_owned()),
        serde_json::Value::Bool(_) => serde_json::Value::String("bool".to_owned()),
        serde_json::Value::Number(_) => serde_json::Value::String("number".to_owned()),
        serde_json::Value::String(_) => serde_json::Value::String("string".to_owned()),
    }
}

fn soundfont_patch() -> Patch {
    Patch::new(
        PatchId::new(1).unwrap(),
        "Witness 1".to_owned(),
        create_soundfont_config(
            &production_soundfont_capability().unwrap(),
            SoundFontInstrument::new(0, 0, false).unwrap(),
        )
        .unwrap(),
        MidiChannel::new(0).unwrap(),
        PatchOutput::to_track(MixerTrackId::ALL[0]),
    )
}

fn fixture() -> Fixture {
    let registry = production_capability_registry().unwrap();
    // T035: the complete witness runs on the FOUR-entry registry. Every
    // pre-existing measurement below is taken with the fourth entry installed
    // and still reads exactly what it read with three — that, and not a
    // sidecar fixture, is what "adding an entry changes zero structure" means.
    let effects = witness_effect_registry();
    let bank = production_default_bus_returns(&effects).unwrap();

    // Track 0 sends toward the default-occupied bus 1 so return-content
    // changes are wet-coupled.
    let mut sends = [0.0_f32; MAX_BUS_RETURNS];
    sends[1] = 0.5;
    let mixer = MixerState::default().with_track(
        MixerTrackId::ALL[0],
        MixerTrackParameters::from_values(0.0, 0.0, false, false, sends).unwrap(),
    );

    let mut state = AppState::for_graph_with_effects(
        registry.clone(),
        effects.clone(),
        GlobalParameters::new(0.0).unwrap(),
        GraphRevision::INITIAL,
    )
    .with_initial_returns(bank)
    .with_initial_mixer(mixer);
    state
        .apply(AppEvent::InstallPatches(vec![soundfont_patch()]))
        .unwrap();

    let initial_transport = ParameterSnapshot::new(
        0,
        GlobalParameters::new(0.0).unwrap(),
        MixerState::default(),
        &[],
    )
    .unwrap();
    let boundary = LockFreeAudioBoundary::new(128, initial_transport);
    let (audio_control, audio_callback) = boundary.into_handles();
    let mut app_loop = AppLoop::new(
        state,
        StateProjector::for_graph(GraphRevision::INITIAL),
        audio_control,
    )
    .unwrap();

    let audio_config =
        AudioDeviceConfig::new(SAMPLE_RATE, 2, AudioSampleFormat::F32, FRAME_COUNT).unwrap();
    let instrument_preparers = production_instrument_preparers().unwrap();
    let effect_preparers = witness_effect_preparers();
    let initial_graph = PreparedGraphBuilder::new(&registry, &instrument_preparers)
        .with_effects(&effects, &effect_preparers)
        .with_returns(app_loop.bus_returns())
        .build(
            GraphRevision::INITIAL,
            app_loop.patches(),
            app_loop.current_parameters().clone(),
            SAMPLE_RATE,
            FRAME_COUNT,
        )
        .unwrap();

    let structural = LockFreeStructuralGraphBoundary::new(
        1,
        1,
        GraphHandoffStatus::with_active(GraphRevision::INITIAL),
    )
    .unwrap();
    let (structural_control, structural_callback) = structural.into_handles();
    let worker = DeterministicGraphPreparationWorker::new_with_effects(
        registry.clone(),
        production_instrument_preparers().unwrap(),
        effects.clone(),
        witness_effect_preparers(),
        audio_config,
    );
    let worker_handle = worker.advance_handle();
    app_loop
        .configure_engine_selection(
            DescriptorDefaultConfigFactory::new(
                registry,
                production_instrument_providers().unwrap(),
            ),
            worker,
            structural_control,
            &initial_graph,
            audio_config,
        )
        .unwrap();
    let (writer, reader) = AtomicAudioObservation::default().into_handles();
    let renderer =
        AudioRenderer::with_observation(audio_callback, structural_callback, initial_graph, writer);

    Fixture {
        app_loop,
        renderer,
        worker: worker_handle,
        observation: reader,
    }
}

/// A sample-exact unity return for the isolation and gate measurements.
struct UnityReturn;

impl PreparedPostEffect for UnityReturn {
    fn patch_id(&self) -> PatchId {
        PatchId::new(u32::MAX).unwrap()
    }

    fn slot_id(&self) -> EffectSlotId {
        EffectSlotId::new(1).unwrap()
    }

    fn process(
        &mut self,
        _interleaved_stereo: &mut [f32],
        _frame_count: usize,
        _parameters: &RtPostEffectParameters,
    ) -> Result<(), PreparedEffectError> {
        Ok(())
    }
}

struct RoutingMeasurements {
    max_off_target_bus_dbfs: f64,
    gated_wet_contribution: f64,
    unoccupied_return_silent: bool,
    dry_uninterrupted_across_content_change: bool,
}

/// SC-004/SC-005/NFR-007/C-BR-6: eight-destination isolation, gate, and
/// unoccupied-return measurements over the production mixer, plus the
/// dry-path independence from return content.
fn measure_routing() -> RoutingMeasurements {
    const MIX_FRAMES: usize = 8;
    const MIX_SAMPLES: usize = MIX_FRAMES * 2;

    let run = |parameters: MixerTrackParameters,
               unity_buses: &[BusId],
               live_slot: u16|
     -> (
        [f32; MIX_SAMPLES],
        crest_synth::mixer::mix_observation::MixObservation,
    ) {
        let track_id = MixerTrackId::ALL[0];
        let stems: [f32; MIX_SAMPLES] =
            core::array::from_fn(|index| if index % 2 == 0 { 0.4 } else { 0.2 });
        let mixer_state = MixerState::default().with_track(track_id, parameters);
        let patches = [RtPatchParameters::new(
            PatchId::new(1).unwrap(),
            PatchOutput::to_track(track_id),
        )];
        let base = ParameterSnapshot::new(
            1,
            GlobalParameters::new(0.0).unwrap(),
            mixer_state,
            &patches,
        )
        .unwrap();
        let mut returns = [const { RtBusReturnParameters::EMPTY }; MAX_BUS_RETURNS];
        for bus in unity_buses {
            returns[bus.index()] =
                RtBusReturnParameters::new(EffectSlotId::new(live_slot).unwrap(), &[], 1.0)
                    .unwrap();
        }
        let snapshot = base.with_returns(returns);
        let mut block = PatchAudioBlock::prepare(MIX_FRAMES).unwrap();
        block.begin_render(&snapshot, MIX_FRAMES).unwrap();
        block
            .stem_mut(0, PatchId::new(1).unwrap())
            .unwrap()
            .copy_from_slice(&stems);
        let mut mixer = MixEngine::new();
        mixer.prepare(SAMPLE_RATE, MIX_FRAMES).unwrap();
        for bus in unity_buses {
            mixer
                .install_bus_return(
                    *bus,
                    Box::new(UnityReturn),
                    RtPostEffectParameters::new(EffectSlotId::new(1).unwrap(), &[]).unwrap(),
                    1.0,
                )
                .unwrap();
        }
        let mut output = [0.0_f32; MIX_SAMPLES];
        let observation = mixer.mix(&block, &snapshot, &mut output);
        (output, observation)
    };

    // Isolation: raise one send toward each destination in turn; every
    // other destination must measure exact silence.
    let mut max_off_target = 0.0_f64;
    for target in BusId::ALL {
        let parameters = MixerTrackParameters::default()
            .with_send(target, 0.8)
            .unwrap();
        let (_, observation) = run(parameters, &BusId::ALL, 1);
        assert!(
            observation
                .bus_input_rms(target)
                .expect("bus belongs to observed default bank")
                > 0.0
        );
        for other in BusId::ALL {
            if other != target {
                max_off_target = max_off_target
                    .max(f64::from(
                        observation
                            .bus_input_rms(other)
                            .expect("bus belongs to observed default bank"),
                    ))
                    .max(f64::from(
                        observation
                            .bus_output_rms(other)
                            .expect("bus belongs to observed default bank"),
                    ));
            }
        }
    }
    let max_off_target_bus_dbfs = if max_off_target <= 0.0 {
        SILENCE_DBFS
    } else {
        (20.0 * max_off_target.log10()).max(SILENCE_DBFS)
    };

    // SC-005: muted and solo-excluded tracks contribute zero wet signal.
    let muted =
        MixerTrackParameters::from_values(0.0, 0.0, true, false, [1.0; MAX_BUS_RETURNS]).unwrap();
    let (_, muted_observation) = run(muted, &BusId::ALL, 1);
    let mut gated_wet = 0.0_f64;
    for bus in BusId::ALL {
        gated_wet = gated_wet
            .max(f64::from(
                muted_observation
                    .bus_input_rms(bus)
                    .expect("bus belongs to observed default bank"),
            ))
            .max(f64::from(
                muted_observation
                    .bus_output_rms(bus)
                    .expect("bus belongs to observed default bank"),
            ));
    }

    // C-BR-6: an unoccupied return contributes silence, never its input.
    let sends = MixerTrackParameters::default()
        .with_send(BusId::ALL[7], 1.0)
        .unwrap();
    let (unoccupied_output, unoccupied_observation) = run(sends, &[], 1);
    let dry_reference: [f32; MIX_SAMPLES] =
        core::array::from_fn(|index| if index % 2 == 0 { 0.4 } else { 0.2 });
    let unoccupied_return_silent = unoccupied_observation
        .bus_input_rms(BusId::ALL[7])
        .expect("bus belongs to observed default bank")
        > 0.0
        && unoccupied_observation
            .bus_output_rms(BusId::ALL[7])
            .expect("bus belongs to observed default bank")
            == 0.0
        && unoccupied_output
            .iter()
            .zip(dry_reference.iter())
            .all(|(actual, expected)| (actual - expected).abs() <= 1.0e-6);

    // FR-013: a return-content change leaves the dry path untouched — a
    // track with zero sends renders identically under either occupant.
    let dry_only = MixerTrackParameters::default();
    let (before_change, _) = run(dry_only.clone(), &[BusId::ALL[1]], 1);
    let (after_change, _) = run(dry_only, &[BusId::ALL[1]], 2);
    let wet_coupled = MixerTrackParameters::default()
        .with_send(BusId::ALL[1], 0.5)
        .unwrap();
    let (coupled_attested, _) = run(wet_coupled.clone(), &[BusId::ALL[1]], 1);
    let (coupled_changed, _) = run(wet_coupled, &[BusId::ALL[1]], 2);
    let dry_uninterrupted_across_content_change =
        before_change == after_change && coupled_attested != coupled_changed;

    RoutingMeasurements {
        max_off_target_bus_dbfs,
        gated_wet_contribution: gated_wet,
        unoccupied_return_silent,
        dry_uninterrupted_across_content_change,
    }
}

/// T037: measures WP08's per-position capability-identity requirement
/// refusing a carry-over whose candidate agrees on patch, slot, and scalar
/// layout but records a different capability at an unscoped position.
///
/// **Which layer this field witnesses.** Refusal is observable at two
/// distinct layers, and this measurement is layer **(a), admission time**:
/// `StructuralGraphCoordinator::stage_replacement` returns
/// `GraphPublicationFailure::IncompatibleLayout`, a discrete typed signal on
/// the production coordinator, because
/// `PreparedGraphLayout::permits_replacement` compares the recorded
/// per-position identities outside the declared scope. The carry-over is
/// refused by never admitting the replacement at all, so the running graph
/// keeps its own live instances and no exchange happens.
///
/// Layer **(b)**, the rack guards inside `PreparedGraph::carry_live_state_from`,
/// refuses the same disagreement per position — but the guard `continue`s
/// silently and publishes no counter and no status field, so refusal there is
/// observable only as a sample-level consequence ("the freshly prepared
/// instance is still there"). That plumbing does not exist and this test does
/// not invent it; layer (a) is measured instead, deliberately.
///
/// Neither is `GraphPreparationError::UnrecordableCapabilityIdentity` →
/// `EngineSelectionFailure::InvalidDefaultConfig` (surfaced as
/// `"invalidDefaultConfig"`), which is an identity that cannot be recorded at
/// all — a different path that this measurement never touches.
///
/// The read seam is WP08's chosen one: `PreparedGraph::layout()` and its
/// `engine_capability_identity` / `effect_capability_identity` /
/// `return_capability_identity` accessors. `PreparedGraphLayout` is `Copy`, so
/// the three layouts are captured and compared without holding live graphs.
///
/// Non-vacuity: the measurement is an A/B against one active layout. The two
/// candidates are built from the same patch shape at the same position with
/// the same instance identity, from two registry entries that declare the
/// same Scalar count (measured, not assumed), and their recorded engine and
/// all eight return identities agree — the only recorded difference is the
/// effect position's capability identity. The identity-agreeing candidate
/// must be admitted, so a coordinator that refused everything would read
/// false, and removing WP08's identity comparison would admit the mismatched
/// candidate and also read false.
fn measure_carry_over_identity_refusal() -> bool {
    let capabilities = production_capability_registry().unwrap();
    let instrument_preparers = production_instrument_preparers().unwrap();
    let effects = witness_effect_registry();
    let effect_preparers = witness_effect_preparers();
    let bank = production_default_bus_returns(&effects).unwrap();
    let global = GlobalParameters::new(0.0).unwrap();
    let mixer = MixerState::default();
    let entries: Vec<EffectCapabilityId> = effects
        .descriptors()
        .iter()
        .map(|descriptor| descriptor.id().clone())
        .collect();
    let incumbent = entries[0].clone();
    let candidate_entry = EffectCapabilityId::new(FOURTH_CAPABILITY_ID).unwrap();
    let scalar_layout_agrees = effects
        .descriptor(&incumbent)
        .unwrap()
        .scalar_parameter_count()
        == effects
            .descriptor(&candidate_entry)
            .unwrap()
            .scalar_parameter_count();

    let position = EffectSlotIndex::ALL[0];
    let patches_occupied_by = |entry: &EffectCapabilityId| {
        let occupant = effects
            .descriptor(entry)
            .unwrap()
            .default_config(position.instance_identity())
            .unwrap();
        vec![soundfont_patch().with_effect_slot(position, occupant)]
    };
    let build = |patches: &[Patch], revision: GraphRevision| {
        let parameters = ParameterSnapshot::project_patches_with_effects_and_returns(
            0,
            revision,
            global,
            mixer.clone(),
            patches,
            &capabilities,
            &effects,
            &bank,
        )
        .unwrap();
        PreparedGraphBuilder::new(&capabilities, &instrument_preparers)
            .with_effects(&effects, &effect_preparers)
            .with_returns(&bank)
            .build(revision, patches, parameters, SAMPLE_RATE, FRAME_COUNT)
            .unwrap()
    };

    let incumbent_patches = patches_occupied_by(&incumbent);
    let candidate_patches = patches_occupied_by(&candidate_entry);
    let replacement_revision = GraphRevision::INITIAL.checked_next().unwrap();
    let active = build(&incumbent_patches, GraphRevision::INITIAL);
    let agreeing = build(&incumbent_patches, replacement_revision);
    let mismatched = build(&candidate_patches, replacement_revision);

    let active_layout = active.layout();
    let agreeing_layout = agreeing.layout();
    let mismatched_layout = mismatched.layout();
    let difference_is_only_capability_identity = active_layout == agreeing_layout
        && active_layout != mismatched_layout
        && active_layout.effect_capability_identity(0, position.index())
            == PositionCapabilityIdentity::from_effect_capability_id(&incumbent)
        && mismatched_layout.effect_capability_identity(0, position.index())
            == PositionCapabilityIdentity::from_effect_capability_id(&candidate_entry)
        && active_layout.engine_capability_identity(0)
            == mismatched_layout.engine_capability_identity(0)
        && BusId::ALL.iter().all(|bus| {
            active_layout.return_capability_identity(bus.index())
                == mismatched_layout.return_capability_identity(bus.index())
        });

    let boundary = LockFreeStructuralGraphBoundary::new(
        1,
        1,
        GraphHandoffStatus::with_active(GraphRevision::INITIAL),
    )
    .unwrap();
    let (structural_control, _structural_callback) = boundary.into_handles();
    let mut coordinator = StructuralGraphCoordinator::new(structural_control, &active);
    // The declared scope is one unoccupied bus return, so every Patch effect
    // position is outside the scope and must agree exactly.
    let scope = GraphReplacementScope::BusReturn(BusId::ALL[7]);
    let refused = matches!(
        coordinator.stage_replacement(mismatched, scope),
        Err(error) if error.reason() == GraphPublicationFailure::IncompatibleLayout
    );
    let admitted = matches!(
        coordinator.stage_replacement(agreeing, scope),
        Ok(GraphStageOutcome::Staged)
    );

    scalar_layout_agrees && difference_is_only_capability_identity && refused && admitted
}

#[allow(clippy::too_many_lines)]
fn measure() -> (EffectsAndBusesObservation, Vec<u8>) {
    let mut fixture = fixture();
    let mut output = vec![0.0_f32; SAMPLE_COUNT];
    let mut trace = Vec::new();
    let mut counters = Counters::default();
    let entries: Vec<EffectCapabilityId> = fixture
        .app_loop
        .effects()
        .descriptors()
        .iter()
        .map(|descriptor| descriptor.id().clone())
        .collect();
    let patch_id = PatchId::new(1).unwrap();
    let slot = |index: usize| EffectSlotIndex::ALL[index];
    let occupy =
        |slot_index: usize, entry: Option<EffectCapabilityId>| SemanticAction::SetSlotOccupancy {
            patch_id,
            slot: slot(slot_index),
            entry,
        };

    let mut collected = 0_u64;

    // FR-002/FR-004: fill all three slots one at a time; every addition
    // changes the deterministic fresh-note render.
    let mut previous = fixture.chain_trace(60, 120, &mut counters, &mut trace);
    let mut ordered_slot_cases_exercised = 0;
    for (index, entry) in entries.iter().enumerate().take(EffectSlotIndex::ALL.len()) {
        fixture
            .app_loop
            .dispatch_action_from(occupy(index, Some(entry.clone())), EventSource::System)
            .unwrap();
        collected += fixture.complete_change(&mut output, &mut trace, &mut counters);
        let current = fixture.chain_trace(60, 120, &mut counters, &mut trace);
        if current != previous {
            ordered_slot_cases_exercised += 1;
        }
        previous = current;
    }

    // SC-002: exchanging two different effects produces a measurably
    // different render.
    fixture
        .app_loop
        .dispatch_action_from(occupy(0, Some(entries[1].clone())), EventSource::System)
        .unwrap();
    collected += fixture.complete_change(&mut output, &mut trace, &mut counters);
    fixture
        .app_loop
        .dispatch_action_from(occupy(1, Some(entries[0].clone())), EventSource::System)
        .unwrap();
    collected += fixture.complete_change(&mut output, &mut trace, &mut counters);
    let exchanged = fixture.chain_trace(60, 120, &mut counters, &mut trace);
    let slot_order_exchange_distinct = exchanged != previous;

    // SC-003: two instances of one registry entry occupy positions 0 and 2
    // as distinct prepared instances with their own state.
    fixture
        .app_loop
        .dispatch_action_from(occupy(0, Some(entries[2].clone())), EventSource::System)
        .unwrap();
    collected += fixture.complete_change(&mut output, &mut trace, &mut counters);
    let twin = fixture.chain_trace(60, 120, &mut counters, &mut trace);
    let slots = fixture.app_loop.patches()[0].effect_slots();
    let same_entry_instances_independent = twin != exchanged
        && slots[0]
            .as_ref()
            .zip(slots[2].as_ref())
            .is_some_and(|(first, third)| {
                first.capability_id() == third.capability_id() && first.slot_id() != third.slot_id()
            });

    // FR-003/AS-1.5: clear position 0 while a held note rings, with the slot
    // row focused. The held-note value is measured through the production
    // observation after the activation — no note is re-sounded, so the
    // measurement can only read true if the sounding voice itself survived
    // the swap (WP10 voice carry-over).
    fixture
        .app_loop
        .dispatch_from(
            AppEvent::SelectContext(crest_synth::control::TopLevelContext::Patch),
            EventSource::System,
        )
        .unwrap();
    let mut guard = 0;
    while fixture
        .app_loop
        .current_patch_page()
        .is_some_and(|page| page.focused_control_id() != PatchControlId::EffectSlot(slot(0)))
    {
        fixture
            .app_loop
            .dispatch_from(
                AppEvent::Navigate(crest_synth::control::Direction::Down),
                EventSource::System,
            )
            .unwrap();
        guard += 1;
        assert!(guard < 64, "the slot row is reachable");
    }
    fixture.note_on(64);
    let (allocations, deallocations) = fixture.counted_render(&mut output, &mut trace);
    counters.allocations += allocations;
    counters.deallocations += deallocations;
    let ringing = fixture.observation.read_latest_on_control();
    assert!(ringing.active_notes() > 0, "the held note rings");
    fixture
        .app_loop
        .dispatch_action_from(occupy(0, None), EventSource::System)
        .unwrap();
    collected += fixture.complete_change(&mut output, &mut trace, &mut counters);
    let (allocations, deallocations) = fixture.counted_render(&mut output, &mut trace);
    counters.allocations += allocations;
    counters.deallocations += deallocations;
    let after_clear = fixture.observation.read_latest_on_control();
    let cleared_slot_preserved_held_notes =
        after_clear.primary_active_notes() > 0 && after_clear.primary_patch_rms() > 0.0;
    // FR-002: the cleared position stays a stable, focused address, and the
    // canonical focus and the projected page agree exactly.
    let focus_tree: serde_json::Value =
        serde_json::from_str(fixture.app_loop.current_state_tree().json()).unwrap();
    let cleared_slot_focus_recovered = fixture
        .app_loop
        .current_patch_page()
        .is_some_and(|page| page.focused_control_id() == PatchControlId::EffectSlot(slot(0)))
        && focus_tree
            .pointer("/patchPage/focusedControlId")
            .and_then(serde_json::Value::as_str)
            == focus_tree
                .pointer("/interaction/activeFocus/controlId/id")
                .and_then(serde_json::Value::as_str)
        && focus_tree
            .pointer("/patchPage/focusedControlId")
            .and_then(serde_json::Value::as_str)
            .is_some();
    fixture.all_notes_off();

    // FR-013: change the content of the default-occupied return through the
    // complete lifecycle.
    fixture
        .app_loop
        .dispatch_action_from(
            SemanticAction::SetReturnOccupancy {
                bus: BusId::ALL[1],
                entry: Some(entries[0].clone()),
            },
            EventSource::System,
        )
        .unwrap();
    collected += fixture.complete_change(&mut output, &mut trace, &mut counters);
    assert!(fixture
        .app_loop
        .bus_returns()
        .bus_return(BusId::ALL[1])
        .is_occupied());

    // FR-015/T033: every refused class is typed, attributable, and mutates
    // nothing.
    let state_hash_before = fixture
        .app_loop
        .current_state_tree()
        .state_hash()
        .to_owned();
    let mut topology_rejections_exercised = 0;
    let refusals = [
        fixture.app_loop.dispatch_action_from(
            SemanticAction::SetSlotOccupancy {
                patch_id: PatchId::new(99).unwrap(),
                slot: slot(0),
                entry: Some(entries[0].clone()),
            },
            EventSource::System,
        ),
        fixture.app_loop.dispatch_action_from(
            SemanticAction::SetSlotOccupancy {
                patch_id,
                slot: slot(1),
                entry: Some(EffectCapabilityId::new("effect.absent").unwrap()),
            },
            EventSource::System,
        ),
        fixture.app_loop.dispatch_action_from(
            SemanticAction::SetReturnOccupancy {
                bus: BusId::ALL[4],
                entry: Some(EffectCapabilityId::new("effect.absent").unwrap()),
            },
            EventSource::System,
        ),
    ];
    assert!(matches!(refusals[0], Err(EventRejection::UnknownPatch)));
    assert!(matches!(
        refusals[1],
        Err(EventRejection::InvalidEffectConfig)
    ));
    assert!(matches!(
        refusals[2],
        Err(EventRejection::InvalidEffectConfig)
    ));
    topology_rejections_exercised += refusals.iter().filter(|result| result.is_err()).count();
    assert_eq!(
        fixture.app_loop.current_state_tree().state_hash(),
        state_hash_before,
        "refusals mutate nothing"
    );

    // The worker-refused preparation, its preserved graph, its attributable
    // reason, and the accepted recovery come from the refused-topology
    // witness case — the same measurement the declared negative command
    // falsifies.
    let refused = crest_synth::testing::BehavioralMutationHarness::new().run(
        crest_synth::testing::BehavioralMutationCase::RefusedTopology,
        false,
    );
    let crest_synth::testing::BehavioralMutationObservation::RefusedTopology(refused) =
        refused.into_observation()
    else {
        panic!("the refused-topology case retains its typed schema");
    };
    topology_rejections_exercised += usize::from(refused.refusal_recorded);
    let rejection_preserved_active_graph = refused.active_graph_preserved
        && refused.canonical_state_preserved
        && refused.render_preserved_exactly;
    let rejection_reason_attributable =
        refused.rejection_reason_attributable && refused.rejection_reason == "preparationFailed";
    let post_rejection_valid_change_accepted = refused.post_rejection_valid_change_accepted;

    // B-1: all eight returns are addressable by position.
    let registry = production_effect_registry().unwrap();
    let addressable_returns = BusId::ALL
        .iter()
        .filter(|bus| {
            let mut bank = fixture.app_loop.bus_returns().clone();
            bank.set_return_occupancy(&registry, **bus, Some(&entries[0]))
                .is_ok()
        })
        .count();

    // The composition default: reverb behavior at position 0, delay at
    // position 1 (positional registry entries 1 and 2), retained levels.
    let default_bank = production_default_bus_returns(&registry).unwrap();
    let default_return_occupancy_exact = default_bank
        .bus_return(BusId::ALL[0])
        .effect()
        .is_some_and(|config| config.capability_id() == &entries[1])
        && default_bank
            .bus_return(BusId::ALL[1])
            .effect()
            .is_some_and(|config| config.capability_id() == &entries[2])
        && (default_bank.bus_return(BusId::ALL[0]).return_level() - 0.5).abs() <= f32::EPSILON
        && (default_bank.bus_return(BusId::ALL[1]).return_level() - 0.5).abs() <= f32::EPSILON
        && BusId::ALL[2..]
            .iter()
            .all(|bus| !default_bank.bus_return(*bus).is_occupied());

    let routing = measure_routing();

    // SC-008: the structural vocabulary names no registry entry, so adding
    // an entry changes zero structure — measured by scanning the declared
    // realtime leaf schema for entry identities.
    let registry_entry_addition_structural_changes = ParameterSnapshot::SERIALIZED_LEAF_DESCRIPTOR
        .iter()
        .filter(|leaf| entries.iter().any(|entry| leaf.contains(entry.as_str())))
        .count();

    // ---------------------------------------------------------------------
    // T035/T036: the fourth entry driven end to end as a full citizen.
    //
    // Five stages must each contribute real evidence — partial coverage is
    // exactly the inference gap that graded SC-008 PARTIAL, so every one of
    // them is measured here and all five are conjoined into the emitted
    // field. Nothing below is asserted into existence: each stage reads a
    // production surface and would read false if the mechanism regressed.
    //
    // Position 0 is empty at this point (the cleared-slot case above) and bus
    // 1 carries a built-in occupant, so both roles start from a known state.
    // ---------------------------------------------------------------------
    let fourth = EffectCapabilityId::new(FOURTH_CAPABILITY_ID).unwrap();

    // Stage 1 — SLOT OCCUPANCY. The ordinary `SetSlotOccupancy` gesture, the
    // same call the three built-ins used above, with no fourth-entry branch.
    let revision_before_slot = fixture.renderer.active_revision();
    fixture
        .app_loop
        .dispatch_action_from(occupy(0, Some(fourth.clone())), EventSource::System)
        .unwrap();
    collected += fixture.complete_change(&mut output, &mut trace, &mut counters);
    let fourth_slot_occupancy_recorded = fixture.app_loop.patches()[0]
        .effect_slot(slot(0))
        .is_some_and(|config| config.capability_id() == &fourth);

    // Stage 3a — PREPARATION through the production worker and coordinator:
    // the deterministic preparation worker built a replacement graph
    // containing a prepared fourth-entry instance and the coordinator
    // activated it at a block boundary, advancing the active revision and
    // returning engine selection to Ready. An entry the worker could not
    // prepare would leave the revision where it was.
    let fourth_slot_preparation_activated = fixture.renderer.active_revision()
        > revision_before_slot
        && fixture.app_loop.engine_selection_status().kind() == EngineSelectionStatusKind::Ready;

    // Stage 4 — PROJECTION, taken here while the two peer positions still hold
    // built-in occupants. The occupied position projects generically: the
    // occupancy carries the entry's own id and descriptor label, the rows are
    // its own descriptor's parameters in descriptor order, the entry appears
    // among the registry-derived occupancy choices, and the projected shape is
    // structurally identical to a built-in occupant's — same keys, same array
    // lengths, different values. There is no role marker anywhere that would
    // distinguish the fourth entry from a built-in.
    let page = fixture
        .app_loop
        .current_patch_page()
        .expect("the Patch page projects");
    let fourth_slot_view = &page.effects()[slot(0).index()];
    let builtin_slot_view = &page.effects()[slot(1).index()];
    let fourth_descriptor = fixture
        .app_loop
        .effects()
        .descriptor(&fourth)
        .expect("the fourth entry is installed");
    let descriptor_rows: Vec<(String, String)> = fourth_descriptor
        .parameters()
        .map(|spec| (spec.id().as_str().to_owned(), spec.label().to_owned()))
        .collect();
    let projected_rows: Vec<(String, String)> = fourth_slot_view
        .sections()
        .iter()
        .flat_map(PatchPageSection::parameters)
        .map(|row| (row.id().as_str().to_owned(), row.label().to_owned()))
        .collect();
    let occupancy_shape =
        |view: &PatchPageEffectSlot| json_shape(&serde_json::to_value(view.occupancy()).unwrap());
    let sections_shape =
        |view: &PatchPageEffectSlot| json_shape(&serde_json::to_value(view.sections()).unwrap());
    let fourth_projection_generic = matches!(
        fourth_slot_view.occupancy(),
        PatchPageSlotOccupancy::Occupied {
            capability_id,
            label,
            ..
        } if capability_id == &fourth && label.as_str() == fourth_descriptor.label()
    ) && matches!(
        builtin_slot_view.occupancy(),
        PatchPageSlotOccupancy::Occupied { .. }
    ) && !descriptor_rows.is_empty()
        && projected_rows == descriptor_rows
        && occupancy_shape(fourth_slot_view) == occupancy_shape(builtin_slot_view)
        && sections_shape(fourth_slot_view) == sections_shape(builtin_slot_view)
        && serde_json::to_value(fourth_slot_view.sections()).unwrap()
            != serde_json::to_value(builtin_slot_view.sections()).unwrap()
        && fourth_slot_view.choices().iter().any(|choice| {
            choice.id() == fourth.as_str() && choice.label() == fourth_descriptor.label()
        })
        && fixture
            .app_loop
            .current_state_tree()
            .json()
            .contains(fourth.as_str());

    // Stage 5a — RENDER in the slot role, measured strictly around the fourth
    // entry's own processing. The production per-Patch effect-stage
    // observation spans the first occupied position's input to the chain's
    // output, so the two peer positions are cleared first and the fourth entry
    // becomes the whole chain: `differenceRms` is then exactly what this entry
    // did to the stem. The control clears the fourth entry too and requires
    // the same observation to fall back to `EMPTY` — so a pass-through
    // occupant, or one that never ran, could not read true here.
    for peer in [1_usize, 2] {
        fixture
            .app_loop
            .dispatch_action_from(occupy(peer, None), EventSource::System)
            .unwrap();
        collected += fixture.complete_change(&mut output, &mut trace, &mut counters);
    }
    let isolated = fixture
        .sustained_observation(60, &mut counters, &mut trace)
        .patch_effect();
    fixture
        .app_loop
        .dispatch_action_from(occupy(0, None), EventSource::System)
        .unwrap();
    collected += fixture.complete_change(&mut output, &mut trace, &mut counters);
    let unoccupied_chain = fixture
        .sustained_observation(60, &mut counters, &mut trace)
        .patch_effect();
    let fourth_slot_render_consequence = isolated.patch_id() == Some(patch_id)
        && isolated.input_rms().is_finite()
        && isolated.input_rms() > 0.0
        && isolated.output_rms().is_finite()
        && isolated.output_rms() > 0.0
        && isolated.difference_rms().is_finite()
        && isolated.difference_rms() > 0.0
        && unoccupied_chain == crest_synth::real_time::PatchEffectObservation::EMPTY;

    // Restore the fourth entry to the position: the same gesture again, now
    // from an empty chain, which the seam read below observes.
    fixture
        .app_loop
        .dispatch_action_from(occupy(0, Some(fourth.clone())), EventSource::System)
        .unwrap();
    collected += fixture.complete_change(&mut output, &mut trace, &mut counters);

    // Stage 2 — RETURN OCCUPANCY. Same entry, the other role, through the
    // ordinary `SetReturnOccupancy` gesture. Bus 1 is the wet-coupled
    // destination of track 0's 0.5 send and currently carries a built-in
    // occupant, so the same bus yields all three references — built-in,
    // unoccupied, and the fourth entry — under one unchanged Patch chain.
    let wet_with_builtin = f64::from(
        fixture
            .sustained_observation(64, &mut counters, &mut trace)
            .wet_output_rms(),
    );
    fixture
        .app_loop
        .dispatch_action_from(
            SemanticAction::SetReturnOccupancy {
                bus: BusId::ALL[1],
                entry: None,
            },
            EventSource::System,
        )
        .unwrap();
    collected += fixture.complete_change(&mut output, &mut trace, &mut counters);
    let wet_without_occupant = f64::from(
        fixture
            .sustained_observation(64, &mut counters, &mut trace)
            .wet_output_rms(),
    );
    let revision_before_return = fixture.renderer.active_revision();
    fixture
        .app_loop
        .dispatch_action_from(
            SemanticAction::SetReturnOccupancy {
                bus: BusId::ALL[1],
                entry: Some(fourth.clone()),
            },
            EventSource::System,
        )
        .unwrap();
    collected += fixture.complete_change(&mut output, &mut trace, &mut counters);
    let fourth_return_occupancy_recorded = fixture
        .app_loop
        .bus_returns()
        .bus_return(BusId::ALL[1])
        .effect()
        .is_some_and(|config| config.capability_id() == &fourth);
    let fourth_return_preparation_activated = fixture.renderer.active_revision()
        > revision_before_return
        && fixture.app_loop.engine_selection_status().kind() == EngineSelectionStatusKind::Ready;

    // Stage 5b — RENDER in the return role: the return contributes finite
    // nonzero wet energy where the unoccupied bus contributed none, and the
    // wet content is the fourth entry's own — a different occupant on the very
    // same bus, under the same dry chain and the same send, returns different
    // energy.
    let wet_with_fourth = f64::from(
        fixture
            .sustained_observation(64, &mut counters, &mut trace)
            .wet_output_rms(),
    );
    let fourth_return_render_consequence = wet_with_fourth.is_finite()
        && wet_with_fourth > 0.0
        && wet_without_occupant == 0.0
        && wet_with_builtin > 0.0
        && (wet_with_fourth - wet_with_builtin).abs() > f64::EPSILON;

    // Stage 3b — PREPARATION read through WP08's chosen seam. One more
    // replacement graph is prepared from the canonical state through the
    // production `PreparedGraphBuilder`, and its `PreparedGraphLayout` (Copy,
    // so no second live graph is held) records the fourth entry's capability
    // identity at BOTH the Patch-slot position and the bus-return position.
    // Only `layout()` covers all three position kinds — the rack path has no
    // public bus-returns accessor.
    let fourth_identity = PositionCapabilityIdentity::from_effect_capability_id(&fourth);
    let seam_instrument_preparers = production_instrument_preparers().unwrap();
    let seam_effect_preparers = witness_effect_preparers();
    let seam_capabilities = production_capability_registry().unwrap();
    let seam_parameters = fixture.app_loop.current_parameters().clone();
    let seam_layout = PreparedGraphBuilder::new(&seam_capabilities, &seam_instrument_preparers)
        .with_effects(fixture.app_loop.effects(), &seam_effect_preparers)
        .with_returns(fixture.app_loop.bus_returns())
        .build(
            seam_parameters.graph_revision(),
            fixture.app_loop.patches(),
            seam_parameters,
            SAMPLE_RATE,
            FRAME_COUNT,
        )
        .unwrap()
        .layout();
    let fourth_preparation_recorded = fourth_identity.is_some()
        && seam_layout.effect_capability_identity(0, slot(0).index()) == fourth_identity
        && seam_layout.return_capability_identity(BusId::ALL[1].index()) == fourth_identity;

    // The emitted field is the conjunction of all five stages. Each conjunct
    // was checked for non-vacuity by mutation: with the fourth entry's DSP
    // neutered to a pass-through, `differenceRms` reads 0 and the slot-render
    // stage reads false; with WP08's identity comparison removed from
    // `PreparedGraphLayout::permits_replacement`, T037's field reads false.
    // Both mutations were reverted.
    let fourth_entry_end_to_end_exercised = fourth_slot_occupancy_recorded
        && fourth_return_occupancy_recorded
        && fourth_slot_preparation_activated
        && fourth_return_preparation_activated
        && fourth_preparation_recorded
        && fourth_projection_generic
        && fourth_slot_render_consequence
        && fourth_return_render_consequence;

    // T037.
    let carry_over_wrong_engine_identity_refused = measure_carry_over_identity_refusal();

    // NFR-006: teardown — zero active notes, clean ownership.
    fixture.all_notes_off();
    let (allocations, deallocations) = fixture.counted_render(&mut output, &mut trace);
    counters.allocations += allocations;
    counters.deallocations += deallocations;
    let final_observation = fixture.observation.read_latest_on_control();
    let active_notes_at_exit = final_observation.active_notes();
    drop(fixture.renderer);
    assert_eq!(fixture.app_loop.owned_structural_graphs_on_control(), 0);
    fixture
        .app_loop
        .shutdown_engine_selection_on_control()
        .unwrap();

    trace.extend_from_slice(fixture.app_loop.current_state_tree().json().as_bytes());

    let observation = EffectsAndBusesObservation {
        schema_version: 2,
        ordered_slot_cases_exercised,
        slot_order_exchange_distinct,
        same_entry_instances_independent,
        cleared_slot_preserved_held_notes,
        cleared_slot_focus_recovered,
        addressable_returns,
        default_return_occupancy_exact,
        max_off_target_bus_dbfs: routing.max_off_target_bus_dbfs,
        muted_or_solo_excluded_wet_contribution: routing.gated_wet_contribution,
        unoccupied_return_silent: routing.unoccupied_return_silent,
        return_content_change_dry_uninterrupted: routing.dry_uninterrupted_across_content_change,
        topology_rejections_exercised,
        rejection_preserved_active_graph,
        rejection_reason_attributable,
        post_rejection_valid_change_accepted,
        partially_applied_topology_blocks: counters.partial_blocks,
        registry_entry_addition_structural_changes,
        fourth_entry_end_to_end_exercised,
        carry_over_wrong_engine_identity_refused,
        callback_allocations: counters.allocations,
        callback_deallocations: counters.deallocations,
        callback_destructions: counters.deallocations,
        retired_graphs_collected_off_callback: collected,
        active_notes_at_exit,
        two_run_trace_equal: false,
    };
    (observation, trace)
}

#[test]
fn expandable_effects_and_bus_topology() {
    let (mut first, first_trace) = measure();
    let (mut second, second_trace) = measure();
    let two_run_trace_equal = first_trace == second_trace && {
        first.two_run_trace_equal = true;
        second.two_run_trace_equal = true;
        first == second
    };
    first.two_run_trace_equal = two_run_trace_equal;

    assert_eq!(first.schema_version, 2);
    assert_eq!(first.ordered_slot_cases_exercised, 3);
    assert!(first.slot_order_exchange_distinct);
    assert!(first.same_entry_instances_independent);
    // AS-1.5/SC-001 (WP10): the sounding voice itself survived the cleared-
    // slot activation — measured, with no re-sounded note to fake it.
    assert!(first.cleared_slot_preserved_held_notes);
    assert!(first.cleared_slot_focus_recovered);
    assert_eq!(first.addressable_returns, 16);
    assert!(first.default_return_occupancy_exact);
    assert!(first.max_off_target_bus_dbfs < -60.0);
    assert_eq!(first.muted_or_solo_excluded_wet_contribution, 0.0);
    assert!(first.unoccupied_return_silent);
    assert!(first.return_content_change_dry_uninterrupted);
    assert!(first.topology_rejections_exercised > 0);
    assert!(first.rejection_preserved_active_graph);
    assert!(first.rejection_reason_attributable);
    assert!(first.post_rejection_valid_change_accepted);
    assert_eq!(first.partially_applied_topology_blocks, 0);
    assert_eq!(first.registry_entry_addition_structural_changes, 0);
    // T035/T036: slot occupancy, return occupancy, preparation, projection,
    // and render each contributed real evidence for the fourth registry entry.
    assert!(first.fourth_entry_end_to_end_exercised);
    // T037: measured at admission time — see
    // `measure_carry_over_identity_refusal` for which layer this witnesses.
    assert!(first.carry_over_wrong_engine_identity_refused);
    assert_eq!(first.callback_allocations, 0);
    assert_eq!(first.callback_deallocations, 0);
    assert_eq!(first.callback_destructions, 0);
    assert!(first.retired_graphs_collected_off_callback > 0);
    assert_eq!(first.active_notes_at_exit, 0);
    assert!(first.two_run_trace_equal);

    let json = serde_json::to_string(&first).unwrap();
    println!("CREST_EFFECTS_AND_BUSES_OBSERVATION {json}");
    println!("CREST_ACCEPTANCE expandable_effects_and_bus_topology passed");
}
