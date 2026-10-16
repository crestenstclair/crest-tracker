//! Acceptance for the graphical application shell.
//!
//! Retargeted by mission webview-shell-cutover-01KZAC7Q WP05 (T018): the
//! retired raw-input/tessellation mechanics are gone. The shell contract is
//! proven through the webview projection path:
//!
//! - normalized [`WindowInput`] drives the production
//!   [`KeyboardInputTranslator`] into the production adapter callback wired
//!   to `AppLoop::dispatch_action` — the exact `KeyPipeline` wiring
//!   `TauriWebviewWindow::run` composes;
//! - each tick fetches the immutable projection and pushes it through the
//!   production [`ProjectionChannel`] (the WP03 transport), capturing the
//!   exact serialized document the window would emit to the page;
//! - the page's paint-acknowledgment role is played headless: identity
//!   copied verbatim from the pushed document, band geometry from the
//!   authored [`ViewportDensityPolicy`], and WP02's `forward_ack` seam
//!   constructs the one [`ShellFrameObservation`] per painted document,
//!   recorded onto the production window's [`QualifyingFrameStream`].
//!
//! Structure, identity, ordering, bounds, and non-overlap are asserted on
//! the forwarded observation; content is asserted on the serialized rendered
//! document itself. Adapter-owned context/focus/mode/return state is
//! rejected by construction: the emitted document must be byte-identical to
//! the projector's own serialization, so nothing between the reducer and the
//! page can own or rewrite any of it. The DOM-level measurement of the same
//! bands on the real page is `tests/webview_projection_shell.rs` (T024);
//! the owned close path is the window run loop's, held by WP02's window
//! tests and the live shutdown-parity section of the same target.

use crest_synth::adapter::production_instruments::{
    production_capability_registry, production_soundfont_capability,
};
use crest_synth::control::app_event::AppEvent;
use crest_synth::control::app_loop::AppLoop;
use crest_synth::control::app_state::AppState;
use crest_synth::control::event_record::{EventInput, EventOutcome};
use crest_synth::control::state_projector::StateProjector;
use crest_synth::control::{
    GraphicalShellProjection, PatchControlId, SemanticControlId, SemanticControlValue,
    ShellSideRegion, SurfaceId, TopLevelContext,
};
use crest_synth::kernel::midi_channel::MidiChannel;
use crest_synth::kernel::patch_id::PatchId;
use crest_synth::mixer::global_parameters::GlobalParameters;
use crest_synth::mixer::mix_engine::MixEngine;
use crest_synth::mixer::mixer_track_id::MixerTrackId;
use crest_synth::mixer::patch_output::{PatchOutput, PatchOutputParameter};
use crest_synth::real_time::audio_boundary::{BoundaryFull, ControlAudioBoundary};
use crest_synth::real_time::audio_command::AudioCommand;
use crest_synth::real_time::parameter_snapshot::ParameterSnapshot;
use crest_synth::real_time::{AudioObservationSnapshot, PatchAudioBlock};
use crest_synth::shell::app_window::{AppInputCallback, ProjectionCallback};
use crest_synth::shell::density::ResponsiveShellContract;
use crest_synth::shell::tokens::{SemanticColor, ALL_WEIGHTS};
use crest_synth::shell::typeface::{family_name, AUTHORED_FAMILY};
use crest_synth::shell::webview::frame_stream::FrameExpectation;
use crest_synth::shell::webview::meter_channel::{MeterChannel, MeterEmit, METER_INTERVAL};
use crest_synth::shell::webview::projection_channel::{
    ForwardedAck, ProjectionChannel, ProjectionPush,
};
use crest_synth::shell::webview::token_export;
use crest_synth::shell::webview::TauriWebviewWindow;
use crest_synth::shell::{
    KeyboardInputTranslator, ShellFrameObservation, ShellRegionId, WindowInput, WindowKey,
};
use crest_synth::synth::sound_font_instrument::SoundFontInstrument;
use crest_synth::synth::Patch;
use crest_synth::testing::automatic_midi_test::create_soundfont_config;
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Default)]
struct BoundaryObservations {
    parameters: Vec<ParameterSnapshot>,
    commands: Vec<AudioCommand>,
}

struct ProbeBoundary {
    observations: Arc<Mutex<BoundaryObservations>>,
}

impl ControlAudioBoundary for ProbeBoundary {
    fn push_command(&mut self, command: AudioCommand) -> Result<(), BoundaryFull> {
        self.observations.lock().unwrap().commands.push(command);
        Ok(())
    }

    fn publish_parameters(&mut self, parameters: ParameterSnapshot) {
        self.observations
            .lock()
            .unwrap()
            .parameters
            .push(parameters);
    }
}

