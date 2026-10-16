//! Acceptance for the semantic graphical view model.
//!
//! Retargeted by mission webview-shell-cutover-01KZAC7Q WP05 (T019): only the
//! render half changed. The action/focus/recovery/projection assertions are
//! renderer-neutral and untouched; "render through production native frames at
//! both viewports" became "render the same immutable model through the
//! webview projection path at both viewports" — the production
//! [`ProjectionChannel`] emit (the exact transport `TauriWebviewWindow`'s
//! tick performs), the page's painted-ack role echoing the pushed document's
//! own identity with the authored band geometry, and WP02's
//! `forward_ack` seam constructing the one `ShellFrameObservation`. Identity
//! cannot be supplied from outside the pushed document: `forward_ack`
//! rejects any ack that is not a verbatim copy of the in-flight document's
//! serialized identity, so a separately supplied projection is structurally
//! impossible in this harness.

use crest_synth::adapter::braids_capability::{BraidsCapability, BRAIDS_CAPABILITY_ID};
use crest_synth::adapter::production_effects::{
    production_chorus_config, production_effect_registry,
};
use crest_synth::adapter::production_instruments::{
    production_capability_registry, production_soundfont_capability,
};
use crest_synth::control::event_record::{EmittedEvent, EventInput, EventSource};
use crest_synth::control::{
    AppEvent, AppLoop, AppState, Direction, EngineSelectionFailure, FocusCapabilityId,
    InteractionMode, MixerControlId, PatchControlId, SemanticAction, SemanticControlId,
    SemanticResolver, SemanticSurfaceSummary, StateProjector, SurfaceId, TopLevelContext,
};
use crest_synth::kernel::{MidiChannel, PatchId};
use crest_synth::mixer::global_parameters::GlobalParameters;
use crest_synth::mixer::mixer_track_id::MixerTrackId;
use crest_synth::mixer::mixer_track_parameters::MixerTrackParameter;
use crest_synth::mixer::patch_output::PatchOutput;
use crest_synth::real_time::audio_boundary::{BoundaryFull, ControlAudioBoundary};
use crest_synth::real_time::{AudioCommand, GraphRevision, ParameterSnapshot};
use crest_synth::shell::density::ResponsiveShellContract;
use crest_synth::shell::webview::projection_channel::{
    ForwardedAck, ProjectionChannel, ProjectionPush,
};
use crest_synth::shell::{ShellFrameObservation, ShellRegionId};
use crest_synth::synth::effect_slot_id::EffectSlotIndex;
use crest_synth::synth::sound_font_instrument::SoundFontInstrument;
use crest_synth::synth::{EffectSlotId, InstrumentConfig, Patch};
use crest_synth::testing::automatic_midi_test::create_soundfont_config;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct BoundaryEvidence {
    parameters: Vec<ParameterSnapshot>,
    commands: Vec<AudioCommand>,
}

struct ProbeBoundary {
    evidence: Arc<Mutex<BoundaryEvidence>>,
}

impl ControlAudioBoundary for ProbeBoundary {
    fn push_command(&mut self, command: AudioCommand) -> Result<(), BoundaryFull> {
        self.evidence.lock().unwrap().commands.push(command);
        Ok(())
    }

    fn publish_parameters(&mut self, parameters: ParameterSnapshot) {
        self.evidence.lock().unwrap().parameters.push(parameters);
    }
}

fn globals() -> GlobalParameters {
    GlobalParameters::new(-3.0).unwrap()
}

fn soundfont_config() -> InstrumentConfig {
    create_soundfont_config(
        &production_soundfont_capability().unwrap(),
        SoundFontInstrument::new(0, 40, false).unwrap(),
    )
    .unwrap()
}

