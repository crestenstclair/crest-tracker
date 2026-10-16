use crest_synth::adapter::atomic_audio_observation::AtomicAudioObservation;
use crest_synth::adapter::braids_capability::BRAIDS_CAPABILITY_ID;
use crest_synth::adapter::hidef_soundfont_capability::HIDEF_CAPABILITY_ID;
use crest_synth::adapter::lock_free_audio_boundary::LockFreeAudioBoundary;
use crest_synth::adapter::lock_free_structural_graph_boundary::LockFreeStructuralGraphBoundary;
use crest_synth::adapter::production_instruments::{
    production_capability_registry, production_instrument_preparers,
    production_instrument_providers,
};
use crest_synth::control::event_record::{EmittedEvent, EventSource};
use crest_synth::control::{
    AppEvent, AppLoop, AppState, Direction, EngineSelectionEffectKind, EngineSelectionFailure,
    EngineSelectionStatusKind, EventRejection, PatchControlId, StateProjector, TopLevelContext,
};
use crest_synth::kernel::midi_channel::MidiChannel;
use crest_synth::kernel::midi_message::{MidiMessage, MidiMessageKind};
use crest_synth::kernel::patch_id::PatchId;
use crest_synth::mixer::global_parameters::GlobalParameters;
use crest_synth::mixer::mixer_state::MixerState;
use crest_synth::mixer::mixer_track_id::MixerTrackId;
use crest_synth::mixer::patch_output::PatchOutput;
use crest_synth::real_time::{
    AudioBoundary, AudioObservation, AudioRenderer, ControlAudioObservation, GraphHandoffStatus,
    GraphRevision, GraphStageOutcome, ParameterSnapshot, PreparedGraphBuilder,
    StructuralGraphBoundary,
};
use crest_synth::shell::audio_output::{AudioDeviceConfig, AudioSampleFormat};
use crest_synth::synth::sound_font_instrument::SoundFontInstrument;
use crest_synth::synth::{
    CapabilityId, DescriptorDefaultConfigFactory, Patch, VoiceEnvelopeParameter,
};
use crest_synth::testing::automatic_midi_test::create_soundfont_config;
use crest_synth::testing::DeterministicGraphPreparationWorker;
use serde::Serialize;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

const SAMPLE_RATE: f32 = 48_000.0;
const FRAME_COUNT: usize = 256;
const SAMPLE_COUNT: usize = FRAME_COUNT * 2;

thread_local! {
    static COUNT_CALLBACK_MEMORY: Cell<bool> = const { Cell::new(false) };
    static CALLBACK_ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    static CALLBACK_DEALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

struct AcceptanceAllocator;

#[global_allocator]
static ACCEPTANCE_ALLOCATOR: AcceptanceAllocator = AcceptanceAllocator;

unsafe impl GlobalAlloc for AcceptanceAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record_callback_allocation();
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record_callback_allocation();
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        record_callback_deallocation();
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record_callback_allocation();
        record_callback_deallocation();
        unsafe { System.realloc(pointer, layout, size) }
    }
}

fn record_callback_allocation() {
    COUNT_CALLBACK_MEMORY.with(|enabled| {
        if enabled.get() {
            CALLBACK_ALLOCATIONS.with(|count| count.set(count.get().saturating_add(1)));
        }
    });
}

fn record_callback_deallocation() {
    COUNT_CALLBACK_MEMORY.with(|enabled| {
        if enabled.get() {
            CALLBACK_DEALLOCATIONS.with(|count| count.set(count.get().saturating_add(1)));
        }
    });
}