fn globals() -> GlobalParameters {
    GlobalParameters::new(-3.0).unwrap()
}

fn installed_state() -> AppState {
    let provider = production_soundfont_capability().unwrap();
    let config =
        create_soundfont_config(&provider, SoundFontInstrument::new(0, 40, false).unwrap())
            .unwrap();
    let patch = Patch::new(
        PatchId::new(1).unwrap(),
        "Graphical Shell".to_owned(),
        config,
        MidiChannel::new(0).unwrap(),
        PatchOutput::new(MixerTrackId::new(0).unwrap(), -6.0).unwrap(),
    );
    let mut state = AppState::new(production_capability_registry().unwrap(), globals());
    state.apply(AppEvent::InstallPatches(vec![patch])).unwrap();
    state
}

fn key_stroke(key: WindowKey) -> Vec<WindowInput> {
    vec![WindowInput::key_down(key), WindowInput::key_up(key)]
}

/// The page's per-band first visible text, derived from the pushed document
/// exactly as the committed page derives it (`webview-page/page.js`).
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

/// The page's paint-acknowledgment role, played headless: identity copied
/// verbatim from the pushed document (the page contract — `paintedEvidence`
/// echoes the received document, never re-derives), band geometry from the
/// authored density policy for the viewport. `forward_ack` rejects any ack
/// whose identity is not a verbatim copy of an in-flight pushed document's,
/// so this role cannot render a separately supplied projection.
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

/// The headless webview shell frame loop: the production translator into the
/// production dispatch callback, the production projection transport, and
/// the production painted-ack forwarding onto the production window's
/// qualifying-frame stream.
struct WebviewShellHarness {
    translator: KeyboardInputTranslator,
    on_input: AppInputCallback,
    projection: ProjectionCallback,
    channel: ProjectionChannel,
    window: TauriWebviewWindow,
    forwarded: Vec<ShellFrameObservation>,
}

impl WebviewShellHarness {
    fn new(on_input: AppInputCallback, projection: ProjectionCallback) -> Self {
        Self {
            translator: KeyboardInputTranslator::new(),
            on_input,
            projection,
            channel: ProjectionChannel::new(),
            window: TauriWebviewWindow::new("crest-synth acceptance"),
            forwarded: Vec::new(),
        }
    }

    /// One frame: feed normalized inputs through the shared production key
    /// state machine (the `KeyPipeline` wiring, verbatim), then tick — fetch
    /// the immutable projection, push it through the generation-gated
    /// transport, and forward the page's painted ack for whatever document
    /// was actually emitted. Returns the emitted document and its forwarded
    /// observation, or `None` when the generation gate emitted nothing.
    fn frame(
        &mut self,
        viewport: [f32; 2],
        inputs: Vec<WindowInput>,
    ) -> Option<(Value, ShellFrameObservation)> {
        for input in inputs {
            if let Some(action) = self.translator.translate(input) {
                (self.on_input)(action);
            }
        }
        let projection = (self.projection)();
        let mut emitted = None;
        let outcome = self
            .channel
            .push(&projection, |document| {
                emitted = Some(document);
                Ok(())
            })
            .expect("the tick's projection push succeeds");
        match outcome {
            ProjectionPush::Unchanged => None,
            ProjectionPush::Emitted => {
                let document =
                    emitted.expect("an Emitted push hands the emitter exactly one document");
                let observation = match self
                    .channel
                    .forward_ack(&page_painted_ack(&document, viewport).to_string())
                    .expect("the painted ack for the pushed document forwards")
                {
                    ForwardedAck::Observation(observation) => observation,
                    ForwardedAck::SupersededLate { generation } => panic!(
                        "the ack for the just-pushed document cannot be late (generation {generation})"
                    ),
                };
                self.window.frame_stream().record(observation.clone());
                self.forwarded.push(observation.clone());
                Some((document, observation))
            }
        }
    }
}

fn assert_rect(observation: &ShellFrameObservation, id: ShellRegionId, expected: [f32; 4]) {
    let rect = observation.region(id).rect();
    let actual = [rect.min_x(), rect.min_y(), rect.max_x(), rect.max_y()];
    for (actual, expected) in actual.into_iter().zip(expected) {
        assert!(
            (actual - expected).abs() <= 0.01,
            "{id:?}: expected {expected}, got {actual}"
        );
    }
}