fn installed_state(soundfont_first: bool) -> AppState {
    let soundfont = soundfont_config();
    let braids = BraidsCapability::new().unwrap().default_config().unwrap();
    let configs = if soundfont_first {
        [soundfont, braids]
    } else {
        [braids, soundfont]
    };
    let patches = configs
        .into_iter()
        .enumerate()
        .map(|(index, config)| {
            let patch = Patch::new(
                PatchId::new(index as u32 + 1).unwrap(),
                format!("Semantic {}", index + 1),
                config,
                MidiChannel::new(index as u8).unwrap(),
                PatchOutput::new(MixerTrackId::new(index as u8).unwrap(), -5.0 - index as f32)
                    .unwrap(),
            );
            if index == 0 && soundfont_first {
                patch.with_effect_slot(
                    EffectSlotIndex::ALL[0],
                    production_chorus_config(EffectSlotId::new(1).unwrap()).unwrap(),
                )
            } else {
                patch
            }
        })
        .collect();
    let mut state = AppState::new_with_effects(
        production_capability_registry().unwrap(),
        production_effect_registry().unwrap(),
        globals(),
    );
    state.apply(AppEvent::InstallPatches(patches)).unwrap();
    state
}

fn semantic(state: &AppState) -> crest_synth::control::SemanticGraphicalViewModel {
    StateProjector::new()
        .project_with_shell(state)
        .unwrap()
        .3
        .semantic_model()
        .clone()
}

/// The page's per-band first visible text, derived from the pushed document
/// exactly as the committed page derives it (`webview-page/page.js`): the
/// static product word on the context line, the context display on the
/// identity header, the caption-row left run on the workspace, the side
/// panel's opening label, and the footer breadcrumb prefix.
fn page_band_labels(document: &Value) -> [String; 5] {
    let context = document
        .get("context")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let surface_label = |id: &str| -> String {
        document
            .get("surfaces")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|surface| surface.get("id").and_then(Value::as_str) == Some(id))
            .and_then(|surface| surface.get("label").and_then(Value::as_str))
            .unwrap_or_default()
            .to_owned()
    };
    let workspace = if context == "mixer" {
        "LEVEL / PAN / MUTE / SOLO".to_owned()
    } else {
        surface_label("patchMain")
    };
    let side = if context == "mixer" {
        "CURSOR".to_owned()
    } else {
        surface_label("patchUtility")
    };
    [
        "CREST SYNTH".to_owned(),
        context.to_uppercase(),
        workspace,
        side,
        context.to_uppercase(),
    ]
}

/// The page's paint-acknowledgment role, played headless: the six identity
/// fields copied verbatim from the pushed document (the page contract —
/// `paintedEvidence` echoes the received document, never re-derives), and
/// the measured band geometry standing in for the page's CSS-seated bands
/// through the authored density policy for the viewport. The live DOM
/// measurement of the same bands is `tests/webview_projection_shell.rs`
/// (T024/T026); what this harness proves is the seam: only an ack whose
/// identity matches a pushed document, whose geometry tiles the viewport,
/// and whose labels are visible can ever become an observation.
fn page_painted_ack(document: &Value, viewport: [f32; 2]) -> Value {
    let geometry = ResponsiveShellContract::get().witness_geometry(viewport[0], viewport[1]);
    let context_bottom = geometry.context_line_px;
    let identity_bottom = geometry.workspace_y_px;
    let workspace_bottom = viewport[1] - geometry.footer_px;
    let labels = page_band_labels(document);
    json!({
        "generation": document["generation"],
        "stateHash": document["stateHash"],
        "context": document["context"],
        "activeSurface": document["activeSurface"],
        "focusPath": document["focusPath"],
        "interactionMode": document["interactionMode"],
        "viewport": { "widthPx": viewport[0], "heightPx": viewport[1] },
        "regions": [
            { "id": "contextLine", "xPx": 0.0, "yPx": 0.0,
              "widthPx": viewport[0], "heightPx": context_bottom,
              "label": labels[0] },
            { "id": "identityHeader", "xPx": 0.0, "yPx": context_bottom,
              "widthPx": viewport[0], "heightPx": geometry.identity_header_px,
              "label": labels[1] },
            { "id": "mainWorkspace", "xPx": 0.0, "yPx": identity_bottom,
              "widthPx": geometry.main_width_px, "heightPx": geometry.main_height_px,
              "label": labels[2] },
            { "id": "persistentSideRegion", "xPx": geometry.side_x_px, "yPx": geometry.side_y_px,
              "widthPx": geometry.side_width_px, "heightPx": geometry.side_height_px,
              "label": labels[3] },
            { "id": "footer", "xPx": 0.0, "yPx": workspace_bottom,
              "widthPx": viewport[0], "heightPx": geometry.footer_px,
              "label": labels[4] },
        ],
    })
}

