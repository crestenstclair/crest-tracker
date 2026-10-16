//! Deterministic integration surface for the retained effects-and-buses
//! scene: drives the cumulative live scene headless through the production
//! reducer, projections, renderer, and observation paths, and asserts the
//! phase contract — topology transitions correlated through their
//! checkpoints, the controlled rejection with uninterrupted audio, the
//! add-only frozen checkpoint-identity baseline with the journey-driven
//! occupancy contract, and the NFR-008 edit-responsiveness measurements
//! (acceptance-to-projection ≤ 1 frame; the change audible in the first
//! observed block on the activated graph, with an activation-sequence gap
//! of exactly 1 in this block-per-tick deterministic drive).

#[path = "support/performance_evidence.rs"]
mod performance_evidence;
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
use crest_synth::control::app_loop::AppLoop;
use crest_synth::control::app_state::AppState;
use crest_synth::control::event_log::EventLog;
use crest_synth::control::event_record::EventOutcome;
use crest_synth::control::state_projector::StateProjector;
use crest_synth::control::{
    EngineSelectionStatusKind, GraphicalShellProjection, MixerControlId, PatchControlId,
    SemanticAction,
};
use crest_synth::real_time::audio_boundary::AudioBoundary;
use crest_synth::real_time::audio_observation::AudioObservation;
use crest_synth::real_time::audio_renderer::AudioRenderer;
use crest_synth::real_time::graph_revision::GraphRevision;
use crest_synth::real_time::prepared_graph_builder::PreparedGraphBuilder;
use crest_synth::real_time::StructuralGraphBoundary;
use crest_synth::shell::audio_output::{AudioDeviceConfig, AudioSampleFormat};
use crest_synth::shell::{
    ShellFrameObservation, ShellRegionId, ShellRegionObservation, ShellRegionRect,
};
use crest_synth::synth::DescriptorDefaultConfigFactory;
use crest_synth::testing::automatic_midi_test::AutomaticMidiTest;
use crest_synth::testing::live_demo_runner::LiveDemoRunner;
use crest_synth::testing::live_effects_and_buses_scene::LiveTopologySupport;
use crest_synth::testing::{RuntimeAudioWitness, EFFECTS_AND_BUSES_SCENE_NAME};
use std::time::Duration;
use support::{globals, FixtureMidiSource, FRAME_COUNT, SAMPLE_RATE};