fn assert_frame(
    observation: &ShellFrameObservation,
    projection: &GraphicalShellProjection,
    document: &Value,
    viewport: [f32; 2],
) {
    assert_eq!(observation.viewport_width(), viewport[0]);
    assert_eq!(observation.viewport_height(), viewport[1]);
    assert_eq!(observation.generation(), projection.generation());
    assert_eq!(observation.state_hash(), projection.state_hash());
    assert_eq!(observation.context(), projection.context());

    // Adapter-owned context/focus/mode/return state is rejected wholesale:
    // the emitted document must be the projector's own serialization,
    // byte-identical — no transport-side struct, trimmed field, or rewritten
    // value can survive this.
    let independent = serde_json::to_value(projection.semantic_model())
        .expect("the projector's model serializes");
    assert_eq!(
        document, &independent,
        "the rendered document must be the projector's serialization, verbatim"
    );

    assert!(
        observation.regions_are_non_overlapping(),
        "{:?}",
        observation.regions()
    );
    assert_eq!(
        observation
            .regions()
            .iter()
            .map(|region| region.id())
            .collect::<Vec<_>>(),
        ShellRegionId::surface_descriptor()
    );

    // Every band and split resolves from the density policy for this
    // viewport, so this asserts the authored geometry itself: a policy whose
    // bands stopped tiling the viewport fails the exact-rect and non-overlap
    // assertions here before any page could seat it.
    let geometry = ResponsiveShellContract::get().witness_geometry(viewport[0], viewport[1]);
    let context_bottom = geometry.context_line_px;
    let identity_bottom = geometry.workspace_y_px;
    let workspace_bottom = viewport[1] - geometry.footer_px;
    assert_rect(
        observation,
        ShellRegionId::ContextLine,
        [0.0, 0.0, viewport[0], context_bottom],
    );
    assert_rect(
        observation,
        ShellRegionId::IdentityHeader,
        [0.0, context_bottom, viewport[0], identity_bottom],
    );
    assert_rect(
        observation,
        ShellRegionId::MainWorkspace,
        [
            0.0,
            identity_bottom,
            geometry.main_width_px,
            identity_bottom + geometry.main_height_px,
        ],
    );
    assert_rect(
        observation,
        ShellRegionId::PersistentSideRegion,
        [
            geometry.side_x_px,
            geometry.side_y_px,
            geometry.side_x_px + geometry.side_width_px,
            geometry.side_y_px + geometry.side_height_px,
        ],
    );
    assert_rect(
        observation,
        ShellRegionId::Footer,
        [0.0, workspace_bottom, viewport[0], viewport[1]],
    );
    assert!(
        observation
            .region(ShellRegionId::PersistentSideRegion)
            .rect()
            .width()
            >= 320.0
    );

    // Region visibility: every region carries the visible label the page
    // derives from this document, and the identity/footer chrome names the
    // active context.
    let labels = page_band_labels(document);
    for (region, expected) in observation.regions().iter().zip(&labels) {
        assert!(!region.visible_label().trim().is_empty());
        assert_eq!(region.visible_label(), expected, "{:?}", region.id());
    }
    assert_eq!(
        observation
            .region(ShellRegionId::IdentityHeader)
            .visible_label(),
        match projection.context() {
            TopLevelContext::Patch => "PATCH",
            TopLevelContext::Mixer => "MIXER",
        }
    );
    assert!(!projection.workspace().diagnostic().body().is_empty());
}

/// One serialized surface's control array.
fn surface_controls(document: &Value, id: &str) -> Vec<Value> {
    document
        .get("surfaces")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|surface| surface.get("id").and_then(Value::as_str) == Some(id))
        .unwrap_or_else(|| panic!("the document carries the {id} surface"))
        .get("controls")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

