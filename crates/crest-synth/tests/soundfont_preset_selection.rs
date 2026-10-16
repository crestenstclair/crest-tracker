use crest_synth::adapter::atomic_audio_observation::AtomicAudioObservation;
use crest_synth::adapter::braids_capability::BraidsCapability;
use crest_synth::adapter::braids_preparer::BraidsPreparer;
use crest_synth::adapter::hidef_soundfont_asset::HiDefSoundFontAsset;
use crest_synth::adapter::hidef_soundfont_capability::{
    HiDefSoundFontCapability, HIDEF_SOUNDFONT_PATH, SOUNDFONT_PRESET_PARAMETER_ID,
};
use crest_synth::adapter::hidef_soundfont_preparer::HiDefSoundFontPreparer;
use crest_synth::adapter::lock_free_audio_boundary::LockFreeAudioBoundary;
use crest_synth::adapter::lock_free_structural_graph_boundary::LockFreeStructuralGraphBoundary;
use crest_synth::control::event_record::EventSource;
use crest_synth::control::{
    AppEvent, AppLoop, AppState, Direction, EngineSelectionFailure, EngineSelectionStatusKind,
    EventRejection, PatchControlId, StateProjector, StructuralEditIntent, SurfaceId,
    TopLevelContext,
};
use crest_synth::kernel::midi_channel::MidiChannel;
use crest_synth::kernel::midi_message::{MidiMessage, MidiMessageKind};
use crest_synth::kernel::patch_id::PatchId;
use crest_synth::mixer::global_parameters::GlobalParameters;
use crest_synth::mixer::mixer_state::MixerState;
use crest_synth::mixer::mixer_track_id::MixerTrackId;
use crest_synth::mixer::patch_output::PatchOutput;
use crest_synth::real_time::{
    AudioBoundary, AudioObservation, AudioRenderer, GraphHandoffStatus, GraphRevision,
    ParameterSnapshot, PreparedGraphBuilder, StructuralGraphBoundary,
};
use crest_synth::shell::audio_output::{AudioDeviceConfig, AudioSampleFormat};
use crest_synth::synth::{
    CapabilityRegistry, DescriptorDefaultConfigFactory, InstrumentCapabilityProvider,
    InstrumentConfig, InstrumentPreparer, ParameterId, ParameterValue, Patch,
    SoundFontPresetCatalog, SoundFontPresetCatalogError, SoundFontPresetId, SoundFontPresetSource,
    VoiceEnvelopeParameter,
};
use crest_synth::testing::DeterministicGraphPreparationWorker;
use rustysynth::SoundFont;
use serde::Serialize;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::collections::BTreeSet;
use std::fs::File;

const SAMPLE_RATE: f32 = 48_000.0;
const FRAME_COUNT: usize = 256;

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
        record_allocation();
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record_allocation();
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        record_deallocation();
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record_allocation();
        record_deallocation();
        unsafe { System.realloc(pointer, layout, size) }
    }
}

fn record_allocation() {
    COUNT_CALLBACK_MEMORY.with(|enabled| {
        if enabled.get() {
            CALLBACK_ALLOCATIONS.with(|count| count.set(count.get().saturating_add(1)));
        }
    });
}

