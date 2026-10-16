use crest_synth::adapter::atomic_audio_observation::AtomicAudioObservation;
use crest_synth::adapter::braids_capability::BRAIDS_CAPABILITY_ID;
use crest_synth::adapter::chorus_capability::{
    CHORUS_AMOUNT_PARAMETER_ID, CHORUS_CAPABILITY_ID, CHORUS_DEPTH_PARAMETER_ID,
};
use crest_synth::adapter::chorus_native::chorus_lifecycle_counts;
use crest_synth::adapter::chorus_preparer::{ChorusPreparer, CHORUS_SAMPLE_RATE};
use crest_synth::adapter::lock_free_audio_boundary::LockFreeAudioBoundary;
use crest_synth::adapter::lock_free_structural_graph_boundary::LockFreeStructuralGraphBoundary;
use crest_synth::adapter::production_effects::{
    production_chorus_config, production_effect_preparers, production_effect_providers,
    production_effect_registry,
};
use crest_synth::adapter::production_instruments::{
    production_capability_registry, production_instrument_providers,
};
use crest_synth::control::{
    AppEvent, AppLoop, AppState, Direction, PatchControlId, StateProjector, SurfaceId,
    TopLevelContext,
};
use crest_synth::kernel::{midi_message::MidiMessage, PatchId};
use crest_synth::mixer::global_parameters::GlobalParameters;
use crest_synth::mixer::mixer_state::MixerState;
use crest_synth::mixer::mixer_track_id::MixerTrackId;
use crest_synth::mixer::patch_output::PatchOutput;
use crest_synth::real_time::{
    AudioBoundary, AudioObservation, AudioRenderer, AudioThreadBoundary, ControlAudioObservation,
    GraphHandoffStatus, GraphPreparationCorrelation, GraphPreparationError,
    GraphPreparationRequest, GraphRevision, ParameterSnapshot, PatchAudioBlock,
    PatchEffectObservation, PreparedGraphBuilder, RtPostEffectParameters, StructuralGraphBoundary,
    MAX_ACTIVE_PATCHES,
};
use crest_synth::synth::{
    CapabilityId, CapabilityRegistry, DescriptorDefaultConfigFactory, EffectCapabilityError,
    EffectCapabilityRegistry, EffectPreparer, EffectRackPreparationError, EffectSlotId,
    InstrumentPreparationError, InstrumentPreparer, ParameterDefault, ParameterId, ParameterValue,
    Patch, PostEffectConfig, PreparedEffectError, PreparedInstrument, PreparedInstrumentError,
    PreparedPostEffectRackBuilder, MAX_POST_EFFECTS_PER_PATCH,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

static CHORUS_TEST_LOCK: Mutex<()> = Mutex::new(());

thread_local! {
    static COUNT_MEMORY: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<u64> = const { Cell::new(0) };
    static DEALLOCATIONS: Cell<u64> = const { Cell::new(0) };
}

struct AcceptanceAllocator;

#[global_allocator]
static ACCEPTANCE_ALLOCATOR: AcceptanceAllocator = AcceptanceAllocator;

unsafe impl GlobalAlloc for AcceptanceAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record_allocation();
        // SAFETY: This allocator delegates the original request unchanged.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record_allocation();
        // SAFETY: This allocator delegates the original request unchanged.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        record_deallocation();
        // SAFETY: This allocator delegates the original request unchanged.
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record_allocation();
        record_deallocation();
        // SAFETY: This allocator delegates the original request unchanged.
        unsafe { System.realloc(pointer, layout, size) }
    }
}

fn record_allocation() {
    let _ = COUNT_MEMORY.try_with(|enabled| {
        if enabled.get() {
            let _ = ALLOCATIONS.try_with(|count| count.set(count.get().saturating_add(1)));
        }
    });
}

fn record_deallocation() {
    let _ = COUNT_MEMORY.try_with(|enabled| {
        if enabled.get() {
            let _ = DEALLOCATIONS.try_with(|count| count.set(count.get().saturating_add(1)));
        }
    });
}

