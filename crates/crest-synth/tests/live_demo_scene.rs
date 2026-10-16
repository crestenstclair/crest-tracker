#[allow(dead_code)]
mod support;

use crest_synth::adapter::atomic_audio_observation::AtomicAudioObservation;
use crest_synth::adapter::lock_free_audio_boundary::LockFreeAudioBoundary;
use crest_synth::adapter::lock_free_structural_graph_boundary::LockFreeStructuralGraphBoundary;
use crest_synth::adapter::production_effects::{
    production_effect_preparers, production_effect_providers, production_effect_registry,
};
use crest_synth::adapter::production_instruments::{
    production_capability_registry, production_instrument_preparers,
    production_instrument_providers,
};
use crest_synth::adapter::threaded_graph_preparation_worker::ThreadedGraphPreparationWorker;
use crest_synth::control::app_event::AppEvent;
use crest_synth::control::app_loop::AppLoop;
use crest_synth::control::app_state::AppState;
use crest_synth::control::event_log::EventLog;
use crest_synth::control::event_record::{EmittedEvent, EventInput, EventOutcome, EventSource};
use crest_synth::control::state_projector::StateProjector;
use crest_synth::control::{GraphicalShellProjection, PatchControlId, StructuralEditIntent};
use crest_synth::kernel::midi_message::MidiMessageKind;
use crest_synth::mixer::global_parameters::GlobalParameters;
use crest_synth::mixer::mixer_state::MixerState;
use crest_synth::mixer::mixer_track_id::MixerTrackId;
use crest_synth::mixer::mixer_track_parameters::MixerTrackParameter;
use crest_synth::mixer::patch_output::PatchOutputParameter;
use crest_synth::real_time::audio_boundary::AudioBoundary;
use crest_synth::real_time::audio_observation::AudioObservation;
use crest_synth::real_time::audio_renderer::AudioRenderer;
use crest_synth::real_time::graph_revision::GraphRevision;
use crest_synth::real_time::parameter_snapshot::ParameterSnapshot;
use crest_synth::real_time::prepared_graph_builder::PreparedGraphBuilder;
use crest_synth::real_time::structural_graph_boundary::NoStructuralGraphChanges;
use crest_synth::real_time::{GraphHandoffStatus, StructuralGraphBoundary};
use crest_synth::shell::audio_output::{AudioDeviceConfig, AudioSampleFormat};
use crest_synth::shell::{
    ShellFrameObservation, ShellRegionId, ShellRegionObservation, ShellRegionRect,
};
use crest_synth::synth::{
    DescriptorDefaultConfigFactory, InstrumentPreparer, PatchEditableTarget, VoiceEnvelope,
};
use crest_synth::testing::automatic_midi_test::AutomaticMidiTest;
use crest_synth::testing::live_demo_runner::LiveDemoRunner;
use crest_synth::testing::live_demo_scene::LiveDemoScene;
use crest_synth::testing::{LiveDemoError, RuntimeAudioWitness};
use std::time::Duration;
use support::{globals, FixtureMidiSource, FixturePreparer, FRAME_COUNT, SAMPLE_RATE};

fn shell_frame(projection: &GraphicalShellProjection) -> ShellFrameObservation {
    ShellFrameObservation::try_new_semantic(
        1_920.0,
        1_080.0,
        projection.semantic_model(),
        [
            ShellRegionObservation::new(
                ShellRegionId::ContextLine,
                ShellRegionRect::new(0.0, 0.0, 1_920.0, 48.0),
                projection.context_line().context_label(),
            ),
            ShellRegionObservation::new(
                ShellRegionId::IdentityHeader,
                ShellRegionRect::new(0.0, 48.0, 1_920.0, 120.0),
                projection.identity_header().primary_label(),
            ),
            ShellRegionObservation::new(
                ShellRegionId::MainWorkspace,
                ShellRegionRect::new(0.0, 120.0, 1_500.0, 1_016.0),
                projection.workspace().main_label(),
            ),
            ShellRegionObservation::new(
                ShellRegionId::PersistentSideRegion,
                ShellRegionRect::new(1_500.0, 120.0, 1_920.0, 1_016.0),
                projection.workspace().side_label(),
            ),
            ShellRegionObservation::new(
                ShellRegionId::Footer,
                ShellRegionRect::new(0.0, 1_016.0, 1_920.0, 1_080.0),
                projection.footer().path_label(),
            ),
        ],
    )
    .unwrap()
}