/// Renders one immutable projection through the webview projection path at
/// one viewport: the production [`ProjectionChannel`] serializes and emits
/// the projection's embedded semantic model (the exact transport the
/// `TauriWebviewWindow` tick performs), the page's ack role echoes the
/// pushed document, and WP02's `forward_ack` seam constructs the one
/// [`ShellFrameObservation`]. Passive by construction: no input path exists
/// anywhere in this render.
fn render(
    projection: crest_synth::control::GraphicalShellProjection,
    viewport: [f32; 2],
) -> ShellFrameObservation {
    let mut channel = ProjectionChannel::new();
    let mut emitted = None;
    let outcome = channel
        .push(&projection, |document| {
            emitted = Some(document);
            Ok(())
        })
        .expect("the passive webview render emit succeeds");
    assert_eq!(outcome, ProjectionPush::Emitted);
    let document = emitted.expect("an Emitted push hands the emitter exactly one document");
    match channel
        .forward_ack(&page_painted_ack(&document, viewport).to_string())
        .expect("the painted ack for the pushed document becomes exactly one observation")
    {
        ForwardedAck::Observation(observation) => observation,
        ForwardedAck::SupersededLate { generation } => {
            panic!("the ack for the just-pushed document cannot be late (generation {generation})")
        }
    }
}