#[test]
fn production_update_renders_both_contexts_at_both_reference_viewports() {
    let observations = Arc::new(Mutex::new(BoundaryObservations::default()));
    let app_loop = AppLoop::new(
        installed_state(),
        StateProjector::new(),
        ProbeBoundary {
            observations: Arc::clone(&observations),
        },
    )
    .unwrap();
    let initial_tree: Value = serde_json::from_str(app_loop.current_state_tree().json()).unwrap();
    let shared = Rc::new(RefCell::new(app_loop));
    let rejections = Rc::new(RefCell::new(Vec::new()));

    let input_loop = Rc::clone(&shared);
    let input_rejections = Rc::clone(&rejections);
    let on_input: AppInputCallback = Box::new(move |event| {
        if let Err(rejection) = input_loop.borrow_mut().dispatch_action(event) {
            input_rejections.borrow_mut().push(rejection);
        }
    });
    let projection_loop = Rc::clone(&shared);
    let projection: ProjectionCallback =
        Box::new(move || projection_loop.borrow().current_graphical_shell());
    let mut harness = WebviewShellHarness::new(on_input, projection);

    let cases = [
        ([1_920.0, 1_080.0], None, TopLevelContext::Mixer),
        (
            [1_920.0, 1_080.0],
            Some(WindowKey::Digit2),
            TopLevelContext::Patch,
        ),
        (
            [1_280.0, 800.0],
            Some(WindowKey::Digit1),
            TopLevelContext::Mixer,
        ),
        (
            [1_280.0, 800.0],
            Some(WindowKey::Digit2),
            TopLevelContext::Patch,
        ),
    ];

    for (viewport, key, expected_context) in cases {
        let before = harness.forwarded.len();
        let inputs = key.map_or_else(Vec::new, key_stroke);
        let (document, observation) = harness
            .frame(viewport, inputs)
            .expect("an accepted generation forwards exactly one observation");
        assert_eq!(harness.forwarded.len(), before + 1);
        let projection = shared.borrow().current_graphical_shell();
        assert_eq!(projection.context(), expected_context);
        assert_eq!(
            projection.workspace().side_region(),
            match expected_context {
                TopLevelContext::Patch => ShellSideRegion::Utility,
                TopLevelContext::Mixer => ShellSideRegion::Inspector,
            }
        );
        assert_frame(&observation, &projection, &document, viewport);

        // Full-surface generation agreement: document, semantic model, shell
        // projection, text projection, state tree, event record, and
        // parameter snapshot all report the same accepted generation.
        {
            let app_loop = shared.borrow();
            let tree = app_loop.current_state_tree();
            let text = app_loop.current_text();
            assert_eq!(
                document["generation"].as_u64(),
                Some(tree.generation()),
                "the rendered document names the accepted generation"
            );
            assert_eq!(document["stateHash"].as_str(), Some(tree.state_hash()));
            assert_eq!(text.state_hash(), tree.state_hash());
            assert_eq!(projection.generation(), tree.generation());
            assert_eq!(projection.state_hash(), tree.state_hash());
            if key.is_some() {
                let records = app_loop
                    .event_log_ref()
                    .records()
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>();
                let record = records.last().expect("the accepted key left a record");
                assert_eq!(record.outcome(), EventOutcome::Accepted);
                assert_eq!(record.generation_after(), tree.generation());
                assert_eq!(record.state_hash_after(), tree.state_hash());
                assert_eq!(record.parameter_generation(), tree.generation());
                assert_eq!(record.projection_state_hash(), tree.state_hash());
            }
        }

        match expected_context {
            TopLevelContext::Mixer => {
                // The stable sixteen-track index reaches the rendered
                // document: every track's level row serializes, in full.
                let controls = surface_controls(&document, "mixerMain");
                assert_eq!(
                    controls.len(),
                    MixerTrackId::COUNT * 4,
                    "the horizontal strip must retain every track's four main controls"
                );
                for track in 0..MixerTrackId::COUNT as u64 {
                    assert!(
                        controls.iter().any(|control| {
                            control
                                .pointer("/path/controlId/id/trackId")
                                .and_then(Value::as_u64)
                                == Some(track)
                                && control
                                    .pointer("/path/controlId/id/parameter")
                                    .and_then(Value::as_str)
                                    == Some("level")
                        }),
                        "track {track} must remain in the rendered document at {viewport:?}"
                    );
                }
                // The page's meter and Inspector key on a focused mixer
                // track; the document names one.
                assert_eq!(
                    document
                        .pointer("/focusPath/controlId/id/kind")
                        .and_then(Value::as_str),
                    Some("track"),
                    "the MIXER document's focus names a track"
                );
                let main = projection
                    .semantic_model()
                    .surface(crest_synth::control::SurfaceId::MixerMain)
                    .unwrap();
                assert_eq!(main.controls().len(), MixerTrackId::COUNT * 4);
                let inspector = projection
                    .semantic_model()
                    .surface(SurfaceId::MixerInspector)
                    .unwrap();
                // Eight indexed sends, then each unoccupied return's
                // occupancy and level rows, then master gain alone.
                let bus_count = crest_synth::mixer::bus_id::BusId::COUNT;
                assert_eq!(
                    inspector.controls().len(),
                    bus_count + bus_count * 2 + GlobalParameters::surface_descriptor().len()
                );
                assert!(inspector.controls()[..bus_count].iter().all(|control| {
                    matches!(
                        control.path().control_id(),
                        SemanticControlId::Mixer(crest_synth::control::MixerControlId::Send { .. })
                    )
                }));
                assert!(inspector.controls()[bus_count..bus_count * 3]
                    .iter()
                    .all(|control| {
                        matches!(
                            control.path().control_id(),
                            SemanticControlId::Mixer(
                                crest_synth::control::MixerControlId::ReturnOccupancy { .. }
                                    | crest_synth::control::MixerControlId::ReturnLevel { .. }
                            )
                        )
                    }));
                assert!(inspector.controls()[bus_count * 3..]
                    .iter()
                    .all(|control| matches!(
                        control.path().control_id(),
                        SemanticControlId::Mixer(
                            crest_synth::control::MixerControlId::Global { .. }
                        )
                    )));
            }
            TopLevelContext::Patch => {
                let utility = surface_controls(&document, "patchUtility");
                for label in ["Trim Gain", "Output Track"] {
                    assert!(
                        utility.iter().any(|control| {
                            control.get("label").and_then(Value::as_str) == Some(label)
                        }),
                        "the PATCH document's utility surface carries {label}"
                    );
                }
            }
        }
    }

    assert!(rejections.borrow().is_empty());
    let app_loop = shared.borrow();
    let final_tree: Value = serde_json::from_str(app_loop.current_state_tree().json()).unwrap();
    for path in [
        "/patches",
        "/global",
        "/parameters/patches",
        "/parameters/global",
        "/parameters/graphRevision",
    ] {
        assert_eq!(
            final_tree.pointer(path),
            initial_tree.pointer(path),
            "{path}"
        );
    }
    assert_eq!(app_loop.event_log_ref().records().len(), 3);
    assert!(app_loop.event_log_ref().records().iter().all(|record| {
        record.outcome() == EventOutcome::Accepted
            && matches!(record.input(), EventInput::SelectContext { .. })
    }));
    let boundary = observations.lock().unwrap();
    assert!(boundary.commands.is_empty());
    let parameter_values = boundary
        .parameters
        .iter()
        .map(|snapshot| {
            let mut value = serde_json::to_value(snapshot).unwrap();
            value["generation"] = Value::Null;
            value
        })
        .collect::<Vec<_>>();
    assert!(parameter_values.windows(2).all(|pair| pair[0] == pair[1]));
    drop(app_loop);

    // The qualifying-frame stream — the seam live scenes poll instead of
    // sleeping — serves the final accepted identity from any handle cloned
    // off the production window.
    let last = harness.forwarded.last().unwrap().clone();
    let expectation = FrameExpectation::new(
        last.generation(),
        last.state_hash(),
        last.context(),
        last.active_surface(),
    );
    assert!(harness.window.frame_stream().poll(&expectation).is_some());
    // A stale identity does not qualify.
    let stale = FrameExpectation::new(
        last.generation() + 1_000,
        last.state_hash(),
        last.context(),
        last.active_surface(),
    );
    assert!(harness.window.frame_stream().poll(&stale).is_none());

    println!("CREST_ACCEPTANCE graphical_application_shell passed");
}