fn counted_render<Boundary, Structural, Observation>(
    renderer: &mut AudioRenderer<Boundary, Structural, Observation>,
    output: &mut [f32],
) -> (u64, u64, u64)
where
    Boundary: AudioThreadBoundary,
    Structural: crest_synth::real_time::AudioStructuralGraphBoundary,
    Observation: crest_synth::real_time::CallbackAudioObservation,
{
    ALLOCATIONS.with(|count| count.set(0));
    DEALLOCATIONS.with(|count| count.set(0));
    COUNT_MEMORY.with(|enabled| enabled.set(true));
    let start = Instant::now();
    renderer.render(output);
    let elapsed = start.elapsed().as_micros() as u64;
    COUNT_MEMORY.with(|enabled| enabled.set(false));
    (
        ALLOCATIONS.with(Cell::get),
        DEALLOCATIONS.with(Cell::get),
        elapsed,
    )
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StaticPatchEffectObservation {
    schema_version: u32,
    upstream_revision: &'static str,
    stmlib_revision: &'static str,
    source_hashes_match: bool,
    license_present: bool,
    configured_patch_id: u32,
    configured_effect_slots: u32,
    amount_depth_cases_exercised: u32,
    patch_focus_order_exact: bool,
    scalar_only_publication: bool,
    ordered_before_mix: bool,
    target_audio_distinct: bool,
    stereo_side_energy_nonzero: bool,
    target_patch_exact: bool,
    untargeted_patches_exact: bool,
    independent_instances: bool,
    independent_tails: bool,
    structural_config_preserved: bool,
    unsupported_rate_rejected: bool,
    missing_registration_rejected: bool,
    fallback_count: u64,
    callback_allocations: u64,
    callback_deallocations: u64,
    callback_destructions: u64,
    p99_render_microseconds: u64,
}

struct FixturePreparer {
    capability_id: CapabilityId,
}

impl FixturePreparer {
    fn new(capability_id: &str) -> Self {
        Self {
            capability_id: CapabilityId::new(capability_id).unwrap(),
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
        max_frames: usize,
    ) -> Result<Box<dyn PreparedInstrument>, InstrumentPreparationError> {
        if max_frames == 0 {
            return Err(InstrumentPreparationError::InvalidFrameCapacity);
        }
        Ok(Box::new(FixtureInstrument {
            patch_id: patch.id(),
            phase: 0,
        }))
    }
}

struct FixtureInstrument {
    patch_id: PatchId,
    phase: usize,
}

impl PreparedInstrument for FixtureInstrument {
    fn patch_id(&self) -> PatchId {
        self.patch_id
    }

    fn dispatch(
        &mut self,
        _message: MidiMessage,
        _parameters: &crest_synth::real_time::RtPatchParameters,
    ) -> Result<(), PreparedInstrumentError> {
        Ok(())
    }

    fn render(
        &mut self,
        output: &mut [f32],
        _frame_count: usize,
        _parameters: &crest_synth::real_time::RtPatchParameters,
    ) -> Result<(), crest_synth::synth::PreparedInstrumentError> {
        if self.patch_id.value() == 2 {
            for frame in output.chunks_exact_mut(2) {
                frame[0] = 0.07;
                frame[1] = -0.04;
            }
            return Ok(());
        }
        const WAVE: [f32; 8] = [0.20, 0.12, -0.04, -0.18, -0.16, -0.02, 0.14, 0.22];
        for frame in output.chunks_exact_mut(2) {
            let sample = WAVE[self.phase % WAVE.len()];
            frame[0] = sample;
            frame[1] = sample;
            self.phase = self.phase.wrapping_add(1);
        }

        Ok(())
    }

    fn all_notes_off(&mut self) {}
}

fn globals() -> GlobalParameters {
    GlobalParameters::new(0.0).unwrap()
}

fn instrument_preparers() -> Vec<Box<dyn InstrumentPreparer>> {
    vec![
        Box::new(FixturePreparer::new("instrument.soundfont.hidef")),
        Box::new(FixturePreparer::new(BRAIDS_CAPABILITY_ID)),
    ]
}

fn energy(samples: &[f32]) -> f64 {
    samples
        .iter()
        .map(|sample| f64::from(*sample) * f64::from(*sample))
        .sum()
}

fn collect_files(root: &Path, directory: &Path, output: &mut BTreeSet<String>) {
    for entry in fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect_files(root, &path, output);
        } else {
            output.insert(
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
}

fn verify_source_bundle() -> (bool, bool) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("vendor/chorus");
    let manifest = fs::read_to_string(root.join("SHA256SUMS")).unwrap();
    let mut manifest_paths = BTreeSet::new();
    let mut hashes_match = true;
    for line in manifest.lines() {
        let (expected, path) = line.split_once("  ").unwrap();
        manifest_paths.insert(path.to_owned());
        let bytes = fs::read(root.join(path)).unwrap();
        let actual = format!("{:x}", Sha256::digest(bytes));
        hashes_match &= actual == expected;
    }
    let mut actual_paths = BTreeSet::new();
    collect_files(&root, &root, &mut actual_paths);
    let mut expected_paths = manifest_paths;
    expected_paths.insert("PROVENANCE.md".to_owned());
    expected_paths.insert("SHA256SUMS".to_owned());
    hashes_match &= actual_paths == expected_paths;

    let provenance = fs::read_to_string(root.join("PROVENANCE.md")).unwrap();
    hashes_match &= provenance.contains("08460a69a7e1f7a81c5a2abcc7189c9a6b7208d4")
        && provenance.contains("e3bd7c9cc00e4364166f9905c0509b6ffd0535ec");
    let license = fs::read_to_string(root.join("LICENSE")).unwrap();
    let chorus = fs::read_to_string(root.join("rings/dsp/fx/chorus.h")).unwrap();
    (
        hashes_match,
        license.contains("released under the MIT License")
            && chorus.contains("Permission is hereby granted"),
    )
}

fn effect_fixture_patches() -> (CapabilityRegistry, EffectCapabilityRegistry, Vec<Patch>) {
    let capabilities = production_capability_registry().unwrap();
    let effects = production_effect_registry().unwrap();
    let factory = DescriptorDefaultConfigFactory::new(
        capabilities.clone(),
        production_instrument_providers().unwrap(),
    );
    let chorus = production_chorus_config(EffectSlotId::new(1).unwrap()).unwrap();
    let patches = vec![
        Patch::new(
            PatchId::new(1).unwrap(),
            "Configured".to_owned(),
            factory
                .create(&CapabilityId::new("instrument.soundfont.hidef").unwrap())
                .unwrap(),
            crest_synth::kernel::MidiChannel::new(0).unwrap(),
            PatchOutput::to_track(MixerTrackId::new(0).unwrap()),
        )
        .with_effect_slot(
            crest_synth::synth::effect_slot_id::EffectSlotIndex::new(0).unwrap(),
            chorus,
        ),
        Patch::new(
            PatchId::new(2).unwrap(),
            "Dry".to_owned(),
            factory
                .create(&CapabilityId::new(BRAIDS_CAPABILITY_ID).unwrap())
                .unwrap(),
            crest_synth::kernel::MidiChannel::new(1).unwrap(),
            PatchOutput::to_track(MixerTrackId::new(1).unwrap()),
        ),
    ];
    (capabilities, effects, patches)
}

fn independent_chorus_proof() -> (bool, bool) {
    let before = chorus_lifecycle_counts();
    let preparer = ChorusPreparer::new().unwrap();
    let config_a = production_chorus_config(EffectSlotId::new(1).unwrap()).unwrap();
    let config_b = production_chorus_config(EffectSlotId::new(2).unwrap()).unwrap();
    let mut first = preparer
        .prepare(PatchId::new(11).unwrap(), &config_a, 48_000.0, 256)
        .unwrap();
    let mut second = preparer
        .prepare(PatchId::new(12).unwrap(), &config_b, 48_000.0, 256)
        .unwrap();
    let prepared = chorus_lifecycle_counts();
    let independent_instances = prepared.active == before.active + 2
        && first.patch_id() != second.patch_id()
        && first.slot_id() != second.slot_id();

    let parameters_a = RtPostEffectParameters::new(config_a.slot_id(), &[0.8, 0.9]).unwrap();
    let parameters_b = RtPostEffectParameters::new(config_b.slot_id(), &[0.8, 0.9]).unwrap();
    let mut first_block = vec![0.0_f32; 512];
    let mut second_block = vec![0.0_f32; 512];
    first_block[0] = 0.9;
    first_block[1] = 0.9;
    first.process(&mut first_block, 256, &parameters_a).unwrap();
    second
        .process(&mut second_block, 256, &parameters_b)
        .unwrap();
    let second_remained_zero = second_block.iter().all(|sample| *sample == 0.0);
    let mut first_tail = 0.0_f64;
    let mut second_tail = 0.0_f64;
    for _ in 0..16 {
        first_block.fill(0.0);
        second_block.fill(0.0);
        first.process(&mut first_block, 256, &parameters_a).unwrap();
        second
            .process(&mut second_block, 256, &parameters_b)
            .unwrap();
        first_tail = first_tail.max(energy(&first_block));
        second_tail = second_tail.max(energy(&second_block));
    }
    let independent_tails = second_remained_zero && first_tail > 1.0e-10 && second_tail == 0.0;
    drop(first);
    drop(second);
    let after = chorus_lifecycle_counts();
    assert_eq!(after.active, before.active);
    assert_eq!(after.destroyed - before.destroyed, 2);
    (independent_instances, independent_tails)
}

#[test]
fn static_patch_effect() {
    let _chorus_guard = CHORUS_TEST_LOCK.lock().unwrap();
    const UPSTREAM: &str = "08460a69a7e1f7a81c5a2abcc7189c9a6b7208d4";
    const STMLIB: &str = "e3bd7c9cc00e4364166f9905c0509b6ffd0535ec";
    const FRAME_COUNT: usize = 256;

    let (source_hashes_match, license_present) = verify_source_bundle();
    assert!(source_hashes_match);
    assert!(license_present);

    let effect_providers = production_effect_providers().unwrap();
    let effect_registry = production_effect_registry().unwrap();
    let effect_preparers = production_effect_preparers().unwrap();
    assert_eq!(effect_registry.descriptors().len(), effect_providers.len());
    let descriptor = &effect_registry.descriptors()[0];
    assert_eq!(descriptor.id().as_str(), CHORUS_CAPABILITY_ID);
    assert_eq!(descriptor.label(), "Chorus");
    let parameters = descriptor.parameters().collect::<Vec<_>>();
    assert_eq!(parameters.len(), 2);
    assert_eq!(parameters[0].id().as_str(), CHORUS_AMOUNT_PARAMETER_ID);
    assert_eq!(parameters[1].id().as_str(), CHORUS_DEPTH_PARAMETER_ID);
    for parameter in &parameters {
        assert_eq!(parameter.fine_step(), Some(0.01));
        assert_eq!(parameter.coarse_step(), Some(0.1));
        let ParameterDefault::Value(default) = parameter.default_value() else {
            panic!("Chorus parameters have scalar defaults");
        };
        assert_eq!(parameter.scalar_value(default).unwrap(), 0.5);
    }

    let capabilities = production_capability_registry().unwrap();
    let instrument_providers = production_instrument_providers().unwrap();
    let factory = DescriptorDefaultConfigFactory::new(
        capabilities.clone(),
        production_instrument_providers().unwrap(),
    );
    let first_id = PatchId::new(1).unwrap();
    let second_id = PatchId::new(2).unwrap();
    let chorus = production_chorus_config(EffectSlotId::new(1).unwrap()).unwrap();
    let patches = vec![
        Patch::new(
            first_id,
            "Configured".to_owned(),
            factory
                .create(&CapabilityId::new("instrument.soundfont.hidef").unwrap())
                .unwrap(),
            crest_synth::kernel::MidiChannel::new(0).unwrap(),
            PatchOutput::to_track(MixerTrackId::new(0).unwrap()),
        )
        .with_effect_slot(
            crest_synth::synth::effect_slot_id::EffectSlotIndex::new(0).unwrap(),
            chorus.clone(),
        ),
        Patch::new(
            second_id,
            "Dry".to_owned(),
            factory
                .create(&CapabilityId::new(BRAIDS_CAPABILITY_ID).unwrap())
                .unwrap(),
            crest_synth::kernel::MidiChannel::new(1).unwrap(),
            PatchOutput::to_track(MixerTrackId::new(1).unwrap()),
        ),
    ];

    let initial = ParameterSnapshot::new(0, globals(), MixerState::default(), &[]).unwrap();
    let boundary = LockFreeAudioBoundary::new(64, initial);
    let (control, mut audio) = boundary.into_handles();
    let state = AppState::for_graph_with_effects(
        capabilities.clone(),
        effect_registry.clone(),
        globals(),
        GraphRevision::INITIAL,
    );
    let mut app_loop = AppLoop::new(
        state,
        StateProjector::for_graph(GraphRevision::INITIAL),
        control,
    )
    .unwrap();
    app_loop
        .dispatch(AppEvent::InstallPatches(patches))
        .unwrap();
    app_loop
        .dispatch(AppEvent::SelectContext(TopLevelContext::Patch))
        .unwrap();

    let first_patch = &app_loop.patches()[0];
    let instrument_descriptor = capabilities
        .descriptor(first_patch.instrument_config().capability_id())
        .unwrap();
    let focus_order = PatchControlId::resolve(
        instrument_descriptor,
        first_patch.instrument_config(),
        &effect_registry,
        first_patch.effect_slots(),
    );
    let amount_id = ParameterId::new(CHORUS_AMOUNT_PARAMETER_ID).unwrap();
    let depth_id = ParameterId::new(CHORUS_DEPTH_PARAMETER_ID).unwrap();
    let amount_control = PatchControlId::Effect(chorus.slot_id(), amount_id.clone());
    let depth_control = PatchControlId::Effect(chorus.slot_id(), depth_id.clone());
    let amount_index = focus_order
        .iter()
        .position(|control| control == &amount_control)
        .unwrap();
    let depth_index = focus_order
        .iter()
        .position(|control| control == &depth_control)
        .unwrap();
    // The descriptor keeps the occupant's scalar order, while the overview
    // keeps all three position identities independently reachable.
    let page = app_loop.current_patch_page().unwrap();
    let patch_focus_order_exact = depth_index == amount_index + 1
        && page.effects().len() == 3
        && page.effects()[0].occupancy().capability_id().is_some()
        && page.effects()[1].occupancy().capability_id().is_none()
        && page.effects()[2].occupancy().capability_id().is_none();
    assert!(patch_focus_order_exact);

    app_loop
        .dispatch(AppEvent::Navigate(Direction::Down))
        .unwrap();
    assert_eq!(
        app_loop.current_patch_page().unwrap().focused_control_id(),
        PatchControlId::EffectSlot(
            crest_synth::synth::effect_slot_id::EffectSlotIndex::new(0).unwrap(),
        )
    );
    app_loop
        .dispatch(AppEvent::EnterSurface(SurfaceId::PatchDetail))
        .unwrap();
    for _ in 0..64 {
        if app_loop.current_patch_page().unwrap().focused_control_id() == amount_control {
            break;
        }
        app_loop
            .dispatch(AppEvent::Navigate(Direction::Down))
            .unwrap();
    }
    assert_eq!(
        app_loop.current_patch_page().unwrap().focused_control_id(),
        amount_control
    );
    let generation_before = app_loop.current_parameters().generation();
    let revision_before = app_loop.graph_revision();
    app_loop
        .dispatch(AppEvent::Adjust(Direction::Right))
        .unwrap();
    app_loop
        .dispatch(AppEvent::Adjust(Direction::Left))
        .unwrap();
    app_loop
        .dispatch(AppEvent::Navigate(Direction::Down))
        .unwrap();
    app_loop.dispatch(AppEvent::Adjust(Direction::Up)).unwrap();
    app_loop
        .dispatch(AppEvent::Adjust(Direction::Down))
        .unwrap();
    let scalar_only_publication = app_loop.current_parameters().generation()
        == generation_before + 5
        && app_loop.graph_revision() == revision_before
        && audio.pop_command().is_none()
        && app_loop
            .current_parameters()
            .patch(first_id)
            .unwrap()
            .effect(0)
            .unwrap()
            .scalars()
            == [0.5, 0.5]
        && app_loop.patches()[0].effect_slots()[0]
            .as_ref()
            .is_some_and(|config| {
                config.value(&amount_id) == Some(&ParameterValue::continuous(0.5).unwrap())
                    && config.value(&depth_id) == Some(&ParameterValue::continuous(0.5).unwrap())
            })
        && app_loop.patches()[0].effect_slots()[1..]
            .iter()
            .all(Option::is_none);
    assert!(scalar_only_publication);

    let missing_registration_rejected = matches!(
        PreparedGraphBuilder::new(&capabilities, &instrument_preparers())
            .with_effects(&effect_registry, &[])
            .build(
                GraphRevision::INITIAL,
                app_loop.patches(),
                app_loop.current_parameters().clone(),
                CHORUS_SAMPLE_RATE,
                FRAME_COUNT,
            ),
        Err(GraphPreparationError::EffectRack(
            EffectRackPreparationError::MissingPreparer { .. }
        ))
    );
    assert!(missing_registration_rejected);

    let unsupported_rate_rejected = matches!(
        ChorusPreparer::new()
            .unwrap()
            .prepare(first_id, &chorus, 44_100.0, FRAME_COUNT,),
        Err(crest_synth::synth::EffectPreparationError::InvalidSampleRate)
    );
    assert!(unsupported_rate_rejected);

    let lifecycle_before_graph = chorus_lifecycle_counts();
    let instrument_preparers = instrument_preparers();
    let graph = PreparedGraphBuilder::new(&capabilities, &instrument_preparers)
        .with_effects(&effect_registry, &effect_preparers)
        .build(
            GraphRevision::INITIAL,
            app_loop.patches(),
            app_loop.current_parameters().clone(),
            CHORUS_SAMPLE_RATE,
            FRAME_COUNT,
        )
        .unwrap();
    assert_eq!(graph.effect_rack().slot_id_at(0, 0), Some(chorus.slot_id()));
    assert_eq!(graph.effect_rack().slot_id_at(1, 0), None);

    let target_config =
        DescriptorDefaultConfigFactory::new(capabilities.clone(), instrument_providers)
            .create(&CapabilityId::new(BRAIDS_CAPABILITY_ID).unwrap())
            .unwrap();
    let request = GraphPreparationRequest::replacement_with_effects(
        GraphPreparationCorrelation::new(
            crest_synth::control::EngineSelectionRequestId::FIRST,
            first_id,
            CapabilityId::new("instrument.soundfont.hidef").unwrap(),
            CapabilityId::new(BRAIDS_CAPABILITY_ID).unwrap(),
            GraphRevision::INITIAL,
            GraphRevision::new(2).unwrap(),
        )
        .unwrap(),
        app_loop.patches(),
        target_config,
        app_loop.current_parameters().generation(),
        globals(),
        MixerState::new(app_loop.current_parameters().mixer_tracks().clone()),
        crest_synth::shell::audio_output::AudioDeviceConfig::new(
            CHORUS_SAMPLE_RATE,
            2,
            crest_synth::shell::audio_output::AudioSampleFormat::F32,
            FRAME_COUNT,
        )
        .unwrap(),
        &capabilities,
        &effect_registry,
        app_loop.bus_returns(),
    )
    .unwrap();
    let structural_config_preserved = request.candidate_patches()[0].effect_slots()
        == app_loop.patches()[0].effect_slots()
        && request
            .candidate_parameters()
            .patch(first_id)
            .unwrap()
            .effects()
            == app_loop
                .current_parameters()
                .patch(first_id)
                .unwrap()
                .effects();
    assert!(structural_config_preserved);

    let structural = LockFreeStructuralGraphBoundary::new(
        1,
        1,
        GraphHandoffStatus::with_active(GraphRevision::INITIAL),
    )
    .unwrap();
    let (_structural_control, structural_audio) = structural.into_handles();
    let observation = AtomicAudioObservation::default();
    let (observation_writer, observation_reader) = observation.into_handles();
    let mut renderer =
        AudioRenderer::with_observation(audio, structural_audio, graph, observation_writer);
    let mut output = vec![0.0_f32; FRAME_COUNT * 2];
    let mut render_times = Vec::with_capacity(128);
    let mut callback_allocations = 0_u64;
    let mut callback_deallocations = 0_u64;
    let destroyed_before_render = chorus_lifecycle_counts().destroyed;
    for _ in 0..128 {
        let (allocations, deallocations, elapsed) = counted_render(&mut renderer, &mut output);
        callback_allocations += allocations;
        callback_deallocations += deallocations;
        render_times.push(elapsed);
    }
    let callback_destructions = chorus_lifecycle_counts()
        .destroyed
        .saturating_sub(destroyed_before_render);
    render_times.sort_unstable();
    let p99_index = (render_times.len() * 99 / 100).min(render_times.len() - 1);
    let p99_render_microseconds = render_times[p99_index];
    assert_eq!(callback_allocations, 0);
    assert_eq!(callback_deallocations, 0);
    assert_eq!(callback_destructions, 0);
    if !cfg!(debug_assertions) {
        assert!(
            p99_render_microseconds < 2_666,
            "release-profile p99 render time was {p99_render_microseconds} microseconds"
        );
    }

    let observed = observation_reader.read_latest_on_control();
    let effect = observed.patch_effect();
    let target_patch_exact = effect.patch_id() == Some(first_id);
    let target_audio_distinct =
        effect.input_rms() > 0.0 && effect.output_rms() > 0.0 && effect.difference_rms() > 1.0e-6;
    let stereo_side_energy_nonzero = effect.side_rms() > 1.0e-7;
    let ordered_before_mix = effect.output_rms() > 0.0
        && observed.primary_patch_id() == Some(first_id)
        && (observed.primary_patch_rms() - effect.output_rms()).abs() < 1.0e-6;
    let dry_stem = renderer.active_patch_audio().stems()[1].samples();
    let untargeted_patches_exact = dry_stem
        .chunks_exact(2)
        .all(|frame| frame[0] == 0.07 && frame[1] == -0.04);
    assert!(target_patch_exact);
    assert!(target_audio_distinct);
    assert!(stereo_side_energy_nonzero);
    assert!(ordered_before_mix);
    assert!(untargeted_patches_exact);
    assert_eq!(observed.routing_failures(), 0);

    let malformed_layout_rejected = {
        let mut prepared = ChorusPreparer::new()
            .unwrap()
            .prepare(first_id, &chorus, 48_000.0, FRAME_COUNT)
            .unwrap();
        let mut block = vec![0.0_f32; FRAME_COUNT * 2];
        let bad = RtPostEffectParameters::new(chorus.slot_id(), &[0.5]).unwrap();
        matches!(
            prepared.process(&mut block, FRAME_COUNT, &bad),
            Err(PreparedEffectError::ScalarLayoutMismatch)
        )
    };
    assert!(malformed_layout_rejected);

    let (independent_instances, independent_tails) = independent_chorus_proof();
    assert!(independent_instances);
    assert!(independent_tails);

    let configured_effect_slots = app_loop
        .patches()
        .iter()
        .map(|patch| patch.effect_slots().iter().flatten().count() as u32)
        .sum::<u32>();
    assert_eq!(configured_effect_slots, 1);
    assert_eq!(effect_providers.len(), effect_registry.descriptors().len());
    let fallback_count = observed.routing_failures();
    assert_eq!(fallback_count, 0);

    let evidence = StaticPatchEffectObservation {
        schema_version: 1,
        upstream_revision: UPSTREAM,
        stmlib_revision: STMLIB,
        source_hashes_match,
        license_present,
        configured_patch_id: first_id.value(),
        configured_effect_slots,
        amount_depth_cases_exercised: 2,
        patch_focus_order_exact,
        scalar_only_publication,
        ordered_before_mix,
        target_audio_distinct,
        stereo_side_energy_nonzero,
        target_patch_exact,
        untargeted_patches_exact,
        independent_instances,
        independent_tails,
        structural_config_preserved,
        unsupported_rate_rejected,
        missing_registration_rejected,
        fallback_count,
        callback_allocations,
        callback_deallocations,
        callback_destructions,
        p99_render_microseconds,
    };
    println!(
        "CREST_PATCH_EFFECT_OBSERVATION {}",
        serde_json::to_string(&evidence).unwrap()
    );
    println!("CREST_ACCEPTANCE static_patch_effect passed");

    drop(renderer);
    assert_eq!(
        chorus_lifecycle_counts().active,
        lifecycle_before_graph.active
    );
}

#[test]
fn chorus_source_provenance() {
    let (source_hashes_match, license_present) = verify_source_bundle();
    assert!(source_hashes_match);
    assert!(license_present);
}

#[test]
fn chorus_capability_schema_and_config_are_exact() {
    let registry = production_effect_registry().unwrap();
    assert_eq!(
        registry.descriptors().len(),
        production_effect_providers().unwrap().len()
    );
    let descriptor = &registry.descriptors()[0];
    assert_eq!(descriptor.id().as_str(), CHORUS_CAPABILITY_ID);
    assert_eq!(descriptor.label(), "Chorus");
    let parameters = descriptor.parameters().collect::<Vec<_>>();
    assert_eq!(parameters.len(), 2);
    assert_eq!(parameters[0].id().as_str(), CHORUS_AMOUNT_PARAMETER_ID);
    assert_eq!(parameters[1].id().as_str(), CHORUS_DEPTH_PARAMETER_ID);
    for parameter in parameters {
        assert_eq!(parameter.fine_step(), Some(0.01));
        assert_eq!(parameter.coarse_step(), Some(0.1));
        let ParameterDefault::Value(value) = parameter.default_value() else {
            panic!("Chorus parameter default must be a value");
        };
        assert_eq!(parameter.scalar_value(value).unwrap(), 0.5);
    }

    let config = production_chorus_config(EffectSlotId::new(1).unwrap()).unwrap();
    registry.validate_patch_effects(&[]).unwrap();
    registry
        .validate_patch_effects(std::slice::from_ref(&config))
        .unwrap();
    let mut reversed_values = config.values().to_vec();
    reversed_values.reverse();
    let reversed = PostEffectConfig::from_parts(
        config.slot_id(),
        config.capability_id().clone(),
        reversed_values,
        config.asset_references().to_vec(),
    );
    assert!(matches!(
        registry.validate_config(&reversed),
        Err(EffectCapabilityError::ConfigOrderMismatch(_))
    ));
    // The per-Patch cap is the three ordered positions: the same instance
    // twice is a duplicate, and a fourth distinct instance exceeds the
    // capacity.
    assert!(matches!(
        registry.validate_patch_effects(&[config.clone(), config.clone()]),
        Err(EffectCapabilityError::DuplicateSlot(_))
    ));
    let distinct: Vec<_> = (1..=MAX_POST_EFFECTS_PER_PATCH as u16 + 1)
        .map(|slot| {
            registry
                .descriptors()
                .iter()
                .find(|descriptor| descriptor.id() == config.capability_id())
                .unwrap()
                .default_config(EffectSlotId::new(slot).unwrap())
                .unwrap()
        })
        .collect();
    assert!(registry
        .validate_patch_effects(&distinct[..MAX_POST_EFFECTS_PER_PATCH])
        .is_ok());
    assert!(matches!(
        registry.validate_patch_effects(&distinct),
        Err(EffectCapabilityError::TooManyEffectSlots { .. })
    ));
}

#[test]
fn chorus_preparer_enforces_rate_layout_and_finite_stereo_processing() {
    let _chorus_guard = CHORUS_TEST_LOCK.lock().unwrap();
    let patch_id = PatchId::new(1).unwrap();
    let config = production_chorus_config(EffectSlotId::new(1).unwrap()).unwrap();
    let preparer = ChorusPreparer::new().unwrap();
    assert!(matches!(
        preparer.prepare(patch_id, &config, 44_100.0, 256),
        Err(crest_synth::synth::EffectPreparationError::InvalidSampleRate)
    ));
    assert!(matches!(
        preparer.prepare(patch_id, &config, CHORUS_SAMPLE_RATE, 0),
        Err(crest_synth::synth::EffectPreparationError::InvalidFrameCapacity)
    ));

    let mut prepared = preparer
        .prepare(patch_id, &config, CHORUS_SAMPLE_RATE, 256)
        .unwrap();
    let parameters = RtPostEffectParameters::new(config.slot_id(), &[0.8, 0.9]).unwrap();
    let mut block = vec![0.0_f32; 512];
    let mut changed = false;
    let mut side_energy = 0.0_f64;
    for cycle in 0..20 {
        for (index, frame) in block.chunks_exact_mut(2).enumerate() {
            let input = (((index + cycle * 17) % 23) as f32 - 11.0) * 0.0125;
            frame[0] = input;
            frame[1] = input;
        }
        let before = block.clone();
        prepared.process(&mut block, 256, &parameters).unwrap();
        assert!(block.iter().all(|sample| sample.is_finite()));
        changed |= block
            .iter()
            .zip(&before)
            .any(|(after, before)| (after - before).abs() > 1.0e-6);
        side_energy += block
            .chunks_exact(2)
            .map(|frame| f64::from(frame[0] - frame[1]).powi(2))
            .sum::<f64>();
    }
    assert!(changed);
    assert!(side_energy > 1.0e-10);

    let malformed = RtPostEffectParameters::new(config.slot_id(), &[0.5]).unwrap();
    assert_eq!(
        prepared.process(&mut block, 256, &malformed),
        Err(PreparedEffectError::ScalarLayoutMismatch)
    );
    assert_eq!(
        prepared.process(&mut block, 257, &parameters),
        Err(PreparedEffectError::FrameCapacityExceeded)
    );
}

#[test]
fn prepared_post_effect_rack_builder_prepares_exact_zero_or_one_slots() {
    let _chorus_guard = CHORUS_TEST_LOCK.lock().unwrap();
    let (_capabilities, effects, patches) = effect_fixture_patches();
    let preparers = production_effect_preparers().unwrap();
    let rack = PreparedPostEffectRackBuilder::build(
        &patches,
        &effects,
        &preparers,
        CHORUS_SAMPLE_RATE,
        256,
    )
    .unwrap();
    assert_eq!(rack.patch_count(), 2);
    assert_eq!(rack.patch_id(0), Some(PatchId::new(1).unwrap()));
    assert_eq!(rack.slot_id_at(0, 0), Some(EffectSlotId::new(1).unwrap()));
    assert_eq!(rack.scalar_count_at(0, 0), Some(2));
    assert_eq!(rack.slot_id_at(1, 0), None);

    let dry_rack = PreparedPostEffectRackBuilder::build(
        &patches[1..],
        &effects,
        &preparers,
        CHORUS_SAMPLE_RATE,
        256,
    )
    .unwrap();
    assert_eq!(dry_rack.patch_count(), 1);
    assert_eq!(dry_rack.slot_id_at(0, 0), None);
    assert!(matches!(
        PreparedPostEffectRackBuilder::build(&patches, &effects, &[], CHORUS_SAMPLE_RATE, 256,),
        Err(EffectRackPreparationError::MissingPreparer { .. })
    ));
    assert_eq!(
        PreparedPostEffectRackBuilder::build(&patches, &effects, &preparers, f32::NAN, 256,)
            .unwrap_err(),
        EffectRackPreparationError::InvalidSampleRate
    );
    assert_eq!(
        PreparedPostEffectRackBuilder::build(
            &patches,
            &effects,
            &preparers,
            CHORUS_SAMPLE_RATE,
            0,
        )
        .unwrap_err(),
        EffectRackPreparationError::InvalidFrameCapacity
    );
}

#[test]
fn prepared_post_effect_rack_processes_only_the_configured_patch() {
    let _chorus_guard = CHORUS_TEST_LOCK.lock().unwrap();
    let (capabilities, effects, patches) = effect_fixture_patches();
    let preparers = production_effect_preparers().unwrap();
    let mut rack = PreparedPostEffectRackBuilder::build(
        &patches,
        &effects,
        &preparers,
        CHORUS_SAMPLE_RATE,
        256,
    )
    .unwrap();
    let parameters = ParameterSnapshot::project_patches_with_effects(
        1,
        GraphRevision::INITIAL,
        globals(),
        MixerState::default(),
        &patches,
        &capabilities,
        &effects,
    )
    .unwrap();
    let mut block = PatchAudioBlock::prepare(256).unwrap();
    let mut observations = [PatchEffectObservation::EMPTY; MAX_ACTIVE_PATCHES];
    let mut difference = 0.0_f32;
    let mut side = 0.0_f32;
    for cycle in 0..20 {
        block.begin_render(&parameters, 256).unwrap();
        for (index, frame) in block
            .stem_mut(0, PatchId::new(1).unwrap())
            .unwrap()
            .chunks_exact_mut(2)
            .enumerate()
        {
            let input = (((index + cycle * 17) % 23) as f32 - 11.0) * 0.0125;
            frame[0] = input;
            frame[1] = input;
        }
        block
            .stem_mut(1, PatchId::new(2).unwrap())
            .unwrap()
            .chunks_exact_mut(2)
            .for_each(|frame| {
                frame[0] = 0.07;
                frame[1] = -0.04;
            });
        rack.process(&mut block, &parameters, &mut observations)
            .unwrap();
        difference = difference.max(observations[0].difference_rms());
        side = side.max(observations[0].side_rms());
        assert!(block
            .stem(1, PatchId::new(2).unwrap())
            .unwrap()
            .samples()
            .chunks_exact(2)
            .all(|frame| frame == [0.07, -0.04]));
        assert_eq!(observations[1], PatchEffectObservation::EMPTY);
    }
    assert_eq!(observations[0].patch_id(), Some(PatchId::new(1).unwrap()));
    assert!(difference > 1.0e-6);
    assert!(side > 1.0e-7);

    let no_effect_parameters = ParameterSnapshot::project_patches_with_effects(
        2,
        GraphRevision::INITIAL,
        globals(),
        MixerState::default(),
        &patches[1..],
        &capabilities,
        &effects,
    )
    .unwrap();
    assert!(!rack.matches_parameters(&no_effect_parameters));
}