#[test]
fn production_semantic_graphical_view_model_is_exact_passive_and_audio_neutral() {
    let mut state = installed_state(true);
    let initial = semantic(&state);
    assert_eq!(initial.context(), TopLevelContext::Mixer);
    assert_eq!(initial.active_surface(), SurfaceId::MixerMain);
    assert_eq!(initial.interaction_mode(), InteractionMode::Navigate);
    assert_eq!(initial.errors(), []);
    assert_eq!(initial.surfaces().len(), 2);
    assert!(initial.surface(SurfaceId::MixerMain).is_some());
    assert!(initial.surface(SurfaceId::MixerInspector).is_some());
    let inspector = initial.surface(SurfaceId::MixerInspector).unwrap();
    // Eight indexed sends for the selected track, each empty return's
    // occupancy and level rows, then master gain alone.
    let bus_count = crest_synth::mixer::bus_id::BusId::COUNT;
    assert_eq!(
        inspector.controls().len(),
        bus_count + bus_count * 2 + GlobalParameters::surface_descriptor().len()
    );
    for (index, bus) in crest_synth::mixer::bus_id::BusId::ALL
        .into_iter()
        .enumerate()
    {
        assert!(matches!(
            inspector.controls()[index].path().control_id(),
            SemanticControlId::Mixer(MixerControlId::Send {
                track_id,
                bus: control_bus,
            }) if *track_id == MixerTrackId::new(0).unwrap() && *control_bus == bus
        ));
    }
    for (pair, bus) in crest_synth::mixer::bus_id::BusId::ALL
        .into_iter()
        .enumerate()
    {
        let occupancy = &inspector.controls()[bus_count + pair * 2];
        let level = &inspector.controls()[bus_count + pair * 2 + 1];
        assert!(matches!(
            occupancy.path().control_id(),
            SemanticControlId::Mixer(MixerControlId::ReturnOccupancy { bus: control_bus })
                if *control_bus == bus
        ));
        assert!(matches!(
            level.path().control_id(),
            SemanticControlId::Mixer(MixerControlId::ReturnLevel { bus: control_bus })
                if *control_bus == bus
        ));
    }
    assert!(inspector.controls()[bus_count * 3..]
        .iter()
        .all(|control| matches!(
            control.path().control_id(),
            SemanticControlId::Mixer(MixerControlId::Global { .. })
        )));
    match inspector.summary() {
        SemanticSurfaceSummary::MixerInspector {
            focused_track,
            patch_count,
            routed_patches,
            ..
        } => {
            assert_eq!(*focused_track, MixerTrackId::new(0).unwrap());
            assert_eq!(*patch_count, 2);
            assert_eq!(routed_patches.len(), 1);
            assert_eq!(routed_patches[0].patch_id(), PatchId::new(1).unwrap());
            assert_eq!(routed_patches[0].patch_name(), "Semantic 1");
        }
        other => panic!("expected canonical Mixer Inspector summary, got {other:?}"),
    }

    let resolved_actions = SemanticResolver::new(&state).valid_actions();
    assert_eq!(initial.valid_actions(), resolved_actions);
    for valid in initial.valid_actions() {
        let mut candidate = state.clone();
        candidate
            .apply_semantic_action(valid.action().clone())
            .expect("every projected valid action is reducer-accepted");
    }

    state
        .apply_semantic_action(SemanticAction::SelectContext(TopLevelContext::Patch))
        .unwrap();
    let patch = semantic(&state);
    assert_eq!(patch.active_surface(), SurfaceId::PatchMain);
    assert_eq!(patch.surfaces().len(), 2);
    assert!(patch.surface(SurfaceId::PatchUtility).is_some());
    let main = patch.surface(SurfaceId::PatchMain).unwrap();
    assert_eq!(main.controls().len(), 4);
    assert!(main
        .controls()
        .iter()
        .all(|control| control.path().capability_id().is_none()));
    assert_eq!(
        main.sections()
            .iter()
            .map(|section| section.id())
            .collect::<Vec<_>>(),
        vec!["overview.engine", "overview.effects"]
    );
    assert!(main.sections().iter().all(|section| section
        .control_paths()
        .iter()
        .all(|path| main.controls().iter().any(|control| control.path() == path))));

    let origin = patch.focus_path().clone();
    state
        .apply_semantic_action(SemanticAction::EnterSurface(SurfaceId::PatchUtility))
        .unwrap();
    let utility = semantic(&state);
    assert_eq!(utility.active_surface(), SurfaceId::PatchUtility);
    assert_eq!(utility.return_path().unwrap().origin(), &origin);
    let utility_controls = utility.surface(SurfaceId::PatchUtility).unwrap().controls();
    // The declared five-row Utility panel, in its declared order. Compared
    // against the one canonical declaration rather than a second list here, so
    // this asserts the projection follows the declaration instead of pinning a
    // copy of it that could drift.
    assert_eq!(utility_controls.len(), 5);
    assert_eq!(
        utility_controls
            .iter()
            .map(|control| control.path().control_id().clone())
            .collect::<Vec<_>>(),
        PatchControlId::utility_surface_descriptor()
            .iter()
            .cloned()
            .map(SemanticControlId::Patch)
            .collect::<Vec<_>>()
    );
    state.apply_semantic_action(SemanticAction::Return).unwrap();
    assert_eq!(semantic(&state).focus_path(), &origin);
    assert!(semantic(&state).return_path().is_none());

    state
        .apply_semantic_action(SemanticAction::SelectContext(TopLevelContext::Mixer))
        .unwrap();
    let mixer_origin = semantic(&state).focus_path().clone();
    state
        .apply_semantic_action(SemanticAction::EnterSurface(SurfaceId::MixerInspector))
        .unwrap();
    assert_eq!(semantic(&state).active_surface(), SurfaceId::MixerInspector);
    state.apply_semantic_action(SemanticAction::Return).unwrap();
    assert_eq!(semantic(&state).focus_path(), &mixer_origin);

    state
        .apply_semantic_action(SemanticAction::SetInteractionMode(InteractionMode::Adjust))
        .unwrap();
    assert_eq!(semantic(&state).interaction_mode(), InteractionMode::Adjust);
    state
        .apply_semantic_action(SemanticAction::SetInteractionMode(
            InteractionMode::Navigate,
        ))
        .unwrap();
    let before_reserved = semantic(&state);
    assert!(state
        .apply_semantic_action(SemanticAction::SetInteractionMode(InteractionMode::Modal))
        .is_err());
    assert_eq!(semantic(&state), before_reserved);

    let mut braids_state = installed_state(false);
    braids_state
        .apply_semantic_action(SemanticAction::SelectContext(TopLevelContext::Patch))
        .unwrap();
    braids_state
        .apply_semantic_action(SemanticAction::EnterSurface(SurfaceId::PatchDetail))
        .unwrap();
    let braids_patch = semantic(&braids_state);
    assert!(braids_patch
        .surfaces()
        .iter()
        .flat_map(|surface| surface.controls())
        .any(|control| matches!(
            control.path().capability_id(),
            Some(FocusCapabilityId::Instrument(id)) if id.as_str() == BRAIDS_CAPABILITY_ID
        )));

    let evidence = Arc::new(Mutex::new(BoundaryEvidence::default()));
    let mut app_loop = AppLoop::new(
        installed_state(true),
        StateProjector::new(),
        ProbeBoundary {
            evidence: Arc::clone(&evidence),
        },
    )
    .unwrap();
    let publications_before = evidence.lock().unwrap().parameters.len();
    let result = app_loop
        .dispatch_action(SemanticAction::Navigate(Direction::Down))
        .unwrap();
    assert_eq!(
        result.accepted().generation(),
        app_loop.current_state_tree().generation()
    );
    assert_eq!(
        evidence.lock().unwrap().parameters.len(),
        publications_before
    );
    assert!(evidence.lock().unwrap().commands.is_empty());
    let record = app_loop.event_log_ref().records().back().unwrap();
    assert_eq!(record.source(), EventSource::Keyboard);
    assert!(matches!(record.input(), EventInput::Navigate { .. }));
    assert!(record
        .emitted_events()
        .iter()
        .all(|effect| !matches!(effect, EmittedEvent::ParameterSnapshotPublished { .. })));

    let shell = app_loop.current_graphical_shell();
    assert_eq!(
        shell.footer().action_hints().len(),
        shell.semantic_model().valid_actions().len()
    );
    let large = render(shell.clone(), [1920.0, 1080.0]);
    let compact = render(shell.clone(), [1280.0, 800.0]);
    assert_eq!(large.focus_path(), compact.focus_path());
    assert_eq!(large.active_surface(), compact.active_surface());
    assert_eq!(large.interaction_mode(), compact.interaction_mode());
    assert_eq!(large.return_path(), compact.return_path());
    assert_eq!(large.valid_actions(), compact.valid_actions());
    assert_eq!(large.generation(), compact.generation());
    assert_eq!(large.state_hash(), compact.state_hash());
    for observation in [&large, &compact] {
        assert!(observation.regions_are_non_overlapping());
        assert_eq!(observation.regions().len(), ShellRegionId::ALL.len());
        assert!(observation
            .regions()
            .iter()
            .all(|region| region.rect().is_finite_nonempty()));
    }
    // "Identical despite different rectangles": the two representative
    // widths seat different main tracks while the side track remains within
    // its shared bounds.
    assert_ne!(large.viewport_width(), compact.viewport_width());
    assert_ne!(
        large.region(ShellRegionId::MainWorkspace).rect().width(),
        compact.region(ShellRegionId::MainWorkspace).rect().width(),
        "the main workspace must consume the fluid track remainder"
    );
    let side_bounds = ResponsiveShellContract::get().side_track;
    for observation in [&large, &compact] {
        let width = observation
            .region(ShellRegionId::PersistentSideRegion)
            .rect()
            .width();
        assert!((side_bounds.minimum_px..=side_bounds.maximum_px).contains(&width));
    }

    // A separately supplied projection is structurally impossible: an ack
    // whose identity is not a verbatim copy of the pushed document's is a
    // typed rejection, never an observation.
    {
        use crest_synth::shell::webview::projection_channel::PaintedAckError;
        let mut probe = ProjectionChannel::new();
        let mut emitted = None;
        probe
            .push(&shell, |document| {
                emitted = Some(document);
                Ok(())
            })
            .expect("the probe emit succeeds");
        let document = emitted.expect("the probe push emits one document");
        let mut rewritten = page_painted_ack(&document, [1920.0, 1080.0]);
        rewritten["stateHash"] = Value::from("state-invented-elsewhere");
        assert!(matches!(
            probe.forward_ack(&rewritten.to_string()),
            Err(PaintedAckError::IdentityMismatch {
                field: "stateHash",
                ..
            })
        ));
        let mut unpushed = page_painted_ack(&document, [1920.0, 1080.0]);
        unpushed["generation"] = Value::from(shell.generation() + 1_000);
        assert!(matches!(
            probe.forward_ack(&unpushed.to_string()),
            Err(PaintedAckError::UnknownDocument { .. })
        ));
    }

    let mut failed = installed_state(true);
    failed
        .apply(AppEvent::SelectContext(TopLevelContext::Patch))
        .unwrap();
    failed.apply(AppEvent::Adjust(Direction::Right)).unwrap();
    let failed_correlation = failed.engine_selection().correlation().unwrap().clone();
    failed
        .apply(AppEvent::EnginePreparationFailed {
            request_id: failed_correlation.request_id(),
            patch_id: failed_correlation.patch_id().unwrap(),
            intent: failed_correlation.intent().clone(),
            source_capability_id: failed_correlation.source_capability_id().unwrap().clone(),
            target_capability_id: failed_correlation.target_capability_id().unwrap().clone(),
            source_graph_revision: failed_correlation.source_graph_revision(),
            target_graph_revision: GraphRevision::new(2).unwrap(),
            failure: EngineSelectionFailure::AssetUnavailable,
        })
        .unwrap();
    let failed_model = semantic(&failed);
    assert_eq!(failed_model.status().kind().name(), "unavailable");
    assert_eq!(failed_model.errors().len(), 1);

    let mut recovering = installed_state(true);
    recovering
        .apply(AppEvent::SelectContext(TopLevelContext::Patch))
        .unwrap();
    let old_order = SemanticResolver::new(&recovering)
        .patch_main_paths(PatchId::new(1).unwrap())
        .unwrap();
    let overview_origin = recovering.interaction().focus_path().clone();
    recovering
        .apply(AppEvent::Adjust(Direction::Right))
        .unwrap();
    let correlation = recovering.engine_selection().correlation().unwrap().clone();
    for lifecycle in [
        crest_synth::control::EngineSelectionStatusKind::Validating,
        crest_synth::control::EngineSelectionStatusKind::Preparing,
    ] {
        recovering
            .apply(AppEvent::EngineSelectionLifecycleAdvanced {
                request_id: correlation.request_id(),
                lifecycle,
            })
            .unwrap();
    }
    let target_revision = GraphRevision::new(2).unwrap();
    recovering
        .apply(AppEvent::EnginePrepared {
            request_id: correlation.request_id(),
            patch_id: correlation.patch_id().unwrap(),
            intent: correlation.intent().clone(),
            source_capability_id: correlation.source_capability_id().unwrap().clone(),
            target_capability_id: correlation.target_capability_id().unwrap().clone(),
            source_graph_revision: correlation.source_graph_revision(),
            target_graph_revision: target_revision,
            candidate_config: BraidsCapability::new().unwrap().default_config().unwrap(),
            prepared_visualization: None,
        })
        .unwrap();
    let new_order = SemanticResolver::new(&recovering)
        .patch_main_paths(PatchId::new(1).unwrap())
        .unwrap();
    assert_eq!(new_order, old_order);
    assert_eq!(recovering.interaction().focus_path(), &overview_origin);
    assert!(matches!(
        overview_origin.control_id(),
        SemanticControlId::Patch(PatchControlId::Engine)
    ));
    recovering
        .apply(AppEvent::EngineActivationAcknowledged {
            request_id: correlation.request_id(),
            intent: correlation.intent().clone(),
            target_graph_revision: target_revision,
            retired_graph_revision: GraphRevision::INITIAL,
            collected: true,
        })
        .unwrap();
    let recovered = semantic(&recovering);
    assert_eq!(recovered.status().kind().name(), "ready");
    assert!(recovered.errors().is_empty());

    println!("CREST_ACCEPTANCE semantic_graphical_view_model passed");
}