/// The served page paints only through the generated token table.
///
/// The webview twin of "the production render path paints the authored
/// palette and typeface": every value the page can paint with resolves from
/// `tokens.css` — proven byte-fresh against the authored vocabulary — and
/// the page sources spell no color of their own, resolve no token the table
/// does not declare, and load exactly the four authored faces with no
/// fallback stack.
#[test]
fn the_served_page_paints_only_through_the_generated_token_table() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let read = |name: &str| {
        std::fs::read_to_string(root.join("webview-page").join(name))
            .unwrap_or_else(|error| panic!("webview-page/{name} must be readable: {error}"))
    };
    let tokens = read("tokens.css");
    let page_css = read("page.css");
    let page_js = read("page.js");
    let index_html = read("index.html");

    // The committed table is byte-fresh generator output, so page values are
    // the authored vocabulary's values by construction.
    token_export::committed_tokens_are_fresh(&tokens)
        .expect("committed tokens.css must match the authored vocabulary");

    // The two values that changed most in the component mission are present
    // at their authored resolutions.
    let hex = |color: SemanticColor| {
        let resolved = color.resolve();
        format!(
            "#{:02x}{:02x}{:02x}",
            resolved.r(),
            resolved.g(),
            resolved.b()
        )
    };
    assert!(tokens.contains(&format!(
        "--color-accent-focus: {};",
        hex(SemanticColor::AccentFocus)
    )));
    assert!(tokens.contains(&format!(
        "--color-bg-canvas: {};",
        hex(SemanticColor::BgCanvas)
    )));

    // The retired pre-mission palette is nowhere in the served page.
    for retired in [
        "#6ecdae", "#101216", "#181b20", "#1d2127", "#e6eaef", "#969ea9", "#e8ae4c",
    ] {
        for (name, source) in [
            ("tokens.css", &tokens),
            ("page.css", &page_css),
            ("page.js", &page_js),
            ("index.html", &index_html),
        ] {
            assert!(
                !source.to_lowercase().contains(retired),
                "the retired {retired} survives in {name}"
            );
        }
    }

    // The composition stylesheet spells no color: no hex literal, no
    // rgb()/hsl() constructor. (Selectors like `#footer` scan as one or two
    // hex digits and are not colors.)
    for line in page_css.lines() {
        let code = line.split("/*").next().unwrap_or(line);
        assert!(
            !code.contains("rgb(") && !code.contains("hsl(") && !code.contains("rgba("),
            "page.css builds a color of its own: {line}"
        );
        let bytes: Vec<char> = code.chars().collect();
        for (index, character) in bytes.iter().enumerate() {
            if *character != '#' {
                continue;
            }
            let run = bytes[index + 1..]
                .iter()
                .take_while(|c| c.is_ascii_hexdigit())
                .count();
            assert!(
                !matches!(run, 3 | 4 | 6 | 8),
                "page.css spells a hex color: {line}"
            );
        }
    }

    // Every custom property the page resolves is declared — by the generated
    // table or by the page's own declared fader-geometry block — so no token
    // reference can silently resolve to nothing.
    let mut declared: Vec<String> = Vec::new();
    for source in [&tokens, &page_css] {
        for line in source.lines() {
            let trimmed = line.trim_start();
            if let Some(rest) = trimmed.strip_prefix("--") {
                if let Some(colon) = rest.find(':') {
                    declared.push(format!("--{}", &rest[..colon]));
                }
            }
        }
    }
    let mut references = 0_usize;
    let mut from = 0;
    while let Some(offset) = page_css[from..].find("var(--") {
        let start = from + offset + "var(".len();
        let name: String = page_css[start..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        // `var(--x, fallback)` is a per-element document-value binding the
        // render script writes (e.g. the fader's `--level`); its explicit
        // fallback means it can never silently resolve to nothing. A bare
        // `var(--x)` must resolve to a declared token.
        let has_fallback = page_css[start + name.len()..].trim_start().starts_with(',');
        assert!(
            has_fallback || declared.iter().any(|property| property == &name),
            "page.css resolves {name}, which no token table declares"
        );
        references += 1;
        from = start;
    }
    assert!(
        references > 80,
        "page.css resolved only {references} custom properties; the page is not \
         painting through the token table"
    );

    // The authored typeface, exactly: four @font-face declarations for the
    // authored family at the four authored numeric weights, each loading a
    // vendored face that exists, and no fallback stack anywhere.
    assert_eq!(page_css.matches("@font-face").count(), ALL_WEIGHTS.len());
    assert_eq!(
        page_css
            .matches(&format!("font-family: \"{AUTHORED_FAMILY}\";"))
            .count(),
        ALL_WEIGHTS.len() + 1,
        "each face plus the body binding names the authored family, verbatim"
    );
    assert!(
        !page_css.contains(&format!("\"{AUTHORED_FAMILY}\",")),
        "the authored family must carry no fallback stack"
    );
    for weight in ALL_WEIGHTS {
        assert!(
            page_css.contains(&format!("font-weight: {};", weight.numeric())),
            "the authored numeric weight {} is not declared",
            weight.numeric()
        );
        let face = family_name(weight)
            .strip_prefix(AUTHORED_FAMILY)
            .expect("the family name carries the authored prefix")
            .trim()
            .to_owned();
        let file = format!("AzeretMono-{face}.ttf");
        assert!(
            page_css.contains(&format!("url(\"fonts/{file}\")")),
            "the {face} face is not loaded from the served fonts"
        );
        let vendored = root.join("vendor/azeret-mono").join(&file);
        let bytes = std::fs::read(&vendored)
            .unwrap_or_else(|error| panic!("{} must exist: {error}", vendored.display()));
        assert!(!bytes.is_empty(), "{file} must carry face bytes");
    }
}