#[test]
fn live_demo_scene_uses_production_state_projection_render_and_observation_paths() {
    let global = globals();
    let initial = ParameterSnapshot::new(0, global, MixerState::default(), &[])
        .expect("initial parameters are valid");
    let boundary = LockFreeAudioBoundary::new(512, initial);
    let (control, audio) = boundary.into_handles();
    let event_log = EventLog::new(4096).expect("live test journal capacity is valid");
    let providers = production_instrument_providers().expect("production providers are valid");
    let registry = production_capability_registry().expect("production registry is valid");
    let effect_providers =
        production_effect_providers().expect("production effect providers are valid");
    let effects = production_effect_registry().expect("production effect registry is valid");
    let mut app_loop = AppLoop::with_event_log(
        AppState::new_with_effects(registry.clone(), effects.clone(), global).with_initial_returns(
            crest_synth::adapter::production_effects::startup_bus_returns(&effects),
        ),
        StateProjector::for_graph(GraphRevision::INITIAL),
        control,
        event_log,
    )
    .expect("initial application state projects");

    let mut automatic = AutomaticMidiTest::new(FixtureMidiSource::new());
    automatic
        .initialize_with_effects(&providers, &effect_providers, &mut app_loop)
        .expect("fixture initializes through AppLoop");
    let retained_scene = LiveDemoScene::from_installed_state(&app_loop.current_state_tree())
        .expect("installed fixture produces the retained live scene");
    let scene = LiveDemoScene::mixer_from_installed_state(&app_loop.current_state_tree())
        .expect("installed fixture produces the dedicated Mixer live scene");
    assert_eq!(scene.name(), "live-mixer");
    assert_eq!(scene.steps(), retained_scene.steps());
    assert_eq!(
        scene.expected_editable_parameters(),
        retained_scene.expected_editable_parameters()
    );

    let expected_count = app_loop
        .patches()
        .first()
        .map(|patch| {
            let descriptor = app_loop
                .capabilities()
                .descriptor(patch.instrument_config().capability_id())
                .unwrap();
            patch.editable_targets(descriptor).unwrap().len()
        })
        .unwrap()
        + PatchOutputParameter::ALL.len()
        + app_loop
            .patches()
            .first()
            .into_iter()
            .flat_map(|patch| patch.effect_slots().iter().flatten())
            .map(|config| {
                app_loop
                    .effects()
                    .descriptor(config.capability_id())
                    .unwrap()
                    .scalar_parameter_count()
            })
            .sum::<usize>()
        // The four MAIN classes plus the two production-audible indexed
        // sends (buses 0 and 1, whose returns are occupied by default).
        + MixerTrackId::COUNT * (MixerTrackParameter::MAIN.len() + 2)
        + GlobalParameters::surface_descriptor().len();
    assert_eq!(scene.expected_editable_parameters().len(), expected_count);
    assert_eq!(scene.minimum_parameter_dwell(), Duration::from_millis(500));
    assert_eq!(
        scene
            .expected_engine_transitions()
            .iter()
            .map(|transition| transition.identifier())
            .collect::<Vec<_>>(),
        [
            "SoundFontPresetToNext",
            "SoundFontToBraids",
            "BraidsToDescriptorDefaultSoundFont",
        ]
    );
    assert!(matches!(
        scene.expected_engine_transitions()[0].intent(),
        StructuralEditIntent::ReplaceParameterChoice { .. }
    ));
    assert!(scene.expected_engine_transitions()[0]
        .source_label()
        .is_some());
    assert!(scene.expected_engine_transitions()[0]
        .target_label()
        .is_some());
    let focused_patch_id = app_loop.patches()[0].id();
    for parameter in scene.expected_editable_parameters() {
        let (checkpoint_index, _) = scene
            .steps()
            .iter()
            .enumerate()
            .find(|(_, step)| {
                step.requires_checkpoint() && step.editable_parameter() == Some(parameter)
            })
            .expect("every frozen parameter has one checkpoint");
        let expected_probe_patch = match parameter {
            crest_synth::testing::live_demo_scene::LiveEditableParameter::Patch {
                patch_id,
                ..
            } => *patch_id,
            crest_synth::testing::live_demo_scene::LiveEditableParameter::PatchOutput {
                patch_id,
                ..
            } => *patch_id,
            crest_synth::testing::live_demo_scene::LiveEditableParameter::Track { .. }
            | crest_synth::testing::live_demo_scene::LiveEditableParameter::Send { .. } => {
                focused_patch_id
            }
            crest_synth::testing::live_demo_scene::LiveEditableParameter::Global { .. } => {
                focused_patch_id
            }
            crest_synth::testing::live_demo_scene::LiveEditableParameter::Effect {
                patch_id,
                ..
            } => *patch_id,
        };
        let (note_on_patch, note_on) = match scene.steps()[checkpoint_index - 1].event() {
            AppEvent::Midi { patch_id, message } => (*patch_id, *message),
            other => panic!("checkpoint probe is not semantic MIDI: {other:?}"),
        };
        let (note_off_patch, note_off) = match scene.steps()[checkpoint_index + 1].event() {
            AppEvent::Midi { patch_id, message } => (*patch_id, *message),
            other => panic!("checkpoint release is not semantic MIDI: {other:?}"),
        };
        assert_eq!(note_on_patch, expected_probe_patch);
        assert_eq!(note_off_patch, expected_probe_patch);
        assert_eq!(note_on.kind(), MidiMessageKind::NoteOn);
        assert_eq!(note_off.kind(), MidiMessageKind::NoteOff);
        assert_eq!(note_on.data1(), note_off.data1());
        assert!(note_on.data2() > 0);
        assert_eq!(note_off.data2(), 0);
    }
    let shared_track = MixerTrackId::new(1).unwrap();
    let shared_level_index = scene
        .steps()
        .iter()
        .position(|step| {
            matches!(
                step.editable_parameter(),
                Some(
                    crest_synth::testing::live_demo_scene::LiveEditableParameter::Track {
                        track_id,
                        parameter: MixerTrackParameter::Level,
                    }
                ) if *track_id == shared_track
            )
        })
        .expect("the shared-track Level checkpoint exists");
    let shared_patch_id = app_loop.patches()[1].id();
    assert!(matches!(
        scene.steps()[shared_level_index - 2].event(),
        AppEvent::Midi { patch_id, message }
            if *patch_id == shared_patch_id
                && message.kind() == MidiMessageKind::NoteOn
                && message.data2() > 0
    ));
    assert!(matches!(
        scene.steps()[shared_level_index + 3].event(),
        AppEvent::Midi { patch_id, message }
            if *patch_id == shared_patch_id
                && message.kind() == MidiMessageKind::NoteOff
                && message.data2() == 0
    ));
    let mut patch_adsr_step_indices = Vec::new();
    for descriptor in VoiceEnvelope::surface_descriptor() {
        let control = PatchControlId::Envelope(descriptor.parameter());
        let matches = scene
            .steps()
            .iter()
            .enumerate()
            .filter(|(_, step)| {
                step.requires_checkpoint()
                    && step.patch_control_id() == Some(control.clone())
                    && matches!(
                        step.editable_parameter(),
                        Some(crest_synth::testing::live_demo_scene::LiveEditableParameter::Patch {
                            patch_id,
                            target: PatchEditableTarget::Envelope(parameter),
                        }) if *patch_id == focused_patch_id && *parameter == descriptor.parameter()
                    )
            })
            .collect::<Vec<_>>();
        assert_eq!(matches.len(), 1, "{control}");
        patch_adsr_step_indices.push(matches[0].0);
    }
    assert!(patch_adsr_step_indices
        .windows(2)
        .all(|pair| pair[0] < pair[1]));
    let rejection_index = scene
        .steps()
        .iter()
        .position(|step| step.expected_outcome() == EventOutcome::Rejected)
        .expect("scene includes a boundary rejection probe");
    let recovery = scene
        .steps()
        .get(rejection_index + 1)
        .expect("an accepted recovery follows the boundary rejection");
    assert_eq!(recovery.expected_outcome(), EventOutcome::Accepted);
    assert!(matches!(recovery.event(), AppEvent::Return));

    let observation = AtomicAudioObservation::default();
    let (writer, reader) = observation.into_handles();
    let preparers = production_instrument_preparers().expect("production preparers are valid");
    let effect_preparers =
        production_effect_preparers().expect("production effect preparers are valid");
    let audio_config = AudioDeviceConfig::new(SAMPLE_RATE, 2, AudioSampleFormat::F32, FRAME_COUNT)
        .expect("live test audio configuration is valid");
    let graph = PreparedGraphBuilder::new(app_loop.capabilities(), &preparers)
        .with_effects(app_loop.effects(), &effect_preparers)
        .with_returns(app_loop.bus_returns())
        .build(
            GraphRevision::INITIAL,
            app_loop.patches(),
            app_loop.current_parameters().clone(),
            SAMPLE_RATE,
            FRAME_COUNT,
        )
        .expect("complete production graph prepares");
    let structural = LockFreeStructuralGraphBoundary::new(
        1,
        1,
        GraphHandoffStatus::with_active(GraphRevision::INITIAL),
    )
    .expect("live structural boundary is valid");
    let (structural_control, structural_audio) = structural.into_handles();
    let worker = ThreadedGraphPreparationWorker::new_with_effects(
        registry.clone(),
        production_instrument_preparers().expect("worker preparers are valid"),
        effects,
        production_effect_preparers().expect("worker effect preparers are valid"),
        audio_config,
    )
    .expect("production threaded worker starts");
    app_loop
        .configure_engine_selection(
            DescriptorDefaultConfigFactory::new(registry, providers),
            worker,
            structural_control,
            &graph,
            audio_config,
        )
        .expect("live engine-selection runtime configures");
    let mut renderer = AudioRenderer::with_observation(audio, structural_audio, graph, writer);
    automatic
        .start()
        .expect("source starts after graph preparation");
    let mut output = vec![0.0_f32; FRAME_COUNT * 2];
    let runtime_audio = RuntimeAudioWitness::new(
        1,
        app_loop.patches().len(),
        app_loop.patches().len(),
        0,
        true,
        GraphRevision::INITIAL,
        0,
        0,
    );
    let mut runner = LiveDemoRunner::start(scene.clone(), automatic, reader, runtime_audio);

    let mut checkpoints = Vec::new();
    let first_checkpoint_step = scene
        .steps()
        .iter()
        .position(|step| step.requires_checkpoint())
        .expect("scene contains a scalar checkpoint");
    for _ in 0..=first_checkpoint_step {
        assert!(runner
            .advance(Duration::from_millis(50), &mut app_loop)
            .expect("first PATCH route dispatches")
            .is_none());
    }
    let records_after_dispatch = app_loop.event_log().total_observed();
    assert!(runner
        .advance(Duration::ZERO, &mut app_loop)
        .expect("stale audio remains pending")
        .is_none());
    assert_eq!(
        app_loop.event_log().total_observed(),
        records_after_dispatch
    );
    renderer.render(&mut output);
    runner
        .observe_shell_frame(shell_frame(&app_loop.current_graphical_shell()))
        .expect("the production-rendered first shell frame correlates");

    let mut deterministic_elapsed = Duration::from_millis(50 * (first_checkpoint_step as u64 + 1));
    let mut first_checkpoint_elapsed = None;
    for _ in 0..4_096 {
        app_loop
            .advance_structural()
            .expect("live structural control tick advances");
        let demo_records_before = app_loop
            .event_log()
            .records()
            .iter()
            .filter(|record| record.source() == EventSource::DemoScene)
            .count();
        let tick_elapsed = Duration::from_millis(50);
        deterministic_elapsed += tick_elapsed;
        let advance = runner.advance(tick_elapsed, &mut app_loop);
        if let Err(error) = &advance {
            panic!(
                "live runner advances coherently: {error:?}; shell={:?}",
                runner.shell_coverage()
            );
        }
        if let Some(checkpoint) = advance.unwrap() {
            assert!(checkpoint.agrees());
            first_checkpoint_elapsed.get_or_insert(deterministic_elapsed);
            checkpoints.push(checkpoint);
        }
        let demo_records = app_loop
            .event_log()
            .records()
            .iter()
            .filter(|record| record.source() == EventSource::DemoScene)
            .cloned()
            .collect::<Vec<_>>();
        let new_records = &demo_records[demo_records_before..];
        assert!(new_records.len() <= 8, "recovery batch remained bounded");
        if new_records.len() > 1 {
            assert!(new_records.iter().enumerate().all(|(index, record)| {
                matches!(record.input(), EventInput::Navigate { .. })
                    || (index == 0 && matches!(record.input(), EventInput::Adjust { .. }))
            }));
        }

        renderer.render(&mut output);
        if runner.completed_report().is_none() {
            runner
                .observe_shell_frame(shell_frame(&app_loop.current_graphical_shell()))
                .expect("the production-rendered shell frame correlates");
        }
        std::thread::yield_now();
        if runner.completed_report().is_some() {
            break;
        }
    }

    let report = runner
        .completed_report()
        .expect("bounded live scene completes");
    assert!(first_checkpoint_elapsed.unwrap() >= Duration::from_millis(500));
    let routing = report.mixer_routing();
    let mixer_scene = report
        .live_mixer()
        .expect("the dedicated scene retains its typed Mixer evidence");
    assert!(mixer_scene.is_complete());
    assert_eq!(mixer_scene.focus_pairs_exercised(), 16 * 4);
    assert_eq!(mixer_scene.indexed_send_edits_exercised(), 16 * 2);
    assert_eq!(
        mixer_scene.multi_select(),
        crest_synth::testing::LiveMixerMultiSelectResult::NotImplemented
    );
    assert!(
        report.complete(),
        "{}; routing={routing:?}",
        report.summary()
    );
    assert!(routing.is_complete());
    assert_eq!(routing.track_count(), 16);
    assert!(routing.track_ids_exact());
    assert!(routing.all_tracks_projected());
    assert!(routing.empty_tracks_addressable());
    assert!(routing.shared_track_patch_count() > 1);
    assert!(routing.shared_track_sum_exact());
    assert!(routing.track_level_controls_shared_sum());
    assert!(routing.patch_trim_isolated());
    assert!(routing.patch_reroute_isolated());
    assert!(routing.invalid_route_rejected());
    assert_eq!(routing.track_parameter_classes_exercised(), 6);
    assert!(routing.mute_wins());
    assert!(routing.any_solo_exact());
    assert!(routing.post_gate_sends_exact());
    assert!(routing.pre_gate_meters_exact());
    assert!(routing.fixed_snapshot_exact());
    assert!(routing.stable_focus_exact());
    assert_eq!(routing.callback_allocations(), 0);
    assert_eq!(routing.callback_destructions(), 0);
    assert!(report
        .checkpoints()
        .iter()
        .filter_map(|checkpoint| checkpoint.as_parameter())
        .any(|checkpoint| {
            matches!(
                checkpoint.expected_transition().editable_parameter(),
                Some(
                    crest_synth::testing::live_demo_scene::LiveEditableParameter::Track {
                        track_id,
                        parameter: MixerTrackParameter::Level,
                    }
                ) if *track_id == shared_track
            ) && checkpoint.audio_observation().active_notes() >= 2
                && checkpoint.audio_observation().track(shared_track).rms() > 0.0
        }));
    assert_eq!(report.checkpoints(), checkpoints);
    assert_eq!(report.coverage().expected().len(), expected_count);
    assert_eq!(report.coverage().exercised().len(), expected_count);
    assert!(report.coverage().missing().is_empty());
    assert!(report.coverage().unexpected().is_empty());
    assert!(report.coverage().duplicate_expected().is_empty());
    assert!(report.shell_coverage().is_complete());
    assert!(report.shell_coverage().patch_context_observed());
    assert!(report.shell_coverage().mixer_context_observed());
    assert!(report.shell_coverage().all_semantic_surfaces_observed());
    assert!(report.shell_coverage().both_reachable_modes_observed());
    assert!(report.shell_coverage().both_return_round_trips_observed());
    assert!(report.shell_coverage().healthy_empty_errors_observed());
    assert!(report.shell_coverage().focus_recovery_observed());
    assert_eq!(
        report.graphical_shell().state_hash(),
        report.state_tree().state_hash()
    );
    let patch_adsr_checkpoints = report
        .checkpoints()
        .iter()
        .filter_map(|checkpoint| checkpoint.as_parameter())
        .filter(|checkpoint| {
            matches!(
                checkpoint.expected_transition().patch_control_id(),
                Some(PatchControlId::Envelope(_))
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(patch_adsr_checkpoints.len(), 4);
    for (checkpoint, descriptor) in patch_adsr_checkpoints
        .iter()
        .zip(VoiceEnvelope::surface_descriptor())
    {
        let control = PatchControlId::Envelope(descriptor.parameter());
        assert_eq!(
            checkpoint.expected_transition().patch_control_id(),
            Some(control.clone())
        );
        assert_eq!(
            checkpoint.projected_value().patch_control_id(),
            Some(control)
        );
        assert_eq!(
            checkpoint.projected_value().state_value(),
            checkpoint.projected_value().parameter_value()
        );
        assert_eq!(
            checkpoint.audio_observation().parameter_generation(),
            checkpoint.generation()
        );
        assert_eq!(checkpoint.emitted_effects().len(), 2);
        assert!(matches!(
            checkpoint.emitted_effects()[0],
            EmittedEvent::StateAccepted { .. }
        ));
        assert!(matches!(
            checkpoint.emitted_effects()[1],
            EmittedEvent::ParameterSnapshotPublished { .. }
        ));
    }
    let effect_checkpoints = report
        .checkpoints()
        .iter()
        .filter_map(|checkpoint| checkpoint.as_parameter())
        .filter(|checkpoint| {
            matches!(
                checkpoint.expected_transition().patch_control_id(),
                Some(PatchControlId::Effect(_, _))
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(effect_checkpoints.len(), 2);
    assert!(effect_checkpoints.iter().all(|checkpoint| {
        let observation = checkpoint.audio_observation();
        let effect = observation.patch_effect();
        effect.patch_id() == Some(focused_patch_id)
            && effect.input_rms() > 0.0
            && effect.output_rms() > 0.0
            && effect.difference_rms() > 0.0
            && effect.side_rms() > 0.0
    }));
    let engine_checkpoints = report
        .checkpoints()
        .iter()
        .filter_map(|checkpoint| checkpoint.as_engine())
        .collect::<Vec<_>>();
    assert_eq!(engine_checkpoints.len(), 9);
    let preset_checkpoints = &engine_checkpoints[..3];
    assert_eq!(
        preset_checkpoints
            .iter()
            .map(|checkpoint| checkpoint.status())
            .collect::<Vec<_>>(),
        [
            crest_synth::control::EngineSelectionStatusKind::Loading,
            crest_synth::control::EngineSelectionStatusKind::Activating,
            crest_synth::control::EngineSelectionStatusKind::Ready,
        ]
    );
    assert!(preset_checkpoints.iter().all(|checkpoint| matches!(
        checkpoint.intent(),
        StructuralEditIntent::ReplaceParameterChoice { .. }
    )));
    assert!(preset_checkpoints
        .iter()
        .all(|checkpoint| checkpoint.preset().is_some()));
    assert!(preset_checkpoints[0].source_audio_nonzero());
    assert!(preset_checkpoints[2].target_audio_nonzero());
    assert!(engine_checkpoints[3..]
        .iter()
        .all(|checkpoint| checkpoint.focused_control_id() == PatchControlId::Engine));
    assert_eq!(
        report.coverage().expected_engine_transitions(),
        [
            "SoundFontPresetToNext",
            "SoundFontToBraids",
            "BraidsToDescriptorDefaultSoundFont",
        ]
    );
    assert_eq!(
        report.coverage().exercised_engine_transitions(),
        [
            "BraidsToDescriptorDefaultSoundFont",
            "SoundFontPresetToNext",
            "SoundFontToBraids",
        ]
    );
    assert!(report
        .event_log()
        .records()
        .iter()
        .any(|record| record.outcome() == EventOutcome::Accepted));
    assert!(report
        .event_log()
        .records()
        .iter()
        .any(|record| record.outcome() == EventOutcome::Rejected));
    assert_eq!(report.event_log().dropped_records(), 0);
    assert_eq!(
        report.state_tree().generation(),
        report
            .event_log()
            .records()
            .back()
            .unwrap()
            .generation_after()
    );
    let report_json = report.to_json().unwrap();
    assert!(report_json.contains("\"complete\":true"));
    assert!(report_json.contains("\"mixerRouting\":{"));
    assert!(report_json.contains("\"shared_track_sum_exact\":true"));
    assert_eq!(report.runtime_audio().prepared_shared_assets(), 1);
    assert_eq!(report.runtime_audio().prepared_instruments(), 2);
    assert_eq!(
        report.runtime_audio().engine_managed_patches(),
        report.runtime_audio().prepared_instruments()
    );
    assert_eq!(report.runtime_audio().fixed_per_patch_patches(), 0);
    assert!(report.runtime_audio().adjacent_capabilities_distinct());
    assert_eq!(
        report.runtime_audio().active_graph_revision(),
        GraphRevision::new(4).unwrap()
    );
    assert_eq!(report.runtime_audio().engine_switches(), 3);
    assert_eq!(report.runtime_audio().callback_destructions(), 0);
    let event_log_summary = serde_json::to_value(report.event_log_summary()).unwrap();
    assert_eq!(event_log_summary["lossless"], true);
    assert_eq!(
        event_log_summary["totalObserved"],
        report.event_log().total_observed()
    );
    assert_eq!(
        event_log_summary["generationAfter"],
        report.state_tree().generation()
    );
    assert_eq!(event_log_summary["activeGraphRevision"], 4);
    let final_tree_value: serde_json::Value =
        serde_json::from_str(report.state_tree().json()).unwrap();
    assert_eq!(
        final_tree_value["interaction"]["activeFocus"]["controlId"]["id"],
        serde_json::json!({"kind": "track", "trackId": 15, "parameter": "solo"})
    );

    let final_log = app_loop.event_log().to_json().unwrap();
    let final_tree = app_loop.current_state_tree().into_json();
    let final_projection = app_loop.current_text();
    for _ in 0..3 {
        assert!(runner
            .advance(Duration::from_secs(1), &mut app_loop)
            .unwrap()
            .is_none());
    }
    assert_eq!(app_loop.event_log().to_json().unwrap(), final_log);
    assert_eq!(app_loop.current_state_tree().json(), final_tree);
    assert_eq!(app_loop.current_text(), final_projection);
    assert!(matches!(
        runner.observe_shell_frame(shell_frame(&app_loop.current_graphical_shell())),
        Err(LiveDemoError::UnexpectedShellFrame)
    ));

    drop(renderer);
    app_loop
        .shutdown_engine_selection_on_control()
        .expect("live worker shuts down on control");

    println!("CREST_ACCEPTANCE live_demo_scene passed");
}

#[test]
fn live_demo_early_close_uses_semantic_cleanup_without_success_report() {
    let global = globals();
    let initial = ParameterSnapshot::new(0, global, MixerState::default(), &[]).unwrap();
    let boundary = LockFreeAudioBoundary::new(64, initial);
    let (control, audio) = boundary.into_handles();
    let registry = production_capability_registry().unwrap();
    let effects = production_effect_registry().unwrap();
    let mut app_loop = AppLoop::with_event_log(
        AppState::new_with_effects(registry, effects.clone(), global).with_initial_returns(
            crest_synth::adapter::production_effects::startup_bus_returns(&effects),
        ),
        StateProjector::for_graph(GraphRevision::INITIAL),
        control,
        EventLog::new(256).unwrap(),
    )
    .unwrap();
    let providers = production_instrument_providers().unwrap();
    let effect_providers = production_effect_providers().unwrap();
    let mut automatic = AutomaticMidiTest::new(FixtureMidiSource::new());
    automatic
        .initialize_with_effects(&providers, &effect_providers, &mut app_loop)
        .unwrap();
    let preparers: Vec<Box<dyn InstrumentPreparer>> = vec![
        Box::new(FixturePreparer::for_capability(
            "instrument.soundfont.hidef",
        )),
        Box::new(FixturePreparer::for_capability("instrument.braids")),
    ];
    let effect_preparers = production_effect_preparers().unwrap();
    let graph = PreparedGraphBuilder::new(app_loop.capabilities(), &preparers)
        .with_effects(app_loop.effects(), &effect_preparers)
        .with_returns(app_loop.bus_returns())
        .build(
            GraphRevision::INITIAL,
            app_loop.patches(),
            app_loop.current_parameters().clone(),
            SAMPLE_RATE,
            FRAME_COUNT,
        )
        .unwrap();
    let _renderer = AudioRenderer::new(audio, NoStructuralGraphChanges::new(), graph);
    automatic.start().unwrap();
    let scene = LiveDemoScene::from_installed_state(&app_loop.current_state_tree()).unwrap();
    let cleanup_count = scene.patch_ids().len();
    let observation = AtomicAudioObservation::default();
    let (_writer, reader) = observation.into_handles();
    let runtime_audio = RuntimeAudioWitness::new(
        1,
        app_loop.patches().len(),
        app_loop.patches().len(),
        0,
        false,
        GraphRevision::INITIAL,
        0,
        0,
    );
    let mut runner = LiveDemoRunner::start(scene, automatic, reader, runtime_audio);

    runner.cleanup_before_close(&mut app_loop).unwrap();

    assert!(runner.is_aborted());
    assert!(runner.completed_report().is_none());
    assert_eq!(
        app_loop
            .event_log()
            .records()
            .iter()
            .filter(|record| record.source() == EventSource::DemoScene)
            .count(),
        cleanup_count
    );
    assert!(runner
        .advance(Duration::from_secs(1), &mut app_loop)
        .unwrap()
        .is_none());
}