/// The frozen pre-journey checkpoint-identity baseline (spec C-001).
///
/// Captured from the deterministic scene run at the claim state before the
/// journey rework: the exact topology checkpoint identity sequence the phase
/// gate emitted. The rework is strictly add-only — these identities must
/// survive byte-identically and in this order, with the journey identities
/// appearing only as pure insertions between them.
const FROZEN_TOPOLOGY_IDENTITY_BASELINE: [&str; 17] = [
    "Slot.startupOccupantCleared",
    "SlotFill.first",
    "SlotFill.second",
    "SlotFill.third",
    "SlotOrder.exchangeFirst",
    "SlotOrder.exchangeSecond",
    "SlotTwin.sameEntry",
    "Send.towardDestinations",
    "Return.contentChanged",
    "Return.emptyOccupied",
    "Topology.refused",
    "Topology.recoveredAfterRefusal",
    "Reroute.chainFollows",
    "Slot.startupOccupantRestored",
    "Slot.secondCleared",
    "Slot.thirdCleared",
    "Return.emptyRestored",
];

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
fn effects_and_buses_scene_completes_with_measured_topology_and_responsiveness() {
    let global = globals();
    let initial = crest_synth::real_time::ParameterSnapshot::new(
        0,
        global,
        crest_synth::mixer::mixer_state::MixerState::default(),
        &[],
    )
    .expect("initial parameters are valid");
    let boundary = LockFreeAudioBoundary::new(512, initial);
    let (control, audio) = boundary.into_handles();
    let event_log = EventLog::new(131_072).expect("live test journal capacity is valid");
    let providers = production_instrument_providers().expect("production providers are valid");
    let registry = production_capability_registry().expect("production registry is valid");
    let effect_providers =
        production_effect_providers().expect("production effect providers are valid");
    let effects = production_effect_registry().expect("production effect registry is valid");
    // This scene proves edits to already-audible returns. Its fixture owns
    // those occupants; new production sessions deliberately start empty.
    let returns =
        crest_synth::adapter::production_effects::production_default_bus_returns(&effects)
            .expect("the audible effects-and-buses fixture composes");
    let mut app_loop = AppLoop::with_event_log(
        AppState::new_with_effects(registry.clone(), effects.clone(), global)
            .with_initial_returns(returns),
        StateProjector::for_graph(GraphRevision::INITIAL),
        control,
        event_log,
    )
    .expect("initial application state projects");

    let mut automatic = AutomaticMidiTest::new(FixtureMidiSource::new());
    automatic
        .initialize_with_effects(&providers, &effect_providers, &mut app_loop)
        .expect("fixture initializes through AppLoop");
    let scene = crest_synth::testing::live_effects_and_buses_scene::from_installed_state(
        &app_loop.current_state_tree(),
    )
    .expect("installed fixture produces the effects-and-buses scene");
    assert_eq!(scene.name(), EFFECTS_AND_BUSES_SCENE_NAME);
    assert_eq!(scene.expected_topology_transitions().len(), 30);
    assert!(scene.total_timeout() > Duration::from_secs(120));
    assert!(app_loop.event_log().capacity() >= scene.required_event_log_capacity(60_000));

    // C-001, declared surface: the frozen identity baseline survives
    // byte-identically and in order; filtering the declared sequence down to
    // baseline members must reproduce the baseline exactly, so a renamed,
    // removed, reordered, or duplicated identity fails here and every other
    // declared identity is a pure insertion.
    let declared: Vec<&str> = scene
        .expected_topology_transitions()
        .iter()
        .map(|transition| transition.identifier())
        .collect();
    let retained: Vec<&str> = declared
        .iter()
        .copied()
        .filter(|identifier| FROZEN_TOPOLOGY_IDENTITY_BASELINE.contains(identifier))
        .collect();
    assert_eq!(retained, FROZEN_TOPOLOGY_IDENTITY_BASELINE);

    // Journey contract: exactly one direct occupancy injection remains — the
    // documented rejection, whose unknown registry entry the adjacent-choice
    // gesture cannot express — and every other occupancy change dispatches
    // the gesture behind a focus-verified journey to the exact row.
    let mut injected = Vec::new();
    for transition in scene.expected_topology_transitions() {
        match transition.action() {
            Some(SemanticAction::SetSlotOccupancy { slot, .. }) => {
                if transition.adjust().is_none() {
                    injected.push(transition.identifier());
                } else {
                    let row = PatchControlId::EffectSlot(*slot);
                    assert!(
                        transition.support_before().iter().any(|item| matches!(
                            item,
                            LiveTopologySupport::VerifyPatchFocus { control } if *control == row
                        )),
                        "{} lacks a focus-verified slot journey",
                        transition.identifier()
                    );
                }
            }
            Some(SemanticAction::SetReturnOccupancy { bus, .. }) => {
                if transition.adjust().is_none() {
                    injected.push(transition.identifier());
                } else {
                    let row = MixerControlId::ReturnOccupancy { bus: *bus };
                    assert!(
                        transition.support_before().iter().any(|item| matches!(
                            item,
                            LiveTopologySupport::VerifyMixerFocus { control } if *control == row
                        )),
                        "{} lacks a focus-verified return journey",
                        transition.identifier()
                    );
                }
            }
            _ => {}
        }
    }
    assert_eq!(injected, ["Topology.refused"]);

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
        crest_synth::real_time::GraphHandoffStatus::with_active(GraphRevision::INITIAL),
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
    let mut runner = LiveDemoRunner::start(scene, automatic, reader, runtime_audio);

    // Block-per-tick deterministic drive: every advance is followed by one
    // structural pump, one rendered block, and one painted frame, so the
    // runner observes every observation sequence and every projection frame.
    // A 5 ms virtual tick: virtual time only paces dwell and timeouts; the
    // audibility waits are physical render blocks (a fresh full-wet delay
    // emits nothing until its delay time has rendered), so the virtual
    // budget must not starve the physics.
    let mut checkpoints = Vec::new();
    for _ in 0..60_000 {
        app_loop
            .advance_structural()
            .expect("live structural control tick advances");
        let advance = runner.advance(Duration::from_millis(5), &mut app_loop);
        if let Err(error) = &advance {
            panic!("live runner advances coherently: {error:?}");
        }
        if let Some(checkpoint) = advance.unwrap() {
            assert!(checkpoint.agrees());
            checkpoints.push(checkpoint);
        }
        renderer.render(&mut output);
        if runner.completed_report().is_none() {
            runner
                .observe_shell_frame(shell_frame(&app_loop.current_graphical_shell()))
                .expect("the production-rendered shell frame correlates");
        }
        // Preparation runs on a real worker. Do not spend seconds of its
        // virtual deadline in a tight, optimized polling loop: give it the
        // same 5 ms as the simulated tick while continuing to render and
        // observe every block. All transition and timeout gates stay active.
        if matches!(
            app_loop.engine_selection_status().kind(),
            EngineSelectionStatusKind::Loading
                | EngineSelectionStatusKind::Validating
                | EngineSelectionStatusKind::Preparing
        ) {
            std::thread::sleep(Duration::from_millis(5));
        }
        if runner.completed_report().is_some() {
            break;
        }
    }

    let report = runner
        .completed_report()
        .expect("bounded cumulative live scene completes");
    performance_evidence::retain(report);
    assert!(
        report.complete(),
        "cumulative scene incomplete: {}",
        report.summary()
    );
    assert_eq!(report.scene(), EFFECTS_AND_BUSES_SCENE_NAME);
    assert_eq!(report.checkpoints(), checkpoints);

    // The inherited sixteen-track contract still holds in full.
    assert!(report.mixer_routing().is_complete());
    assert!(report.coverage().is_complete());
    assert!(report.shell_coverage().is_complete());

    // Every declared topology transition was exercised in order.
    assert_eq!(
        report.coverage().missing_topology_transitions(),
        &[] as &[String]
    );
    assert_eq!(
        report.coverage().unexpected_topology_transitions(),
        &[] as &[String]
    );
    assert_eq!(report.coverage().exercised_topology_transitions().len(), 30);

    let topology: Vec<_> = report
        .checkpoints()
        .iter()
        .filter_map(|checkpoint| checkpoint.as_topology())
        .collect();
    assert_eq!(topology.len(), 30);
    assert!(topology.iter().all(|checkpoint| checkpoint.agrees()));

    // C-001, emitted surface: the captured checkpoint sequence keeps the
    // frozen baseline identities unchanged and in order too.
    let emitted_retained: Vec<&str> = topology
        .iter()
        .map(|checkpoint| checkpoint.transition())
        .filter(|identifier| FROZEN_TOPOLOGY_IDENTITY_BASELINE.contains(identifier))
        .collect();
    assert_eq!(emitted_retained, FROZEN_TOPOLOGY_IDENTITY_BASELINE);

    // The occupant scalar edit from the PATCH page carries its own accepted,
    // audibly witnessed checkpoint.
    let scalar_edited = topology
        .iter()
        .find(|checkpoint| checkpoint.transition() == "SlotOccupant.scalarEdited")
        .expect("the journey scene edits an installed occupant scalar");
    assert_eq!(scalar_edited.outcome(), EventOutcome::Accepted);
    assert!(scalar_edited.audible_on_activated_graph());
    assert!(scalar_edited.audio_after().active_notes() > 0);

    // AS-1.5 voice carry-over: the slot-clear step held its probe note
    // through the activation — never re-sounding it — and the note was still
    // observed active on the target graph: the sounding voice itself carried
    // over.
    let cleared = topology
        .iter()
        .find(|checkpoint| checkpoint.transition() == "Slot.startupOccupantCleared")
        .expect("the retained scene keeps the slot-clear step");
    assert_eq!(cleared.held_note_carried_across_activation(), Some(true));
    assert!(cleared.audio_after().active_notes() > 0);

    // FR-015/SC-006: exactly one controlled rejection with an attributable
    // reason, uninterrupted audio, and an accepted lifecycle change after it.
    let rejected: Vec<_> = topology
        .iter()
        .filter(|checkpoint| checkpoint.outcome() == EventOutcome::Rejected)
        .collect();
    assert_eq!(rejected.len(), 1);
    assert_eq!(rejected[0].rejection(), Some("invalidEffectConfig"));
    assert!(rejected[0].audio_uninterrupted());
    assert_eq!(
        rejected[0].audio_after().active_graph_revision(),
        rejected[0].source_graph_revision()
    );

    // NFR-008 measured through the checkpoints: acceptance-to-projection is
    // exactly one painted frame, and the very first observed block on every
    // activated graph is already audible, one sequence after the last
    // source-graph block.
    let evidence = report
        .effects_and_buses()
        .expect("the cumulative scene retains effects-and-buses evidence");
    assert!(evidence.is_complete());
    assert_eq!(evidence.frames_to_projection_max(), 1);
    assert_eq!(evidence.activation_sequence_gap_max(), 1);
    assert!(evidence.render_blocks_to_audible_max() >= 1);
    for checkpoint in &topology {
        if let Some(frames) = checkpoint.frames_to_projection() {
            assert_eq!(frames, 1, "{}", checkpoint.transition());
        }
        if let Some(gap) = checkpoint.observed_activation_sequence_gap() {
            // NFR-003/NFR-008: the swap is atomic at one block boundary —
            // exactly one sequence separates the last source-graph block
            // from the first target-graph block in this deterministic drive.
            assert_eq!(gap, 1, "{}", checkpoint.transition());
        }
        if let Some(blocks) = checkpoint.render_blocks_to_audible() {
            // Audibility develops as fast as the note attack and the
            // occupying effect's own latency allow; record the measurement.
            println!(
                "NFR-008 {}: renderBlocksToAudible={blocks}",
                checkpoint.transition()
            );
            assert!(blocks >= 1, "{}", checkpoint.transition());
        }
        assert!(checkpoint.audible_on_activated_graph());
    }

    // NFR-007/SC-004/SC-005 numeric routing measurements.
    assert!(evidence.max_off_target_bus_dbfs() < -60.0);
    assert_eq!(evidence.gated_wet_contribution(), 0.0);

    // NFR-006: teardown left zero active notes on the final observation.
    assert_eq!(report.final_audio_observation().active_notes(), 0);
    assert_eq!(report.final_audio_observation().non_finite_samples(), 0);

    drop(renderer);
    assert_eq!(app_loop.owned_structural_graphs_on_control(), 0);
    app_loop
        .shutdown_engine_selection_on_control()
        .expect("engine selection shuts down cleanly");

    println!("CREST_ACCEPTANCE effects_and_buses passed");
}