#[test]
fn mixer_focus_matrix_inspector_return_and_density_reflow_keep_semantic_identity() {
    let mut state = installed_state(true);
    state
        .apply(AppEvent::SelectContext(TopLevelContext::Mixer))
        .unwrap();

    let rows = MixerTrackParameter::MAIN;
    for (row_index, parameter) in rows.into_iter().enumerate() {
        for track_index in 0..MixerTrackId::COUNT {
            let expected_track = MixerTrackId::new(track_index as u8).unwrap();
            assert!(matches!(
                state.interaction().focus_path().control_id(),
                SemanticControlId::Mixer(MixerControlId::Track {
                    track_id,
                    parameter: focused_parameter,
                }) if *track_id == expected_track && *focused_parameter == parameter
            ));
            let model = semantic(&state);
            let focused = model
                .surface(SurfaceId::MixerMain)
                .unwrap()
                .controls()
                .iter()
                .filter(|control| control.focused())
                .collect::<Vec<_>>();
            assert_eq!(focused.len(), 1);
            assert_eq!(focused[0].path(), state.interaction().focus_path());

            if track_index + 1 < MixerTrackId::COUNT {
                state.apply(AppEvent::Navigate(Direction::Right)).unwrap();
            }
        }

        let before_generation = state.generation();
        let before_tree = StateProjector::new()
            .project(&state)
            .unwrap()
            .0
            .json()
            .to_owned();
        assert_eq!(
            state.apply(AppEvent::Navigate(Direction::Right)),
            Err(crest_synth::control::EventRejection::ActionUnavailableInContext)
        );
        assert_eq!(state.generation(), before_generation);
        assert_eq!(
            StateProjector::new().project(&state).unwrap().0.json(),
            before_tree
        );

        for _ in 1..MixerTrackId::COUNT {
            state.apply(AppEvent::Navigate(Direction::Left)).unwrap();
        }
        if row_index + 1 < rows.len() {
            state.apply(AppEvent::Navigate(Direction::Down)).unwrap();
        }
    }

    // Return to the requested T0A / Solo origin using the same nonwrapping
    // semantic traversal a physical adapter emits.
    for _ in 0..3 {
        state.apply(AppEvent::Navigate(Direction::Up)).unwrap();
    }
    for _ in 0..10 {
        state.apply(AppEvent::Navigate(Direction::Right)).unwrap();
    }
    for _ in 0..3 {
        state.apply(AppEvent::Navigate(Direction::Down)).unwrap();
    }
    let origin = state.interaction().focus_path().clone();
    assert!(matches!(
        origin.control_id(),
        SemanticControlId::Mixer(MixerControlId::Track {
            track_id,
            parameter: MixerTrackParameter::Solo,
        }) if *track_id == MixerTrackId::new(10).unwrap()
    ));
    state
        .apply_semantic_action(SemanticAction::EnterSurface(SurfaceId::MixerInspector))
        .unwrap();
    assert_eq!(
        state.interaction().active_surface(),
        SurfaceId::MixerInspector
    );
    assert!(matches!(
        semantic(&state)
            .surface(SurfaceId::MixerInspector)
            .expect("the persistent Inspector remains projected")
            .summary(),
        SemanticSurfaceSummary::MixerInspector {
            focused_control: MixerControlId::Track {
                track_id,
                parameter: MixerTrackParameter::Solo,
            },
            focused_track,
            ..
        } if *track_id == MixerTrackId::new(10).unwrap()
            && *focused_track == MixerTrackId::new(10).unwrap()
    ));
    state.apply(AppEvent::Navigate(Direction::Down)).unwrap();
    state.apply(AppEvent::Return).unwrap();
    assert_eq!(state.interaction().focus_path(), &origin);

    // A density change paints the exact same semantic path and dispatches no
    // event. The page-facing observation can differ geometrically only.
    let before_generation = state.generation();
    let desktop = render(
        StateProjector::new().project_with_shell(&state).unwrap().3,
        [1_920.0, 1_080.0],
    );
    let compact = render(
        StateProjector::new().project_with_shell(&state).unwrap().3,
        [1_280.0, 800.0],
    );
    assert_eq!(desktop.focus_path(), &origin);
    assert_eq!(compact.focus_path(), &origin);
    assert_eq!(state.generation(), before_generation);

    state
        .apply(AppEvent::SelectContext(TopLevelContext::Patch))
        .unwrap();
    state
        .apply(AppEvent::SelectContext(TopLevelContext::Mixer))
        .unwrap();
    assert_eq!(state.interaction().focus_path(), &origin);

    assert!(semantic(&state).valid_actions().iter().all(|action| {
        action.action() != &SemanticAction::SetInteractionMode(InteractionMode::MultiSelect)
    }));
}