#[test]
fn mixer_meter_frame_carries_one_compatible_immutable_audio_observation() {
    let state = installed_state();
    let projector = StateProjector::new();
    let parameters = projector.parameter_snapshot(&state).unwrap();
    let shell = projector.project_with_shell(&state).unwrap().3;
    let state_before = projector.project(&state).unwrap().0.json().to_owned();

    let mut patch_audio = PatchAudioBlock::prepare(2).unwrap();
    patch_audio.begin_render(&parameters, 2).unwrap();
    let patch_id = parameters.patches()[0].patch_id().unwrap();
    patch_audio
        .stem_mut(0, patch_id)
        .unwrap()
        .copy_from_slice(&[0.5, 0.5, 0.5, 0.5]);
    let mut mixer = MixEngine::new();
    mixer.prepare(48_000.0, 2).unwrap();
    let mut output = [0.0_f32; 4];
    let mix = mixer.mix(&patch_audio, &parameters, &mut output);
    let track = MixerTrackId::new(0).unwrap();
    assert!(mix.track(track).rms() > 0.0);
    let observation = AudioObservationSnapshot::from_mix_with_graph_and_routing(
        1,
        1,
        2,
        parameters.generation(),
        parameters.graph_revision(),
        0,
        0,
        0,
        None,
        mix,
    );

    // The webview meter transport: one latest-value slot, the newest
    // observation wins, and the due emit hands the page exactly the one
    // immutable snapshot — untouched.
    let mut channel = MeterChannel::new();
    channel.observe(observation);
    let mut emitted: Vec<AudioObservationSnapshot> = Vec::new();
    let now = Instant::now();
    assert!(matches!(
        channel.emit_if_due(now, |frame| {
            emitted.push(frame);
            Ok(())
        }),
        MeterEmit::Emitted
    ));
    assert_eq!(emitted.len(), 1, "exactly one meter frame per due emit");
    let frame = serde_json::to_value(emitted[0]).expect("the meter frame serializes");
    assert_eq!(
        frame,
        serde_json::to_value(observation).expect("the observation serializes"),
        "the emitted frame is the one immutable observation, untouched"
    );

    // The page's compatibility rule keys on the frame's parameter generation
    // matching the document on screen; both derive from the same accepted
    // state here, so the reading is displayable.
    let document =
        serde_json::to_value(shell.semantic_model()).expect("the projector's model serializes");
    assert_eq!(frame["parameterGeneration"], document["generation"]);
    let reading = frame["mix"]["tracks"][0]["rms"]
        .as_f64()
        .or_else(|| frame["tracks"][0]["rms"].as_f64())
        .expect("the frame carries the focused track's rms");
    assert!((reading - f64::from(mix.track(track).rms())).abs() < 1.0e-6);

    // One observation is consumed exactly once: the next due emit is idle.
    assert!(matches!(
        channel.emit_if_due(now + METER_INTERVAL, |_| Ok(())),
        MeterEmit::Idle
    ));

    // The passive meter render mutated nothing.
    assert_eq!(projector.project(&state).unwrap().0.json(), state_before);
}

