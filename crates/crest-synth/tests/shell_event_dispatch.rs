//! The headless event-dispatch contract.
//!
//! This target re-proves the retired native-context target's behavioral inventory —
//! event → document coherence — through the webview projection path, with no
//! native window:
//!
//! - normalized key and focus events ([`WindowInput`]) drive the shared
//!   production [`KeyboardInputTranslator`] into the production adapter
//!   callback wired to `AppLoop::dispatch_action` — the exact `KeyPipeline`
//!   composition `TauriWebviewWindow::run` builds;
//! - each frame's tick fetches the immutable projection and pushes it
//!   through the production [`ProjectionChannel`], capturing the exact
//!   serialized document the window would emit to the page;
//! - the page's paint-acknowledgment role is played headless: identity
//!   copied verbatim from the pushed document, band geometry from the
//!   authored [`ViewportDensityPolicy`], forwarded through WP02's
//!   `forward_ack` seam into one [`ShellFrameObservation`] per painted
//!   document, recorded onto the production window's qualifying-frame
//!   stream.
//!
//! Rendering a separately supplied projection is structurally impossible in
//! this harness: every assertion reads the document the real callback chain
//! produced, and `forward_ack` rejects any ack whose identity is not a
//! verbatim copy of an in-flight pushed document's — proven again at the end
//! with a typed-mismatch negative. The retired native-context target this
//! re-proof replaced was deleted with the rest of the retired shell in the
//! WP07 cutover.

use crest_synth::adapter::braids_capability::BRAIDS_CAPABILITY_ID;
use crest_synth::adapter::lock_free_audio_boundary::LockFreeAudioBoundary;
use crest_synth::adapter::production_instruments::production_capability_registry;
use crest_synth::control::app_event::AppEvent;
use crest_synth::control::app_loop::AppLoop;
use crest_synth::control::app_state::AppState;
use crest_synth::control::event_record::{
    EmittedEvent, EventDirection, EventInput, EventOutcome, EventSource,
};
use crest_synth::control::state_projector::StateProjector;
use crest_synth::control::{
    EngineSelectionEffectKind, EngineSelectionRequestId, EngineSelectionStatusKind, PatchControlId,
    TopLevelContext,
};
use crest_synth::kernel::midi_channel::MidiChannel;
use crest_synth::kernel::patch_id::PatchId;
use crest_synth::mixer::global_parameters::GlobalParameters;
use crest_synth::mixer::mixer_track_id::MixerTrackId;
use crest_synth::mixer::patch_output::PatchOutput;
use crest_synth::real_time::audio_boundary::AudioBoundary;
use crest_synth::shell::app_window::{AppInputCallback, ProjectionCallback};
use crest_synth::shell::density::ResponsiveShellContract;
use crest_synth::shell::webview::frame_stream::FrameExpectation;
use crest_synth::shell::webview::projection_channel::{
    ForwardedAck, PaintedAckError, ProjectionChannel, ProjectionPush,
};
use crest_synth::shell::webview::TauriWebviewWindow;
use crest_synth::shell::{KeyboardInputTranslator, ShellFrameObservation, WindowInput, WindowKey};
use crest_synth::synth::sound_font_instrument::SoundFontInstrument;
use crest_synth::synth::{Patch, VoiceEnvelope, VoiceEnvelopeParameter};
use crest_synth::testing::automatic_midi_test::create_soundfont_config;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::rc::Rc;

/// The single reference viewport this contract dispatches at (the compact
/// authored viewport, as the retired inventory used).
const VIEWPORT: [f32; 2] = [1_280.0, 800.0];

fn globals() -> GlobalParameters {
    GlobalParameters::new(-3.0).expect("fixture global parameters are valid")
}