#[test]
fn engine_option_rows_have_one_focus_independent_current_and_reducer_actions() {
    let mut state = installed_state(true);
    state
        .apply(AppEvent::SelectContext(TopLevelContext::Patch))
        .unwrap();
    state
        .apply(AppEvent::SetInteractionMode(InteractionMode::Adjust))
        .unwrap();
    state.apply(AppEvent::Adjust(Direction::Up)).unwrap();
    let model = semantic(&state);
    let choice = model.surface(SurfaceId::PatchChoice).unwrap();
    // Options paint the current registry group; navigation wraps within it.
    let category = state
        .capabilities()
        .descriptor_for_config(state.patches()[0].instrument_config())
        .unwrap()
        .instrument_category();
    assert_eq!(
        choice.controls().len(),
        state
            .capabilities()
            .descriptors()
            .iter()
            .filter(|descriptor| descriptor.instrument_category() == category)
            .count()
    );
    assert_eq!(
        choice
            .controls()
            .iter()
            .filter(|control| control.focused())
            .count(),
        1
    );
    assert_eq!(
        choice
            .controls()
            .iter()
            .filter(|control| control.selected_label() == Some("CURRENT"))
            .count(),
        1
    );
    assert!(choice.controls()[0].focused());
    assert_eq!(choice.controls()[0].selected_label(), Some("CURRENT"));
    for control in choice.controls() {
        let has = |action: &SemanticAction| {
            control
                .valid_actions()
                .iter()
                .any(|valid| valid.action() == action)
        };
        assert!(has(&SemanticAction::Activate));
        assert!(has(&SemanticAction::Return));
        assert_eq!(
            has(&SemanticAction::Navigate(Direction::Up)),
            choice.controls().len() > 1
        );
        assert_eq!(
            has(&SemanticAction::Navigate(Direction::Down)),
            choice.controls().len() > 1
        );
    }
    assert_eq!(choice.controls()[0].valid_actions(), model.valid_actions());
    println!("SEMANTIC_OPTION_ROWS_EXACT");
}