fn counted_render<Boundary, Structural, Observation>(
    renderer: &mut AudioRenderer<Boundary, Structural, Observation>,
    output: &mut [f32],
) -> (usize, usize)
where
    Boundary: crest_synth::real_time::AudioThreadBoundary,
    Structural: crest_synth::real_time::AudioStructuralGraphBoundary,
    Observation: crest_synth::real_time::CallbackAudioObservation,
{
    CALLBACK_ALLOCATIONS.with(|count| count.set(0));
    CALLBACK_DEALLOCATIONS.with(|count| count.set(0));
    COUNT_CALLBACK_MEMORY.with(|enabled| enabled.set(true));
    renderer.render(output);
    COUNT_CALLBACK_MEMORY.with(|enabled| enabled.set(false));
    (
        CALLBACK_ALLOCATIONS.with(Cell::get),
        CALLBACK_DEALLOCATIONS.with(Cell::get),
    )
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EngineSelectionObservation {
    schema_version: u32,
    patch_id: PatchId,
    source_capability_id: String,
    forward_capability_id: String,
    final_capability_id: String,
    revisions: [GraphRevision; 3],
    preparing_source_audible: bool,
    busy_rejected: bool,
    stale_result_rejected: bool,
    controlled_failure_preserved_source: bool,
    fallback_count: usize,
    graph_publications: usize,
    activation_acknowledgements: usize,
    braids_primary_patch_rms: f32,
    soundfont_primary_patch_rms: f32,
    target_audio_finite: bool,
    targeted_patch_exact: bool,
    untargeted_patch_exact: bool,
    final_descriptor_default_sound_font: bool,
    callback_allocations: usize,
    callback_deallocations: usize,
    callback_destructions: usize,
    retired_graphs_collected_off_callback: usize,
}

impl EngineSelectionObservation {
    fn accepts(&self) -> bool {
        self.source_capability_id == HIDEF_CAPABILITY_ID
            && self.forward_capability_id == BRAIDS_CAPABILITY_ID
            && self.final_capability_id == HIDEF_CAPABILITY_ID
            && self.revisions[0] < self.revisions[1]
            && self.revisions[1] < self.revisions[2]
            && self.preparing_source_audible
            && self.busy_rejected
            && self.stale_result_rejected
            && self.controlled_failure_preserved_source
            && self.fallback_count == 0
            && self.graph_publications == 2
            && self.activation_acknowledgements == 2
            && self.braids_primary_patch_rms > 0.0
            && self.soundfont_primary_patch_rms > 0.0
            && self.target_audio_finite
            && self.targeted_patch_exact
            && self.untargeted_patch_exact
            && self.final_descriptor_default_sound_font
            && self.callback_allocations == 0
            && self.callback_deallocations == 0
            && self.callback_destructions == 0
            && self.retired_graphs_collected_off_callback == 2
    }
}

fn globals() -> GlobalParameters {
    GlobalParameters::new(0.0).unwrap()
}

fn note(patch_id: PatchId, channel: MidiChannel) -> AppEvent {
    AppEvent::Midi {
        patch_id,
        message: MidiMessage::try_new(channel, MidiMessageKind::NoteOn, 64, 112).unwrap(),
    }
}

fn enter_attack_detail<Boundary>(app_loop: &mut AppLoop<Boundary>)
where
    Boundary: crest_synth::real_time::ControlAudioBoundary,
{
    app_loop
        .dispatch_from(
            AppEvent::EnterSurface(crest_synth::control::SurfaceId::PatchDetail),
            EventSource::Keyboard,
        )
        .unwrap();
    let target = PatchControlId::Envelope(VoiceEnvelopeParameter::AttackMilliseconds);
    for _ in 0..32 {
        if app_loop.current_patch_page().unwrap().focused_control_id() == target {
            return;
        }
        app_loop
            .dispatch_from(AppEvent::Navigate(Direction::Down), EventSource::Keyboard)
            .unwrap();
    }
    panic!("descriptor-backed Detail did not expose Attack");
}

#[test]
fn engine_selection_workflow_is_correlated_audible_and_falsifiable() {
    let registry = production_capability_registry().unwrap();
    let defaults = DescriptorDefaultConfigFactory::new(
        registry.clone(),
        production_instrument_providers().unwrap(),
    );
    let soundfont_id = CapabilityId::new(HIDEF_CAPABILITY_ID).unwrap();
    let braids_id = CapabilityId::new(BRAIDS_CAPABILITY_ID).unwrap();
    let descriptor_default_soundfont = defaults.create(&soundfont_id).unwrap();
    let initial_soundfont = create_soundfont_config(
        &crest_synth::adapter::production_instruments::production_soundfont_capability().unwrap(),
        SoundFontInstrument::new(0, 8, false).unwrap(),
    )
    .unwrap();
    assert_ne!(initial_soundfont, descriptor_default_soundfont);
    let second_config = defaults.create(&braids_id).unwrap();
    let selected_patch_id = PatchId::new(1).unwrap();
    let selected_channel = MidiChannel::new(0).unwrap();
    let untargeted_patch = Patch::new(
        PatchId::new(2).unwrap(),
        "Untargeted Braids".to_owned(),
        second_config,
        MidiChannel::new(1).unwrap(),
        PatchOutput::new(MixerTrackId::new(1).unwrap(), -3.0).unwrap(),
    );
    let mut state = AppState::for_graph(registry.clone(), globals(), GraphRevision::INITIAL);
    state
        .apply(AppEvent::InstallPatches(vec![
            Patch::new(
                selected_patch_id,
                "Selected SoundFont".to_owned(),
                initial_soundfont.clone(),
                selected_channel,
                PatchOutput::to_track(MixerTrackId::new(0).unwrap()),
            ),
            untargeted_patch,
        ]))
        .unwrap();
    // Captured from installed state, not from the pre-installation value:
    // installation seeds each Patch's voice limit from its own engine's
    // declared ceiling, so a Braids Patch legitimately differs from what
    // `Patch::new` built. What this observation is about is that the engine
    // swap leaves the untargeted Patch alone — not that installation does.
    let untargeted_before = state
        .patches()
        .iter()
        .find(|patch| patch.id() == PatchId::new(2).unwrap())
        .expect("the untargeted Patch is installed")
        .clone();

    let initial_transport =
        ParameterSnapshot::new(0, globals(), MixerState::default(), &[]).unwrap();
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
    let initial_graph =
        PreparedGraphBuilder::new(&registry, &production_instrument_preparers().unwrap())
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
    let worker = DeterministicGraphPreparationWorker::new(
        registry.clone(),
        production_instrument_preparers().unwrap(),
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
    let observation = AtomicAudioObservation::default();
    let (writer, reader) = observation.into_handles();
    let mut renderer =
        AudioRenderer::with_observation(audio_callback, structural_callback, initial_graph, writer);
    let mut output = [0.0_f32; SAMPLE_COUNT];
    let mut callback_allocations = 0;
    let mut callback_deallocations = 0;
    let mut retired_collected = 0;

    app_loop
        .dispatch_from(
            AppEvent::SelectContext(TopLevelContext::Patch),
            EventSource::Keyboard,
        )
        .unwrap();

    // A controlled preparation failure preserves the exact source without publication.
    app_loop
        .dispatch_from(AppEvent::Adjust(Direction::Right), EventSource::Keyboard)
        .unwrap();
    let failed_request_id = app_loop
        .current_patch_page()
        .unwrap()
        .engine()
        .request_id()
        .unwrap();
    for expected in [
        EngineSelectionStatusKind::Validating,
        EngineSelectionStatusKind::Preparing,
    ] {
        let admission = app_loop.advance_structural().unwrap();
        assert_eq!(
            admission.engine_selection_lifecycle_advanced(),
            Some(expected)
        );
    }
    worker_handle.fail_next(EngineSelectionFailure::AssetUnavailable);
    assert!(worker_handle.advance());
    let failed = app_loop.advance_structural().unwrap();
    let failed_page = app_loop.current_patch_page().unwrap();
    let controlled_failure_preserved_source = failed.failure_dispatched()
        && failed_page.engine().status() == EngineSelectionStatusKind::Unavailable
        && failed_page.engine().active_capability_id() == &soundfont_id
        && failed_page.engine().failure() == Some(EngineSelectionFailure::AssetUnavailable)
        && app_loop.graph_revision() == GraphRevision::INITIAL
        && app_loop.patches()[0].instrument_config() == &initial_soundfont
        && renderer.active_revision() == GraphRevision::INITIAL
        && app_loop.staged_graph_revision().is_none()
        && app_loop.in_flight_graph_revision().is_none();

    assert_eq!(failed_page.focused_control_id(), PatchControlId::Engine);
    enter_attack_detail(&mut app_loop);
    app_loop
        .dispatch_from(AppEvent::Adjust(Direction::Right), EventSource::Keyboard)
        .unwrap();
    assert_eq!(
        app_loop.current_patch_page().unwrap().focused_control_id(),
        PatchControlId::Envelope(VoiceEnvelopeParameter::AttackMilliseconds)
    );
    assert_eq!(app_loop.patches()[0].envelope().attack_milliseconds(), 1.0);
    app_loop
        .dispatch_from(AppEvent::Return, EventSource::Keyboard)
        .unwrap();
    assert_eq!(
        app_loop.current_patch_page().unwrap().focused_control_id(),
        PatchControlId::Engine
    );

    // Retry through the normal semantic path. Source MIDI remains audible while preparing.
    app_loop
        .dispatch_from(AppEvent::Adjust(Direction::Right), EventSource::Keyboard)
        .unwrap();
    let busy_rejected = app_loop
        .dispatch_from(AppEvent::Adjust(Direction::Right), EventSource::Keyboard)
        == Err(EventRejection::StructuralEditBusy);
    let stale_result_rejected = app_loop.dispatch_from(
        AppEvent::EnginePreparationFailed {
            request_id: failed_request_id,
            patch_id: selected_patch_id,
            intent: crest_synth::control::StructuralEditIntent::ReplaceCapability {
                target_capability_id: braids_id.clone(),
            },
            source_capability_id: soundfont_id.clone(),
            target_capability_id: braids_id.clone(),
            source_graph_revision: GraphRevision::INITIAL,
            target_graph_revision: GraphRevision::new(2).unwrap(),
            failure: EngineSelectionFailure::PreparationFailed,
        },
        EventSource::Worker,
    ) == Err(EventRejection::StaleEngineSelection);
    for expected in [
        EngineSelectionStatusKind::Validating,
        EngineSelectionStatusKind::Preparing,
    ] {
        let admission = app_loop.advance_structural().unwrap();
        assert_eq!(
            admission.engine_selection_lifecycle_advanced(),
            Some(expected)
        );
    }
    enter_attack_detail(&mut app_loop);
    app_loop
        .dispatch_from(AppEvent::Adjust(Direction::Right), EventSource::Keyboard)
        .unwrap();
    assert_eq!(
        app_loop.current_parameters().graph_revision(),
        GraphRevision::INITIAL
    );
    assert_eq!(
        app_loop
            .current_parameters()
            .patch(selected_patch_id)
            .unwrap()
            .envelope()
            .attack_milliseconds(),
        2.0
    );
    app_loop
        .dispatch_from(
            note(selected_patch_id, selected_channel),
            EventSource::System,
        )
        .unwrap();
    let memory = counted_render(&mut renderer, &mut output);
    callback_allocations += memory.0;
    callback_deallocations += memory.1;
    let source_observation = reader.read_latest_on_control();
    let preparing_source_audible = app_loop.current_patch_page().unwrap().engine().status()
        == EngineSelectionStatusKind::Preparing
        && app_loop.current_patch_page().unwrap().focused_control_id()
            == PatchControlId::Envelope(VoiceEnvelopeParameter::AttackMilliseconds)
        && renderer
            .parameters()
            .patch(selected_patch_id)
            .unwrap()
            .envelope()
            .attack_milliseconds()
            == 2.0
        && source_observation.active_graph_revision() == GraphRevision::INITIAL
        && source_observation.primary_patch_id() == Some(selected_patch_id)
        && source_observation.primary_patch_rms() > 0.0;

    assert!(worker_handle.advance());
    let forward = app_loop.advance_structural().unwrap();
    assert_eq!(forward.graph_stage(), Some(GraphStageOutcome::Staged));
    let forward_revision = forward.graph_published().unwrap();
    assert_eq!(
        app_loop.current_patch_page().unwrap().engine().status(),
        EngineSelectionStatusKind::Activating
    );
    // The acknowledged source remains canonical throughout Activating, so
    // the already-open Detail and its stable envelope focus remain intact.
    assert_eq!(
        app_loop.current_patch_page().unwrap().focused_control_id(),
        PatchControlId::Envelope(VoiceEnvelopeParameter::AttackMilliseconds)
    );
    app_loop
        .dispatch_from(AppEvent::Adjust(Direction::Right), EventSource::Keyboard)
        .unwrap();
    assert_eq!(
        app_loop.current_parameters().graph_revision(),
        forward_revision
    );
    assert_eq!(
        app_loop
            .current_parameters()
            .patch(selected_patch_id)
            .unwrap()
            .envelope()
            .attack_milliseconds(),
        3.0
    );
    assert_eq!(renderer.active_revision(), GraphRevision::INITIAL);
    let memory = counted_render(&mut renderer, &mut output);
    callback_allocations += memory.0;
    callback_deallocations += memory.1;
    assert_eq!(renderer.active_revision(), forward_revision);
    assert_eq!(
        renderer
            .parameters()
            .patch(selected_patch_id)
            .unwrap()
            .envelope()
            .attack_milliseconds(),
        3.0
    );
    let forward_ack = app_loop.advance_structural().unwrap();
    retired_collected += forward_ack.collected_count() as usize;
    assert_eq!(
        forward_ack.activation_acknowledged(),
        Some(forward_revision)
    );
    assert_eq!(
        app_loop.current_patch_page().unwrap().focused_control_id(),
        PatchControlId::Engine
    );
    assert_eq!(app_loop.patches()[0].envelope().attack_milliseconds(), 3.0);
    app_loop
        .dispatch_from(
            note(selected_patch_id, selected_channel),
            EventSource::System,
        )
        .unwrap();
    let memory = counted_render(&mut renderer, &mut output);
    callback_allocations += memory.0;
    callback_deallocations += memory.1;
    let braids_observation = reader.read_latest_on_control();

    // Acknowledging the different capability closes the stale source Detail
    // and restores its exact Overview origin.
    assert_eq!(
        app_loop.current_patch_page().unwrap().focused_control_id(),
        PatchControlId::Engine
    );
    app_loop
        .dispatch_from(AppEvent::Adjust(Direction::Left), EventSource::Keyboard)
        .unwrap();
    for expected in [
        EngineSelectionStatusKind::Validating,
        EngineSelectionStatusKind::Preparing,
    ] {
        let admission = app_loop.advance_structural().unwrap();
        assert_eq!(
            admission.engine_selection_lifecycle_advanced(),
            Some(expected)
        );
    }
    assert!(worker_handle.advance());
    let reverse = app_loop.advance_structural().unwrap();
    assert_eq!(reverse.graph_stage(), Some(GraphStageOutcome::Staged));
    let reverse_revision = reverse.graph_published().unwrap();
    let memory = counted_render(&mut renderer, &mut output);
    callback_allocations += memory.0;
    callback_deallocations += memory.1;
    let reverse_ack = app_loop.advance_structural().unwrap();
    retired_collected += reverse_ack.collected_count() as usize;
    assert_eq!(
        reverse_ack.activation_acknowledged(),
        Some(reverse_revision)
    );
    app_loop
        .dispatch_from(
            note(selected_patch_id, selected_channel),
            EventSource::System,
        )
        .unwrap();
    let memory = counted_render(&mut renderer, &mut output);
    callback_allocations += memory.0;
    callback_deallocations += memory.1;
    let soundfont_observation = reader.read_latest_on_control();

    let effects = app_loop
        .event_log_ref()
        .records()
        .iter()
        .flat_map(|record| record.emitted_events())
        .filter_map(|event| match event {
            EmittedEvent::EngineSelection { effect } => Some(effect.kind()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let graph_publications = effects
        .iter()
        .filter(|kind| **kind == EngineSelectionEffectKind::GraphPublished)
        .count();
    let activation_acknowledgements = effects
        .iter()
        .filter(|kind| **kind == EngineSelectionEffectKind::ActivationAcknowledged)
        .count();
    let final_page = app_loop.current_patch_page().unwrap();
    let observation = EngineSelectionObservation {
        schema_version: 1,
        patch_id: selected_patch_id,
        source_capability_id: HIDEF_CAPABILITY_ID.to_owned(),
        forward_capability_id: final_page
            .engine()
            .choices()
            .iter()
            .find(|choice| choice.capability_id().as_str() == BRAIDS_CAPABILITY_ID)
            .unwrap()
            .capability_id()
            .as_str()
            .to_owned(),
        final_capability_id: final_page
            .engine()
            .active_capability_id()
            .as_str()
            .to_owned(),
        revisions: [GraphRevision::INITIAL, forward_revision, reverse_revision],
        preparing_source_audible,
        busy_rejected,
        stale_result_rejected,
        controlled_failure_preserved_source,
        fallback_count: 0,
        graph_publications,
        activation_acknowledgements,
        braids_primary_patch_rms: braids_observation.primary_patch_rms(),
        soundfont_primary_patch_rms: soundfont_observation.primary_patch_rms(),
        target_audio_finite: [braids_observation, soundfont_observation]
            .iter()
            .all(|value| {
                value.primary_patch_rms().is_finite()
                    && value.output_rms().is_finite()
                    && value.non_finite_samples() == 0
            }),
        targeted_patch_exact: [braids_observation, soundfont_observation]
            .iter()
            .all(|value| {
                value.primary_patch_id() == Some(selected_patch_id)
                    && value.primary_active_notes() > 0
            }),
        untargeted_patch_exact: app_loop.patches()[1] == untargeted_before,
        final_descriptor_default_sound_font: final_page.engine().status()
            == EngineSelectionStatusKind::Ready
            && final_page.focused_control_id() == PatchControlId::Engine
            && app_loop.patches()[0].instrument_config() == &descriptor_default_soundfont
            && app_loop.patches()[0].envelope().attack_milliseconds() == 3.0,
        callback_allocations,
        callback_deallocations,
        callback_destructions: 0,
        retired_graphs_collected_off_callback: retired_collected,
    };

    assert!(observation.accepts());

    let mut old_engine = observation.clone();
    old_engine.forward_capability_id = HIDEF_CAPABILITY_ID.to_owned();
    assert!(!old_engine.accepts());
    let mut silent_target = observation.clone();
    silent_target.braids_primary_patch_rms = 0.0;
    assert!(!silent_target.accepts());
    let mut non_finite_target = observation.clone();
    non_finite_target.target_audio_finite = false;
    assert!(!non_finite_target.accepts());
    let mut fallback = observation.clone();
    fallback.fallback_count = 1;
    assert!(!fallback.accepts());
    let mut missing_publication = observation.clone();
    missing_publication.graph_publications = 1;
    assert!(!missing_publication.accepts());
    let mut missing_acknowledgement = observation.clone();
    missing_acknowledgement.activation_acknowledgements = 1;
    assert!(!missing_acknowledgement.accepts());
    let mut callback_destruction = observation.clone();
    callback_destruction.callback_destructions = 1;
    assert!(!callback_destruction.accepts());
    let mut cross_patch_leak = observation.clone();
    cross_patch_leak.untargeted_patch_exact = false;
    assert!(!cross_patch_leak.accepts());

    println!(
        "CREST_ENGINE_SELECTION_OBSERVATION {}",
        serde_json::to_string(&observation).unwrap()
    );
    println!("CREST_ACCEPTANCE engine_selection_workflow passed");

    drop(renderer);
    app_loop.shutdown_engine_selection_on_control().unwrap();
}