fn patch(id: u32, channel: u8, output: PatchOutput) -> Patch {
    let provider = crest_synth::adapter::production_instruments::production_soundfont_capability()
        .expect("fixture capability is valid");
    Patch::new(
        PatchId::new(id).expect("fixture PatchId is valid"),
        format!("Webview Fixture {id}"),
        create_soundfont_config(
            &provider,
            SoundFontInstrument::new(0, id as u8 * 8, false).expect("fixture instrument is valid"),
        )
        .expect("fixture config matches the descriptor"),
        MidiChannel::new(channel).expect("fixture channel is valid"),
        output,
    )
    .with_envelope(VoiceEnvelope::new(500.0, 600.0, 0.5, 700.0).unwrap())
}

fn installed_state() -> AppState {
    let mut state = AppState::new(
        production_capability_registry().expect("fixture registry is valid"),
        globals(),
    );
    state
        .apply(AppEvent::InstallPatches(vec![
            patch(
                1,
                0,
                PatchOutput::new(MixerTrackId::new(0).unwrap(), -12.0)
                    .expect("fixture output is valid"),
            ),
            patch(
                2,
                1,
                PatchOutput::new(MixerTrackId::new(1).unwrap(), -6.0)
                    .expect("fixture output is valid"),
            ),
        ]))
        .expect("fixture installation is accepted");
    state
}

fn down(key: WindowKey) -> WindowInput {
    WindowInput::key_down(key)
}

fn up(key: WindowKey) -> WindowInput {
    WindowInput::key_up(key)
}

fn tree_value(
    app_loop: &AppLoop<crest_synth::adapter::lock_free_audio_boundary::LockFreeControlHandle>,
) -> Value {
    serde_json::from_str(app_loop.current_state_tree().json()).expect("StateTree is valid JSON")
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
/// verbatim from the pushed document, band geometry from the authored
/// density policy. Only an ack matching an in-flight pushed document can
/// ever become an observation.
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
/// production dispatch callback, the production projection transport, the
/// production painted-ack forwarding, the production window's stream.
struct WebviewShellHarness {
    translator: KeyboardInputTranslator,
    on_input: AppInputCallback,
    projection: ProjectionCallback,
    channel: ProjectionChannel,
    window: TauriWebviewWindow,
    frames_run: usize,
    forwarded: Vec<ShellFrameObservation>,
}

impl WebviewShellHarness {
    fn new(on_input: AppInputCallback, projection: ProjectionCallback) -> Self {
        Self {
            translator: KeyboardInputTranslator::new(),
            on_input,
            projection,
            channel: ProjectionChannel::new(),
            window: TauriWebviewWindow::new("crest-synth event dispatch"),
            frames_run: 0,
            forwarded: Vec::new(),
        }
    }

    fn frame(&mut self, inputs: Vec<WindowInput>) -> Option<(Value, ShellFrameObservation)> {
        self.frames_run += 1;
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
                    .forward_ack(&page_painted_ack(&document, VIEWPORT).to_string())
                    .expect("the painted ack for the pushed document forwards")
                {
                    ForwardedAck::Observation(observation) => observation,
                    ForwardedAck::SupersededLate { generation } => panic!(
                        "the ack for the just-pushed document cannot be late (generation {generation})"
                    ),
                };
                // Identity travels verbatim from the pushed document into
                // the forwarded observation.
                assert_eq!(
                    Some(observation.generation()),
                    document["generation"].as_u64()
                );
                assert_eq!(
                    Some(observation.state_hash()),
                    document["stateHash"].as_str()
                );
                assert!(observation.regions_are_non_overlapping());
                self.window.frame_stream().record(observation.clone());
                self.forwarded.push(observation.clone());
                Some((document, observation))
            }
        }
    }

    /// A frame that must render: an accepted event always produces exactly
    /// one new document and one forwarded observation.
    fn rendered_frame(&mut self, inputs: Vec<WindowInput>) -> (Value, ShellFrameObservation) {
        self.frame(inputs)
            .expect("an accepted event renders exactly one new document")
    }
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