fn record_deallocation() {
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PresetObservation {
    schema_version: u32,
    catalog_entries: usize,
    exact_authored_names: bool,
    numeric_order: bool,
    raw_order_discriminates: bool,
    alphabetical_order_discriminates: bool,
    choice_ids_round_trip: bool,
    duplicate_behavior_exact: bool,
    parse_count: usize,
    focused_control_id: String,
    source_choice_id: String,
    target_choice_id: String,
    preparing_source_audible: bool,
    busy_rejected: bool,
    stale_rejected: bool,
    early_ack_rejected: bool,
    controlled_failure_preserved_source: bool,
    exact_one_assignment_commit: bool,
    scalar_edit_merged: bool,
    target_audio_finite_nonzero_distinct: bool,
    restored_descriptor_default: bool,
    lower_boundary_rejected: bool,
    upper_boundary_rejected: bool,
    callback_allocations: usize,
    callback_deallocations: usize,
    final_revision: u64,
}

fn globals() -> GlobalParameters {
    GlobalParameters::new(0.0).unwrap()
}

fn note(patch_id: PatchId, channel: MidiChannel) -> AppEvent {
    AppEvent::Midi {
        patch_id,
        message: MidiMessage::try_new(channel, MidiMessageKind::NoteOn, 60, 112).unwrap(),
    }
}

fn rms(samples: &[f32]) -> f32 {
    (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32).sqrt()
}

fn synthetic_catalog_predicates() -> bool {
    let source = |ordinal, bank, program, name: &str, playable| {
        SoundFontPresetSource::new(ordinal, bank, program, name, playable)
    };
    let catalog = SoundFontPresetCatalog::from_sources([
        source(0, 128, 0, "Kit", true),
        source(1, 0, 2, "Duplicate label", true),
        source(2, 0, 1, "Duplicate label", true),
        source(3, 0, 1, "Shadow", true),
        source(4, 0, 0, "Ignored", false),
    ])
    .unwrap();
    let ids = catalog
        .entries()
        .iter()
        .map(|entry| entry.id())
        .collect::<Vec<_>>();
    ids == vec![
        SoundFontPresetId::new(0, 1).unwrap(),
        SoundFontPresetId::new(0, 2).unwrap(),
        SoundFontPresetId::new(128, 0).unwrap(),
    ] && catalog.entries()[0].name() == "Duplicate label"
        && catalog.entries()[1].name() == "Duplicate label"
        && catalog.entries()[2].id().is_percussion()
        && catalog.coordinate_collisions().len() == 1
        && catalog.coordinate_collisions()[0].retained_source_ordinal() == 2
        && catalog.coordinate_collisions()[0].shadowed_source_ordinal() == 3
        && matches!(
            SoundFontPresetCatalog::from_sources([source(0, 0, 0, "", true)]),
            Err(SoundFontPresetCatalogError::EmptyName { .. })
        )
        && matches!(
            SoundFontPresetCatalog::from_sources([source(0, -1, 0, "Bad", true)]),
            Err(SoundFontPresetCatalogError::BankOutOfRange { .. })
        )
        && matches!(
            SoundFontPresetCatalog::from_sources([source(0, 65_536, 0, "Bad", true)]),
            Err(SoundFontPresetCatalogError::BankOutOfRange { .. })
        )
        && matches!(
            SoundFontPresetCatalog::from_sources([source(0, 0, 128, "Bad", true)]),
            Err(SoundFontPresetCatalogError::ProgramOutOfRange { .. })
        )
        && matches!(
            SoundFontPresetCatalog::from_sources([source(0, 0, 0, "Empty zone", false)]),
            Err(SoundFontPresetCatalogError::EmptyCatalog)
        )
        && [
            "sf2.bank-00.program-0",
            "sf2.bank-0.program-128",
            "sf2.bank-65536.program-0",
            "sf2.bank--1.program-0",
            "sf2.bank-0.program-0.extra",
        ]
        .iter()
        .all(|value| value.parse::<SoundFontPresetId>().is_err())
}

fn raw_effective_presets() -> Vec<(SoundFontPresetId, String, usize)> {
    let mut file = File::open(HIDEF_SOUNDFONT_PATH).unwrap();
    let sound_font = SoundFont::new(&mut file).unwrap();
    let mut retained = BTreeSet::new();
    let mut result = Vec::new();
    for (source_ordinal, preset) in sound_font.get_presets().iter().enumerate() {
        let playable = preset.get_regions().iter().any(|preset_region| {
            let Some(instrument) = sound_font
                .get_instruments()
                .get(preset_region.get_instrument_id())
            else {
                return false;
            };
            instrument.get_regions().iter().any(|instrument_region| {
                let key_start = preset_region
                    .get_key_range_start()
                    .max(instrument_region.get_key_range_start());
                let key_end = preset_region
                    .get_key_range_end()
                    .min(instrument_region.get_key_range_end());
                let velocity_start = preset_region
                    .get_velocity_range_start()
                    .max(instrument_region.get_velocity_range_start());
                let velocity_end = preset_region
                    .get_velocity_range_end()
                    .min(instrument_region.get_velocity_range_end());
                key_start <= key_end
                    && velocity_start <= velocity_end
                    && sound_font
                        .get_sample_headers()
                        .get(instrument_region.get_sample_id())
                        .is_some()
            })
        });
        if !playable {
            continue;
        }
        let id = SoundFontPresetId::new(
            u16::try_from(preset.get_bank_number()).unwrap(),
            u8::try_from(preset.get_patch_number()).unwrap(),
        )
        .unwrap();
        if retained.insert(id) {
            result.push((id, preset.get_name().to_owned(), source_ordinal));
        }
    }
    result
}

fn config_diff_is_only_preset(source: &InstrumentConfig, candidate: &InstrumentConfig) -> bool {
    let preset = ParameterId::new(SOUNDFONT_PRESET_PARAMETER_ID).unwrap();
    source.capability_id() == candidate.capability_id()
        && source.asset_references() == candidate.asset_references()
        && source.values().len() == candidate.values().len()
        && source
            .values()
            .iter()
            .zip(candidate.values())
            .filter(|(left, right)| left != right)
            .count()
            == 1
        && source
            .values()
            .iter()
            .zip(candidate.values())
            .all(|(left, right)| {
                left.parameter_id() == right.parameter_id()
                    && (left == right || left.parameter_id() == &preset)
            })
}

#[test]
fn soundfont_preset_selection() {
    let asset = HiDefSoundFontAsset::load().unwrap();
    let catalog = asset.catalog();
    let raw = raw_effective_presets();
    let mut expected = raw.clone();
    expected.sort_by_key(|entry| entry.0);
    let catalog_tuples = catalog
        .entries()
        .iter()
        .map(|entry| (entry.id(), entry.name().to_owned(), entry.source_ordinal()))
        .collect::<Vec<_>>();
    let exact_authored_names = catalog_tuples == expected;
    let numeric_order = catalog
        .entries()
        .windows(2)
        .all(|pair| pair[0].id() < pair[1].id());
    let raw_order_discriminates = raw != expected;
    let mut alphabetical = expected.clone();
    alphabetical.sort_by(|left, right| left.1.cmp(&right.1).then(left.0.cmp(&right.0)));
    let alphabetical_order_discriminates = alphabetical != expected;
    let choice_ids_round_trip = catalog.entries().iter().all(|entry| {
        entry.choice_id().parse::<SoundFontPresetId>() == Ok(entry.id())
            && catalog.resolve_choice_id(&entry.choice_id()) == Ok(entry.id())
    });
    assert!(exact_authored_names);
    assert!(numeric_order);
    assert!(raw_order_discriminates);
    assert!(alphabetical_order_discriminates);
    assert!(choice_ids_round_trip);
    assert!(synthetic_catalog_predicates());

    let soundfont_capability = HiDefSoundFontCapability::new(asset.catalog()).unwrap();
    let braids_capability = BraidsCapability::new().unwrap();
    let registry = CapabilityRegistry::new(vec![
        soundfont_capability.descriptor(),
        braids_capability.descriptor(),
    ])
    .unwrap();
    let providers: Vec<Box<dyn InstrumentCapabilityProvider>> = vec![
        Box::new(soundfont_capability.clone()),
        Box::new(braids_capability.clone()),
    ];
    let config_factory = DescriptorDefaultConfigFactory::new(registry.clone(), providers);
    let default_config = config_factory
        .create(soundfont_capability.descriptor().id())
        .unwrap();
    let preset_parameter = ParameterId::new(SOUNDFONT_PRESET_PARAMETER_ID).unwrap();
    let source_choice_id = match default_config.value(&preset_parameter).unwrap() {
        ParameterValue::Choice(value) => value.clone(),
        _ => panic!("preset default is not a Choice"),
    };
    let target_choice_id = catalog.entries()[1].choice_id();
    assert_ne!(source_choice_id, target_choice_id);
    let patch_id = PatchId::new(1).unwrap();
    let channel = MidiChannel::new(0).unwrap();
    let untargeted = Patch::new(
        PatchId::new(2).unwrap(),
        "Untargeted Braids".to_owned(),
        braids_capability.default_config().unwrap(),
        MidiChannel::new(1).unwrap(),
        PatchOutput::new(MixerTrackId::new(1).unwrap(), -4.0).unwrap(),
    );
    let mut state = AppState::for_graph(registry.clone(), globals(), GraphRevision::INITIAL);
    state
        .apply(AppEvent::InstallPatches(vec![
            Patch::new(
                patch_id,
                "Preset target".to_owned(),
                default_config.clone(),
                channel,
                PatchOutput::to_track(MixerTrackId::new(0).unwrap()),
            ),
            untargeted,
        ]))
        .unwrap();
    // Captured from installed state: installation seeds each Patch's voice
    // limit from its own engine's declared ceiling, so an installed Patch
    // legitimately differs from what the constructor built. What this proves
    // is that the preset commit leaves the untargeted Patch alone.
    let untargeted_before = state.patches()[1].clone();

    let boundary = LockFreeAudioBoundary::new(
        128,
        ParameterSnapshot::new(0, globals(), MixerState::default(), &[]).unwrap(),
    );
    let (audio_control, audio_callback) = boundary.into_handles();
    let mut app_loop = AppLoop::new(
        state,
        StateProjector::for_graph(GraphRevision::INITIAL),
        audio_control,
    )
    .unwrap();
    let audio_config =
        AudioDeviceConfig::new(SAMPLE_RATE, 2, AudioSampleFormat::F32, FRAME_COUNT).unwrap();
    let initial_preparers: Vec<Box<dyn InstrumentPreparer>> = vec![
        Box::new(HiDefSoundFontPreparer::new(&asset).unwrap()),
        Box::new(BraidsPreparer::new().unwrap()),
    ];
    let initial_graph = PreparedGraphBuilder::new(&registry, &initial_preparers)
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
    let worker_preparers: Vec<Box<dyn InstrumentPreparer>> = vec![
        Box::new(HiDefSoundFontPreparer::new(&asset).unwrap()),
        Box::new(BraidsPreparer::new().unwrap()),
    ];
    let worker =
        DeterministicGraphPreparationWorker::new(registry.clone(), worker_preparers, audio_config);
    let worker_handle = worker.advance_handle();
    app_loop
        .configure_engine_selection(
            config_factory,
            worker,
            structural_control,
            &initial_graph,
            audio_config,
        )
        .unwrap();
    let observation = AtomicAudioObservation::default();
    let (observation_writer, _observation_reader) = observation.into_handles();
    let mut renderer = AudioRenderer::with_observation(
        audio_callback,
        structural_callback,
        initial_graph,
        observation_writer,
    );
    let mut output = vec![0.0_f32; FRAME_COUNT * 2];

    app_loop
        .dispatch_from(
            AppEvent::SelectContext(TopLevelContext::Patch),
            EventSource::Keyboard,
        )
        .unwrap();
    app_loop
        .dispatch_from(
            AppEvent::EnterSurface(SurfaceId::PatchDetail),
            EventSource::Keyboard,
        )
        .unwrap();
    let preset_control = PatchControlId::Capability(preset_parameter.clone());
    for _ in 0..64 {
        if app_loop.current_patch_page().unwrap().focused_control_id() == preset_control {
            break;
        }
        app_loop
            .dispatch_from(AppEvent::Navigate(Direction::Down), EventSource::Keyboard)
            .unwrap();
    }
    assert_eq!(
        app_loop.current_patch_page().unwrap().focused_control_id(),
        preset_control
    );
    let lower_boundary_rejected = app_loop
        .dispatch_from(AppEvent::Adjust(Direction::Left), EventSource::Keyboard)
        == Err(EventRejection::ParameterAtBoundary);

    app_loop
        .dispatch_from(AppEvent::Adjust(Direction::Right), EventSource::Keyboard)
        .unwrap();
    let first_correlation = app_loop
        .engine_selection_status()
        .correlation()
        .unwrap()
        .clone();
    assert_eq!(
        first_correlation.intent(),
        &StructuralEditIntent::ReplaceParameterChoice {
            capability_id: default_config.capability_id().clone(),
            parameter_id: preset_parameter.clone(),
            choice_id: target_choice_id.clone(),
        }
    );
    let busy_rejected = app_loop
        .dispatch_from(AppEvent::Adjust(Direction::Right), EventSource::Keyboard)
        == Err(EventRejection::StructuralEditBusy);
    let before_stale = app_loop.current_state_tree().json().to_owned();
    let stale_rejected = app_loop.dispatch_from(
        AppEvent::EnginePreparationFailed {
            request_id: first_correlation.request_id().checked_next().unwrap(),
            patch_id,
            intent: first_correlation.intent().clone(),
            source_capability_id: first_correlation.source_capability_id().unwrap().clone(),
            target_capability_id: first_correlation.target_capability_id().unwrap().clone(),
            source_graph_revision: first_correlation.source_graph_revision(),
            target_graph_revision: GraphRevision::new(2).unwrap(),
            failure: EngineSelectionFailure::PresetUnavailable,
        },
        EventSource::Worker,
    ) == Err(EventRejection::StaleEngineSelection)
        && app_loop.current_state_tree().json() == before_stale;
    let early_ack_rejected = app_loop.dispatch_from(
        AppEvent::EngineActivationAcknowledged {
            request_id: first_correlation.request_id(),
            intent: first_correlation.intent().clone(),
            target_graph_revision: GraphRevision::new(2).unwrap(),
            retired_graph_revision: GraphRevision::INITIAL,
            collected: true,
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
    worker_handle.fail_next(EngineSelectionFailure::PresetUnavailable);
    assert!(worker_handle.advance());
    let failure_progress = app_loop.advance_structural().unwrap();
    let failed_page = app_loop.current_patch_page().unwrap();
    let failed_row = failed_page
        .sections()
        .iter()
        .flat_map(|section| section.parameters())
        .find(|row| row.id() == &preset_parameter)
        .unwrap();
    let controlled_failure_preserved_source = failure_progress.failure_dispatched()
        && app_loop.patches()[0].instrument_config() == &default_config
        && app_loop.graph_revision() == GraphRevision::INITIAL
        && app_loop.engine_selection_status().kind() == EngineSelectionStatusKind::Unavailable
        && failed_row.status() == Some(EngineSelectionStatusKind::Unavailable)
        && failed_row.failure() == Some(EngineSelectionFailure::PresetUnavailable);

    app_loop
        .dispatch_from(AppEvent::Adjust(Direction::Right), EventSource::Keyboard)
        .unwrap();
    let retry_correlation = app_loop
        .engine_selection_status()
        .correlation()
        .unwrap()
        .clone();
    assert_eq!(app_loop.patches()[0].instrument_config(), &default_config);
    let release_before = app_loop.patches()[0].envelope().release_milliseconds();
    let release_control = PatchControlId::Envelope(VoiceEnvelopeParameter::ReleaseMilliseconds);
    for _ in 0..64 {
        if app_loop.current_patch_page().unwrap().focused_control_id() == release_control {
            break;
        }
        app_loop
            .dispatch_from(AppEvent::Navigate(Direction::Down), EventSource::Keyboard)
            .unwrap();
    }
    assert_eq!(
        app_loop.current_patch_page().unwrap().focused_control_id(),
        release_control
    );
    app_loop
        .dispatch_from(AppEvent::Adjust(Direction::Up), EventSource::Keyboard)
        .unwrap();
    let release_after = app_loop.patches()[0].envelope().release_milliseconds();
    assert!(release_after > release_before);
    app_loop
        .dispatch_from(note(patch_id, channel), EventSource::System)
        .unwrap();
    let source_memory = counted_render(&mut renderer, &mut output);
    let source_audio = output.clone();
    let source_rms = rms(&source_audio);
    let preparing_source_audible = source_rms > 0.0
        && source_audio.iter().all(|sample| sample.is_finite())
        && app_loop.patches()[0].instrument_config() == &default_config;

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
    let prepared = app_loop.advance_structural().unwrap();
    assert!(prepared.worker_result_polled());
    let target_revision = prepared.graph_published().unwrap();
    assert_eq!(
        app_loop.patches()[0].instrument_config(),
        &default_config,
        "Activating retains the acknowledged source configuration"
    );
    let swap_memory = counted_render(&mut renderer, &mut output);
    assert_eq!(renderer.active_revision(), target_revision);
    let scalar_edit_merged = renderer
        .parameters()
        .patch(patch_id)
        .is_some_and(|parameters| parameters.envelope().release_milliseconds() == release_after);
    let acknowledged = app_loop.advance_structural().unwrap();
    assert_eq!(
        acknowledged.activation_acknowledged(),
        Some(target_revision)
    );
    let target_config = app_loop.patches()[0].instrument_config().clone();
    let exact_one_assignment_commit = config_diff_is_only_preset(&default_config, &target_config)
        && target_config.value(&preset_parameter)
            == Some(&ParameterValue::Choice(target_choice_id.clone()))
        && app_loop.patches()[1] == untargeted_before
        && retry_correlation.intent()
            == &StructuralEditIntent::ReplaceParameterChoice {
                capability_id: default_config.capability_id().clone(),
                parameter_id: preset_parameter.clone(),
                choice_id: target_choice_id.clone(),
            };
    app_loop
        .dispatch_from(note(patch_id, channel), EventSource::System)
        .unwrap();
    let target_memory = counted_render(&mut renderer, &mut output);
    let target_audio = output.clone();
    let target_rms = rms(&target_audio);
    let target_audio_finite_nonzero_distinct = target_rms > 0.0
        && target_audio.iter().all(|sample| sample.is_finite())
        && source_audio
            .iter()
            .zip(&target_audio)
            .any(|(source, target)| (source - target).abs() > 1.0e-6);

    for _ in 0..64 {
        if app_loop.current_patch_page().unwrap().focused_control_id() == preset_control {
            break;
        }
        app_loop
            .dispatch_from(AppEvent::Navigate(Direction::Up), EventSource::Keyboard)
            .unwrap();
    }
    assert_eq!(
        app_loop.current_patch_page().unwrap().focused_control_id(),
        preset_control
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
    let restore_prepared = app_loop.advance_structural().unwrap();
    let restore_revision = restore_prepared.graph_published().unwrap();
    let restore_memory = counted_render(&mut renderer, &mut output);
    let restore_ack = app_loop.advance_structural().unwrap();
    assert_eq!(
        restore_ack.activation_acknowledged(),
        Some(restore_revision)
    );
    let restored_descriptor_default = app_loop.patches()[0].instrument_config() == &default_config
        && app_loop.engine_selection_status().kind() == EngineSelectionStatusKind::Ready;

    let factory = DescriptorDefaultConfigFactory::new(
        registry.clone(),
        vec![
            Box::new(soundfont_capability.clone()),
            Box::new(braids_capability),
        ],
    );
    let last_config = factory
        .replace_structural_choice(
            &default_config,
            &preset_parameter,
            &catalog.entries().last().unwrap().choice_id(),
        )
        .unwrap();
    let mut upper_state = AppState::new(registry, globals());
    upper_state
        .apply(AppEvent::InstallPatches(vec![Patch::new(
            PatchId::new(7).unwrap(),
            "Upper boundary".to_owned(),
            last_config,
            MidiChannel::new(7).unwrap(),
            PatchOutput::to_track(MixerTrackId::new(7).unwrap()),
        )]))
        .unwrap();
    upper_state
        .apply(AppEvent::SelectContext(TopLevelContext::Patch))
        .unwrap();
    upper_state
        .apply(AppEvent::EnterSurface(SurfaceId::PatchDetail))
        .unwrap();
    for _ in 0..64 {
        if matches!(
            upper_state.interaction().focus_path().control_id(),
            crest_synth::control::SemanticControlId::Patch(control) if control == &preset_control
        ) {
            break;
        }
        upper_state
            .apply(AppEvent::Navigate(Direction::Down))
            .unwrap();
    }
    assert!(matches!(
        upper_state.interaction().focus_path().control_id(),
        crest_synth::control::SemanticControlId::Patch(control) if control == &preset_control
    ));
    let upper_boundary_rejected = upper_state.apply(AppEvent::Adjust(Direction::Right))
        == Err(EventRejection::ParameterAtBoundary);

    let callback_allocations = source_memory.0 + swap_memory.0 + target_memory.0 + restore_memory.0;
    let callback_deallocations =
        source_memory.1 + swap_memory.1 + target_memory.1 + restore_memory.1;
    let result = PresetObservation {
        schema_version: 1,
        catalog_entries: catalog.entries().len(),
        exact_authored_names,
        numeric_order,
        raw_order_discriminates,
        alphabetical_order_discriminates,
        choice_ids_round_trip,
        duplicate_behavior_exact: synthetic_catalog_predicates(),
        parse_count: asset.parse_count(),
        focused_control_id: preset_control.to_string(),
        source_choice_id,
        target_choice_id,
        preparing_source_audible,
        busy_rejected,
        stale_rejected,
        early_ack_rejected,
        controlled_failure_preserved_source,
        exact_one_assignment_commit,
        scalar_edit_merged,
        target_audio_finite_nonzero_distinct,
        restored_descriptor_default,
        lower_boundary_rejected,
        upper_boundary_rejected,
        callback_allocations,
        callback_deallocations,
        final_revision: restore_revision.value(),
    };
    assert!(result.catalog_entries > 1);
    assert!(result.exact_authored_names);
    assert!(result.numeric_order);
    assert!(result.raw_order_discriminates);
    assert!(result.alphabetical_order_discriminates);
    assert!(result.choice_ids_round_trip);
    assert!(result.duplicate_behavior_exact);
    assert_eq!(result.parse_count, 1);
    assert_eq!(
        result.focused_control_id,
        "patch.capability.soundfont.preset"
    );
    assert!(result.preparing_source_audible);
    assert!(result.busy_rejected);
    assert!(result.stale_rejected);
    assert!(result.early_ack_rejected);
    assert!(result.controlled_failure_preserved_source);
    assert!(result.exact_one_assignment_commit);
    assert!(result.scalar_edit_merged);
    assert!(result.target_audio_finite_nonzero_distinct);
    assert!(result.restored_descriptor_default);
    assert!(result.lower_boundary_rejected);
    assert!(result.upper_boundary_rejected);
    assert_eq!(result.callback_allocations, 0);
    assert_eq!(result.callback_deallocations, 0);
    println!(
        "CREST_SOUNDFONT_PRESET_OBSERVATION {}",
        serde_json::to_string(&result).unwrap()
    );
    println!("CREST_ACCEPTANCE soundfont_preset_selection passed");
}