#[test]
fn patch_utility_controls_dispatch_through_the_same_semantic_callback() {
    let observations = Arc::new(Mutex::new(BoundaryObservations::default()));
    let app_loop = AppLoop::new(
        installed_state(),
        StateProjector::new(),
        ProbeBoundary {
            observations: Arc::clone(&observations),
        },
    )
    .unwrap();
    let before_parameters = app_loop.current_parameters().clone();
    let initial_output = app_loop.patches()[0].output();
    let shared = Rc::new(RefCell::new(app_loop));

    let input_loop = Rc::clone(&shared);
    let on_input: AppInputCallback = Box::new(move |event| {
        input_loop
            .borrow_mut()
            .dispatch_action(event)
            .expect("PATCH Utility keyboard action is reducer-accepted");
    });
    let projection_loop = Rc::clone(&shared);
    let projection: ProjectionCallback =
        Box::new(move || projection_loop.borrow().current_graphical_shell());
    let mut harness = WebviewShellHarness::new(on_input, projection);
    let viewport = [1_280.0, 800.0];

    harness
        .frame(viewport, key_stroke(WindowKey::Digit2))
        .expect("selecting PATCH renders a document");
    let (utility_document, _) = harness
        .frame(viewport, key_stroke(WindowKey::D))
        .expect("entering the utility surface renders a document");
    let utility_rows = surface_controls(&utility_document, "patchUtility");
    for label in ["Trim Gain", "Output Track"] {
        assert!(
            utility_rows
                .iter()
                .any(|control| control.get("label").and_then(Value::as_str) == Some(label)),
            "the rendered utility surface carries {label}"
        );
    }
    assert_eq!(
        shared.borrow().current_semantic_model().active_surface(),
        SurfaceId::PatchUtility
    );

    let (adjusted_document, _) = harness
        .frame(
            viewport,
            vec![
                WindowInput::key_down(WindowKey::K),
                WindowInput::key_down(WindowKey::D),
                WindowInput::key_up(WindowKey::D),
                WindowInput::key_up(WindowKey::K),
            ],
        )
        .expect("the K+D chord renders a document");
    let app_loop = shared.borrow();
    let expected_trim = initial_output.trim_gain_db()
        + PatchOutputParameter::TrimGain
            .descriptor()
            .fine_step()
            .unwrap();
    assert!((app_loop.patches()[0].output().trim_gain_db() - expected_trim).abs() < 1.0e-6);
    assert_eq!(
        app_loop.patches()[0].output().track_id(),
        initial_output.track_id()
    );
    assert_eq!(
        app_loop.current_parameters().graph_revision(),
        before_parameters.graph_revision()
    );
    assert_eq!(
        app_loop.current_parameters().mixer_tracks(),
        before_parameters.mixer_tracks()
    );
    assert_eq!(
        app_loop.current_parameters().global(),
        before_parameters.global()
    );
    assert_eq!(
        app_loop.current_parameters().patches()[0].output(),
        app_loop.patches()[0].output()
    );
    let semantic = app_loop.current_semantic_model();
    // Found by identity, not by position: the Utility panel declares five rows
    // and master volume is seated above trim, so an index here would silently
    // assert against a different control.
    let trim_id = SemanticControlId::Patch(PatchControlId::Output(PatchOutputParameter::TrimGain));
    let trim_control = semantic
        .surface(SurfaceId::PatchUtility)
        .unwrap()
        .controls()
        .iter()
        .find(|control| control.path().control_id() == &trim_id)
        .expect("the Utility panel carries the canonical trim row");
    assert_eq!(trim_control.path().control_id(), &trim_id);
    assert_eq!(
        trim_control.value(),
        &SemanticControlValue::Scalar(expected_trim as f64)
    );
    // The accepted edit reached the rendered document itself: the serialized
    // trim row carries the exact accepted value.
    let adjusted_rows = surface_controls(&adjusted_document, "patchUtility");
    let trim_row = adjusted_rows
        .iter()
        .find(|control| {
            control
                .pointer("/path/controlId/id")
                .and_then(Value::as_str)
                == Some("patch.output.trimGainDb")
        })
        .expect("the rendered document carries the trim row");
    let rendered_trim = trim_row
        .pointer("/value/value")
        .and_then(Value::as_f64)
        .expect("the trim row serializes a scalar");
    assert!((rendered_trim - f64::from(expected_trim)).abs() < 1.0e-6);
    assert!(app_loop.event_log_ref().records().iter().any(|record| {
        record.outcome() == EventOutcome::Accepted
            && matches!(
                record.input(),
                EventInput::Adjust {
                    direction: crest_synth::control::event_record::EventDirection::Right
                }
            )
    }));
    let observed_parameters = observations
        .lock()
        .unwrap()
        .parameters
        .last()
        .cloned()
        .expect("the accepted trim edit publishes one parameter snapshot");
    assert_eq!(
        observed_parameters.patches()[0].output(),
        app_loop.current_parameters().patches()[0].output()
    );
    assert_eq!(
        observed_parameters.mixer_tracks(),
        app_loop.current_parameters().mixer_tracks()
    );
    assert_eq!(
        observed_parameters.graph_revision(),
        app_loop.current_parameters().graph_revision()
    );
    assert_eq!(
        observed_parameters.generation() + 1,
        app_loop.current_parameters().generation(),
        "the following release-K navigation event is audio-neutral"
    );
}