/// The document's scroll target: exactly one visible control on the active
/// surface is focused, and it is the document's own focus path — the row the
/// page scrolls to and paints in the focus treatment.
fn assert_exactly_one_focused_row_is_the_scroll_target(document: &Value) {
    let focus = document
        .pointer("/focusPath/controlId")
        .cloned()
        .expect("the document names a focused control");
    let active = document
        .get("activeSurface")
        .and_then(Value::as_str)
        .expect("the document names its active surface");
    let controls = surface_controls(document, active);
    let focused: Vec<&Value> = controls
        .iter()
        .filter(|control| {
            control.get("focused").and_then(Value::as_bool) == Some(true)
                && control.get("visible").and_then(Value::as_bool) == Some(true)
        })
        .collect();
    assert_eq!(
        focused.len(),
        1,
        "exactly one visible focused row on the active surface"
    );
    assert_eq!(
        focused[0].pointer("/path/controlId"),
        Some(&focus),
        "the focused row is the document's focus path — the page's scroll target"
    );
}

/// The serialized PATCH engine row.
fn engine_row(document: &Value) -> Value {
    surface_controls(document, "patchMain")
        .into_iter()
        .find(|control| {
            control
                .pointer("/path/controlId/id")
                .and_then(Value::as_str)
                == Some("patch.engine")
        })
        .expect("the PATCH document carries the engine row")
}