/// The webview idle economy, replacing the retired repaint-delay and false-tick
/// assertions: every tick fetches the immutable projection (the port
/// invariant), but an unchanged accepted generation emits nothing — no
/// serialization, no push, no observation. The close-once ordering on a
/// false tick is the window run loop's own behavior, held by the WP02 window
/// tests and the live shutdown-parity section of
/// `tests/webview_projection_shell.rs`.
#[test]
fn an_unchanged_generation_tick_emits_no_document() {
    let app_loop = AppLoop::new(
        installed_state(),
        StateProjector::new(),
        ProbeBoundary {
            observations: Arc::new(Mutex::new(BoundaryObservations::default())),
        },
    )
    .unwrap();
    let shared = Rc::new(RefCell::new(app_loop));
    let fetches = Rc::new(Cell::new(0_usize));
    let projection_loop = Rc::clone(&shared);
    let fetch_count = Rc::clone(&fetches);
    let projection: ProjectionCallback = Box::new(move || {
        fetch_count.set(fetch_count.get() + 1);
        projection_loop.borrow().current_graphical_shell()
    });

    let mut channel = ProjectionChannel::new();
    let mut emits = 0_usize;
    for tick in 0..4 {
        let current = projection();
        let outcome = channel
            .push(&current, |_| {
                emits += 1;
                Ok(())
            })
            .expect("an idle tick's push succeeds");
        if tick == 0 {
            assert_eq!(outcome, ProjectionPush::Emitted);
        } else {
            assert_eq!(
                outcome,
                ProjectionPush::Unchanged,
                "an unchanged generation must emit nothing on tick {tick}"
            );
        }
    }
    assert_eq!(fetches.get(), 4, "every tick fetches the projection");
    assert_eq!(emits, 1, "one accepted generation, one emitted document");
}