#[test]
fn webview_frames_dispatch_into_app_loop_and_render_the_accepted_projection() {
    let state = installed_state();
    let projector = StateProjector::new();
    let initial = projector
        .parameter_snapshot(&state)
        .expect("fixture parameters project");
    let boundary = LockFreeAudioBoundary::new(16, initial);
    let (control, _audio) = boundary.into_handles();
    let app_loop = AppLoop::new(state, projector, control).expect("fixture AppLoop initializes");
    let before = tree_value(&app_loop);
    let shared = Rc::new(RefCell::new(app_loop));

    let input_loop = Rc::clone(&shared);
    let on_input: AppInputCallback = Box::new(move |event| {
        let event_debug = format!("{event:?}");
        input_loop
            .borrow_mut()
            .dispatch_action(event)
            .unwrap_or_else(|error| {
                panic!("webview fixture event {event_debug} is accepted: {error:?}")
            });
    });
    let projection_loop = Rc::clone(&shared);
    let projection: ProjectionCallback =
        Box::new(move || projection_loop.borrow().current_graphical_shell());
    let mut harness = WebviewShellHarness::new(on_input, projection);

    // Frame 1: the mixer edit sequence — navigate, the K chord, an
    // adjustment, the releases. One accepted generation, one document.
    let (first_document, _) = harness.rendered_frame(vec![
        down(WindowKey::D),
        down(WindowKey::D),
        down(WindowKey::K),
        down(WindowKey::D),
        up(WindowKey::D),
        up(WindowKey::K),
    ]);
    assert_eq!(first_document["context"], Value::from("mixer"));
    assert_exactly_one_focused_row_is_the_scroll_target(&first_document);

    // Idle frames: the generation gate is the webview idle economy — an
    // unchanged accepted generation serializes nothing and forwards nothing.
    for _ in 0..5 {
        assert!(
            harness.frame(Vec::new()).is_none(),
            "an idle frame must emit no document"
        );
    }

    let (retained_mixer_body, retained_mixer_line, retained_mixer_focus) = {
        let app_loop = shared.borrow();
        (
            app_loop.current_text().body().to_owned(),
            app_loop.current_text().selected_line(),
            tree_value(&app_loop)["interaction"]["rememberedMixerMain"].clone(),
        )
    };

    // Digit2: the canonical PATCH page and its document.
    let (patch_document, patch_observation) =
        harness.rendered_frame(vec![down(WindowKey::Digit2), up(WindowKey::Digit2)]);
    {
        let app_loop = shared.borrow();
        let patch_tree = tree_value(&app_loop);
        let patch_text = app_loop.current_text();
        let page = app_loop
            .current_patch_page()
            .expect("Digit2 produces the canonical PATCH page");
        let record = app_loop.event_log().records().back().cloned().unwrap();

        assert_eq!(patch_text.context(), TopLevelContext::Patch);
        assert!(patch_text.body().starts_with("PATCH | 1 MIXER | 2 PATCH"));
        assert_eq!(
            patch_text.state_hash(),
            app_loop.current_state_tree().state_hash()
        );
        assert_eq!(page.patch().id(), Some(PatchId::new(1).unwrap()));
        assert_eq!(page.state_hash(), patch_text.state_hash());
        assert_eq!(patch_tree["interaction"]["activeFocus"]["context"], "patch");
        assert_eq!(patch_tree["interaction"]["activeFocus"]["patchId"], 1);
        assert_eq!(
            patch_tree["interaction"]["rememberedMixerMain"],
            retained_mixer_focus
        );
        assert_eq!(
            patch_tree["patchPage"]["stateHash"],
            patch_text.state_hash()
        );
        assert_eq!(patch_tree["projection"]["body"], patch_text.body());
        assert!(matches!(
            record.input(),
            EventInput::SelectContext {
                context: TopLevelContext::Patch
            }
        ));
        assert_eq!(record.state_hash_after(), patch_text.state_hash());

        // The rendered document reflects the same accepted state, exactly.
        assert_eq!(patch_document["context"], Value::from("patch"));
        assert_eq!(patch_document["activeSurface"], Value::from("patchMain"));
        assert_eq!(
            patch_document["stateHash"].as_str(),
            Some(patch_text.state_hash())
        );
        assert_eq!(
            patch_document["generation"].as_u64(),
            Some(app_loop.current_state_tree().generation())
        );
        assert_exactly_one_focused_row_is_the_scroll_target(&patch_document);
        assert_eq!(patch_observation.context(), TopLevelContext::Patch);
    }

    // Enter Engine Detail, then walk its envelope rows in declared order. The
    // root keeps only the stable Engine occupancy identity.
    let (mut first_detail_document, _) = harness.rendered_frame(vec![
        down(WindowKey::Shift),
        down(WindowKey::W),
        up(WindowKey::W),
        up(WindowKey::Shift),
    ]);
    for _ in 0..16 {
        if shared
            .borrow()
            .current_patch_page()
            .unwrap()
            .focused_control_id()
            == PatchControlId::Envelope(VoiceEnvelopeParameter::AttackMilliseconds)
        {
            break;
        }
        first_detail_document = harness
            .rendered_frame(vec![down(WindowKey::S), up(WindowKey::S)])
            .0;
    }
    for (index, parameter) in [
        VoiceEnvelopeParameter::AttackMilliseconds,
        VoiceEnvelopeParameter::DecayMilliseconds,
        VoiceEnvelopeParameter::Sustain,
        VoiceEnvelopeParameter::ReleaseMilliseconds,
    ]
    .into_iter()
    .enumerate()
    {
        let focused_document = if index == 0 {
            first_detail_document.clone()
        } else {
            harness
                .rendered_frame(vec![down(WindowKey::S), up(WindowKey::S)])
                .0
        };

        {
            let app_loop = shared.borrow();
            let text = app_loop.current_text();
            let page = app_loop.current_patch_page().unwrap();
            assert_eq!(
                page.focused_control_id(),
                PatchControlId::Envelope(parameter)
            );
            // The PatchMain rows follow the header, the identity line, and
            // the projected Utility rows; derived so adding a Utility row
            // moves this with it.
            assert_eq!(text.selected_line(), 2 + page.output().len() + index + 1);
            assert_eq!(
                text.body()
                    .lines()
                    .filter(|line| line.starts_with('>'))
                    .count(),
                1
            );
            assert_exactly_one_focused_row_is_the_scroll_target(&focused_document);
        }

        if index == 0 {
            let baseline = shared.borrow().patches()[0]
                .envelope()
                .attack_milliseconds();
            let (adjusted_document, _) = harness.rendered_frame(vec![
                down(WindowKey::K),
                down(WindowKey::D),
                down(WindowKey::A),
                down(WindowKey::W),
                down(WindowKey::S),
                up(WindowKey::K),
            ]);

            let app_loop = shared.borrow();
            let text = app_loop.current_text();
            assert_eq!(
                app_loop.patches()[0].envelope().attack_milliseconds(),
                baseline,
                "K+D/A/W/S must use the reducer's reversible fine/coarse steps"
            );
            // Attack is the first row after the header, identity line, and
            // projected Utility rows.
            let page = app_loop.current_patch_page().unwrap();
            assert_eq!(text.selected_line(), 2 + page.output().len() + 1);
            assert_exactly_one_focused_row_is_the_scroll_target(&adjusted_document);
        }
    }

    // Return restores the exact Overview origin.
    let (engine_focus_document, _) =
        harness.rendered_frame(vec![down(WindowKey::A), up(WindowKey::A)]);
    {
        let app_loop = shared.borrow();
        let text = app_loop.current_text();
        let page = app_loop.current_patch_page().unwrap();
        assert_eq!(page.focused_control_id(), PatchControlId::Engine);
        // The engine row opens the PatchMain order, directly after the header,
        // the identity line, and the projected Utility rows.
        assert_eq!(text.selected_line(), 2 + page.output().len());
        assert_exactly_one_focused_row_is_the_scroll_target(&engine_focus_document);
        assert_eq!(
            engine_focus_document
                .pointer("/focusPath/controlId/id")
                .and_then(Value::as_str),
            Some("patch.engine")
        );
    }

    // K+D on the engine row: the correlated engine-selection preparation,
    // with the engine-row lifecycle status in the rendered document.
    let (pending_document, _) = harness.rendered_frame(vec![
        down(WindowKey::K),
        down(WindowKey::D),
        up(WindowKey::D),
        up(WindowKey::K),
    ]);
    {
        let app_loop = shared.borrow();
        let pending_tree = tree_value(&app_loop);
        let pending_text = app_loop.current_text();
        let page = app_loop
            .current_patch_page()
            .expect("K+D keeps the canonical PATCH page visible");
        let records = app_loop.event_log();
        let record = records
            .records()
            .iter()
            .rev()
            .find(|record| matches!(record.input(), EventInput::Adjust { .. }))
            .expect("the normalized chord emits one structural adjustment event");

        assert!(
            records.len() >= 22,
            "the complete Overview → Detail → Overview input sequence is logged"
        );
        assert_eq!(page.engine().status(), EngineSelectionStatusKind::Loading);
        assert!(!page.engine().editable());
        assert_eq!(
            page.engine().active_capability_id().as_str(),
            "instrument.soundfont.hidef"
        );
        assert_eq!(
            page.engine()
                .requested_capability_id()
                .expect("Loading identifies the requested capability")
                .as_str(),
            BRAIDS_CAPABILITY_ID
        );
        assert_eq!(
            page.engine().request_id(),
            Some(EngineSelectionRequestId::FIRST)
        );
        assert_eq!(page.engine().target_graph_revision(), None);
        assert_eq!(pending_tree["engineSelection"]["kind"], "loading");
        assert_eq!(
            pending_tree["engineSelection"]["correlation"]["requestId"],
            EngineSelectionRequestId::FIRST.value()
        );
        assert_eq!(
            pending_tree["engineSelection"]["correlation"]["targetCapabilityId"],
            BRAIDS_CAPABILITY_ID
        );
        assert_eq!(
            pending_tree["parameters"]["graphRevision"],
            page.engine().active_graph_revision().value()
        );
        assert_eq!(
            pending_tree["patchPage"]["stateHash"],
            pending_text.state_hash()
        );
        assert_eq!(pending_tree["projection"]["body"], pending_text.body());
        assert_eq!(pending_text.selected_line(), 2 + page.output().len());
        assert!(pending_text
            .body()
            .lines()
            .nth(pending_text.selected_line())
            .expect("the engine line exists")
            .starts_with("> ENGINE"));
        assert!(matches!(
            record.input(),
            EventInput::Adjust {
                direction: EventDirection::Right
            }
        ));
        assert_eq!(record.source(), EventSource::Keyboard);
        assert_eq!(record.outcome(), EventOutcome::Accepted);
        assert_eq!(
            record.generation_after() + 1,
            app_loop.current_state_tree().generation()
        );
        assert_ne!(record.state_hash_after(), pending_text.state_hash());
        assert_eq!(record.emitted_events().len(), 3);
        match &record.emitted_events()[2] {
            EmittedEvent::EngineSelection { effect } => {
                assert_eq!(effect.kind(), EngineSelectionEffectKind::PrepareRequested);
                assert_eq!(effect.request_id(), EngineSelectionRequestId::FIRST);
                assert_eq!(
                    effect.target_capability_id().unwrap().as_str(),
                    BRAIDS_CAPABILITY_ID
                );
                assert_eq!(effect.target_graph_revision(), None);
            }
            other => panic!("expected correlated preparation effect, got {other:?}"),
        }

        // The engine-row lifecycle status reached the rendered document: the
        // page's loading treatment and lifecycle band read exactly these
        // serialized fields.
        let engine = engine_row(&pending_document);
        assert_eq!(
            engine.pointer("/status/kind").and_then(Value::as_str),
            Some("loading")
        );
        assert_eq!(engine.get("editable"), Some(&Value::from(false)));
        assert_eq!(
            engine.pointer("/status/targetGraphRevision"),
            Some(&Value::Null),
            "Loading has not identified a prepared graph yet"
        );
        assert_eq!(
            engine.pointer("/status/graphRevision"),
            Some(&pending_tree["parameters"]["graphRevision"]),
            "the lifecycle band names the active graph revision"
        );
        assert_exactly_one_focused_row_is_the_scroll_target(&pending_document);
    }

    // Digit1: the MIXER return restores the remembered focus, body, and
    // selection, and the rendered document reflects all of it.
    let (final_document, final_observation) =
        harness.rendered_frame(vec![down(WindowKey::Digit1), up(WindowKey::Digit1)]);

    let app_loop = shared.borrow();
    let after_tree = app_loop.current_state_tree();
    let after = tree_value(&app_loop);
    let text = app_loop.current_text();
    let records = app_loop.event_log();

    assert_eq!(after["interaction"]["activeFocus"]["context"], "mixer");
    assert_eq!(after["interaction"]["activeFocus"], retained_mixer_focus);
    assert_eq!(
        after["interaction"]["activeFocus"]["controlId"]["id"]["kind"],
        "track"
    );
    assert_eq!(
        after["interaction"]["activeFocus"]["controlId"]["id"]["trackId"],
        2
    );
    assert_eq!(
        after["interaction"]["activeFocus"]["controlId"]["id"]["parameter"],
        "level"
    );
    assert!(after["patchPage"].is_null());
    assert_eq!(after["patches"], before["patches"]);
    for parameter in [
        "masterGainDb",
        "reverbRoomSize",
        "reverbDamping",
        "reverbReturn",
        "delayMilliseconds",
        "delayFeedback",
        "delayReturn",
    ] {
        assert_eq!(
            after["global"][parameter], before["global"][parameter],
            "{parameter}"
        );
    }
    let expected_level = before["mixer"]["tracks"][2]["levelDb"]
        .as_f64()
        .expect("baseline T02 Level is numeric")
        + 1.0;
    let observed_level = after["mixer"]["tracks"][2]["levelDb"]
        .as_f64()
        .expect("accepted T02 Level is numeric");
    assert!((observed_level - expected_level).abs() < 1.0e-6);
    for track_index in 0..16 {
        if track_index != 2 {
            assert_eq!(
                after["mixer"]["tracks"][track_index], before["mixer"]["tracks"][track_index],
                "T{track_index:02X}"
            );
        }
    }
    assert_eq!(
        after["parameters"]["mixerTracks"][2]["levelDb"],
        after["mixer"]["tracks"][2]["levelDb"]
    );

    assert_eq!(text.state_hash(), after_tree.state_hash());
    assert_eq!(text.context(), TopLevelContext::Mixer);
    assert_eq!(text.body(), retained_mixer_body);
    assert_eq!(text.selected_line(), retained_mixer_line);
    assert_eq!(after["projection"]["body"].as_str(), Some(text.body()));
    assert_eq!(
        after["projection"]["selectedLine"].as_u64(),
        Some(text.selected_line() as u64)
    );

    assert_eq!(records.len(), 23);
    let adjustment = &records.records()[3];
    assert_eq!(adjustment.source(), EventSource::Keyboard);
    assert_eq!(adjustment.outcome(), EventOutcome::Accepted);
    assert!(matches!(
        adjustment.input(),
        EventInput::Adjust {
            direction: EventDirection::Right
        }
    ));
    let last = records
        .records()
        .back()
        .expect("the MIXER return has an EventRecord");
    assert_eq!(last.source(), EventSource::Keyboard);
    assert_eq!(last.outcome(), EventOutcome::Accepted);
    assert!(matches!(
        last.input(),
        EventInput::SelectContext {
            context: TopLevelContext::Mixer
        }
    ));
    assert_eq!(last.generation_after(), after_tree.generation());
    assert_eq!(last.parameter_generation(), after_tree.generation());
    assert_eq!(last.state_hash_after(), after_tree.state_hash());
    assert_eq!(last.projection_state_hash(), after_tree.state_hash());
    assert_eq!(last.selected_line(), text.selected_line());

    // The exact selected line is the scroll target, in the diagnostic text
    // and in the rendered document alike.
    let expected_line = format!("> levelDb={observed_level}");
    assert_eq!(
        text.body().lines().nth(text.selected_line()),
        Some(expected_line.as_str())
    );
    assert_eq!(final_document["context"], Value::from("mixer"));
    assert_eq!(
        final_document["stateHash"].as_str(),
        Some(after_tree.state_hash())
    );
    assert_eq!(
        final_document["generation"].as_u64(),
        Some(after_tree.generation())
    );
    assert_exactly_one_focused_row_is_the_scroll_target(&final_document);
    assert_eq!(
        final_document
            .pointer("/focusPath/controlId/id/trackId")
            .and_then(Value::as_u64),
        Some(2),
        "the rendered document's focus is the retained mixer track"
    );

    // Frame accounting: the Detail entry and return add two accepted frames;
    // every accepted generation renders one document and one observation.
    // rendered — one document and one forwarded observation each, nothing on
    // idle frames.
    assert_eq!(harness.frames_run, 17);
    assert_eq!(harness.forwarded.len(), 12);
    assert_eq!(harness.channel.in_flight_documents(), 0);

    // The qualifying-frame stream serves the final accepted identity from a
    // handle cloned off the production window — the seam live scenes poll
    // instead of sleeping.
    let expectation = FrameExpectation::new(
        final_observation.generation(),
        final_observation.state_hash(),
        final_observation.context(),
        final_observation.active_surface(),
    );
    assert_eq!(
        harness.window.frame_stream().poll(&expectation),
        Some(final_observation.clone())
    );

    // Rendering a separately supplied projection is structurally impossible:
    // an ack whose identity is not the pushed document's, verbatim, is a
    // typed rejection — never an observation.
    drop(app_loop);
    let current = shared.borrow().current_graphical_shell();
    let mut probe = ProjectionChannel::new();
    let mut emitted = None;
    probe
        .push(&current, |document| {
            emitted = Some(document);
            Ok(())
        })
        .expect("the probe emit succeeds");
    let document = emitted.expect("the probe push emits one document");
    let mut rewritten = page_painted_ack(&document, VIEWPORT);
    rewritten["focusPath"] = json!({ "controlId": { "id": "invented.control" } });
    assert!(matches!(
        probe.forward_ack(&rewritten.to_string()),
        Err(PaintedAckError::IdentityMismatch {
            field: "focusPath",
            ..
        })
    ));
    let mut unpushed = page_painted_ack(&document, VIEWPORT);
    unpushed["generation"] = Value::from(current.generation() + 1_000);
    assert!(matches!(
        probe.forward_ack(&unpushed.to_string()),
        Err(PaintedAckError::UnknownDocument { .. })
    ));

    println!("CREST_ACCEPTANCE shell_event_dispatch passed");
}
