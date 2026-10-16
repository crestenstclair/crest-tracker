//! Controller Settings reducer documents through the production Tauri transport
//! and an owned WKWebView. Device events are deterministic fixtures; this proves
//! native rendering and Settings behavior, not physical gamepad discovery.
use super::*;
use crest_synth::control::{
    ControllerBindings, ControllerButton, ControllerDevice, ControllerEvent, ControllerFailure,
    ControllerRole, SavedSession, SemanticAction,
};
use tauri::Manager;

struct Witness<'a> {
    handle: &'a tauri::AppHandle,
    window: &'a tauri::WebviewWindow,
    receiver: &'a mpsc::Receiver<Value>,
    painted: &'a PaintedAcks,
    errors: &'a RenderErrors,
    channel: ProjectionChannel,
}

impl Witness<'_> {
    fn paint(&mut self, state: &AppState, tag: &str) -> Result<Value, String> {
        let projection = StateProjector::new()
            .project_with_shell(state)
            .map_err(|error| error.to_string())?
            .3;
        let document =
            serde_json::to_value(projection.semantic_model()).map_err(|error| error.to_string())?;
        let cursor = self.painted.lock().expect("painted lock").len();
        self.channel
            .push(&projection, |payload| {
                tauri::Emitter::emit(self.handle, PROJECTION_EVENT, payload)
            })
            .map_err(|error| format!("{tag}: projection transport: {error}"))?;
        let deadline = Instant::now() + Duration::from_secs(10);
        let ack = loop {
            let ack = self
                .painted
                .lock()
                .expect("painted lock")
                .iter()
                .skip(cursor)
                .find(|(generation, _, _)| *generation == state.generation())
                .map(|(_, _, ack)| ack.clone());
            if let Some(ack) = ack {
                break ack;
            }
            let errors = self.errors.lock().expect("render error lock");
            if !errors.is_empty() || Instant::now() >= deadline {
                return Err(format!(
                    "{tag}: no painted acknowledgment; render errors {errors:?}"
                ));
            }
            drop(errors);
            std::thread::sleep(Duration::from_millis(10));
        };
        for field in PAINTED_ACK_IDENTITY_FIELDS {
            assert_eq!(ack[field], document[field], "{tag}: painted {field}");
        }
        assert!(
            matches!(
                self.channel.forward_ack(&ack.to_string()),
                Ok(ForwardedAck::Observation(_))
            ),
            "{tag}: real acknowledgment accepted by its production channel"
        );

        // Observe the already-painted DOM. Never rerender or write product state
        // from JavaScript, so transport failures cannot hide behind this probe.
        let script = r#"(function () {
          var expected = DOCUMENT;
          function text(node) { return node ? node.textContent.replace(/\s+/g, ' ').trim() : null; }
          function rect(node) {
            var r = node.getBoundingClientRect();
            return {x:r.x,y:r.y,width:r.width,height:r.height,right:r.right,bottom:r.bottom};
          }
          var nodes = Array.from(document.querySelectorAll('[data-focus-path]'));
          var focused = nodes.filter(function (node) {
            return node.getAttribute('data-focus-path') === JSON.stringify(expected.focusPath);
          });
          var list = document.querySelector('[data-role="controller-button-list"]');
          var status = document.querySelector('.controller-settings-status');
          var primaryHint = Array.from(document.querySelectorAll('#footer [data-role="action-hint"]')).find(function (node) {
            return text(node).startsWith('Return:');
          });
          window.__TAURI__.event.emit('EVENT', {phase:'TAG', observation:{
            focusedCount:focused.length,
            focusedPath:focused.length ? JSON.parse(focused[0].getAttribute('data-focus-path')) : null,
            focusedBounds:focused.length ? rect(focused[0]) : null,
            treatedRows:document.querySelectorAll('.controller-button-row.focused').length,
            currentPage:(document.querySelector('[data-settings-page][aria-current="page"]') || {}).dataset?.settingsPage || null,
            sessionRows:Array.from(document.querySelectorAll('.session-file-row')).map(function (row) {
              return {path:JSON.parse(row.getAttribute('data-focus-path')), label:text(row.querySelector('.type-label')), disabled:row.getAttribute('aria-disabled'), bounds:rect(row)};
            }),
            sessionFocusedRows:document.querySelectorAll('.session-file-row.focused').length,
            overflow:document.documentElement.scrollWidth > window.innerWidth,
            footerOverflow:document.getElementById('footer').scrollHeight > document.getElementById('footer').clientHeight,
            rows:Array.from(document.querySelectorAll('.controller-button-row')).map(function (node) {
              return {path:JSON.parse(node.getAttribute('data-focus-path')),
                label:text(node.querySelector('.midi-row-identity')),
                value:text(node.querySelector('.controller-button-value')),
                editable:node.getAttribute('data-editable') === 'true', bounds:rect(node)};
            }),
            listBounds:list ? rect(list) : null,
            capture:status ? status.getAttribute('data-controller-capture') : null,
            preferenceStatus:status ? status.getAttribute('data-controller-preference-status') : null,
            status:text(status && status.querySelector('.type-label')),
            inspector:text(document.getElementById('inspector')),
            footer:text(document.getElementById('footer')),
            primaryHintBounds:primaryHint ? rect(primaryHint) : null,
            footerBounds:rect(document.getElementById('footer')),
            width:window.innerWidth,height:window.innerHeight
          }});
        })();"#
            .replace("DOCUMENT", &document.to_string())
            .replace("EVENT", HARNESS_EVENT)
            .replace("TAG", tag);
        self.window
            .eval(script)
            .map_err(|error| error.to_string())?;
        let message = receive_phase(self.receiver, tag, Duration::from_secs(10))?;
        let observed = message["observation"].clone();
        assert_eq!(
            observed["focusedCount"], 1,
            "{tag}: singular semantic focus"
        );
        assert_eq!(observed["focusedPath"], document["focusPath"], "{tag}");
        if state.interaction().active_surface() == SurfaceId::ControllerSettings {
            let surface = document["surfaces"]
                .as_array()
                .unwrap()
                .iter()
                .find(|surface| surface["id"] == "controllerSettings")
                .unwrap();
            let controls = surface["controls"].as_array().unwrap();
            let rows = observed["rows"].as_array().unwrap();
            assert_eq!(rows.len(), ControllerRole::ALL.len() + 1, "{tag}");
            assert_eq!(rows.len(), controls.len(), "{tag}: complete projected list");
            for (row, control) in rows.iter().zip(controls) {
                assert_eq!(row["path"], control["path"], "{tag}: stable row identity");
                assert_eq!(row["label"], control["label"], "{tag}: role label");
                assert_eq!(
                    row["value"], control["value"]["value"],
                    "{tag}: mapping value"
                );
                assert_eq!(row["editable"], control["editable"], "{tag}: editability");
                assert!(row["bounds"]["width"].as_f64().unwrap() > 0.0, "{tag}");
                assert!(row["bounds"]["height"].as_f64().unwrap() > 0.0, "{tag}");
            }
            assert_eq!(
                observed["treatedRows"], 1,
                "{tag}: one focused row treatment"
            );
            assert_eq!(observed["currentPage"], "controllerSettings", "{tag}");
            assert_eq!(observed["status"], surface["summary"]["summary"], "{tag}");
            if document["validActions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|action| action["hint"] == "Return")
            {
                let hint = &observed["primaryHintBounds"];
                assert!(
                    hint["y"].as_f64().unwrap() >= observed["footerBounds"]["y"].as_f64().unwrap(),
                    "{tag}: primary action above footer scroll port"
                );
                assert!(
                    hint["bottom"].as_f64().unwrap() <= observed["height"].as_f64().unwrap(),
                    "{tag}: primary action below viewport"
                );
            }
            let bounds = &observed["focusedBounds"];
            let list = &observed["listBounds"];
            assert!(
                bounds["y"].as_f64().unwrap() >= list["y"].as_f64().unwrap() - 1.0,
                "{tag}: focus above scroll port"
            );
            assert!(
                bounds["bottom"].as_f64().unwrap() <= list["bottom"].as_f64().unwrap() + 1.0,
                "{tag}: focus below scroll port"
            );
            assert!(
                bounds["bottom"].as_f64().unwrap() <= observed["height"].as_f64().unwrap(),
                "{tag}: focus below viewport"
            );
        }
        if state.interaction().active_surface() == SurfaceId::SaveLoadSettings {
            let surface = document["surfaces"]
                .as_array()
                .unwrap()
                .iter()
                .find(|surface| surface["id"] == "saveLoadSettings")
                .unwrap();
            let rows = observed["sessionRows"].as_array().unwrap();
            assert_eq!(rows.len(), 3, "{tag}: all file actions painted");
            for (row, control) in rows.iter().zip(surface["controls"].as_array().unwrap()) {
                assert_eq!(row["path"], control["path"], "{tag}: file action identity");
                assert_eq!(row["label"], control["label"], "{tag}: file action label");
                assert!(row["bounds"]["width"].as_f64().unwrap() > 0.0);
            }
            assert_eq!(observed["sessionFocusedRows"], 1);
            assert_eq!(observed["currentPage"], "saveLoadSettings");
            assert_eq!(observed["overflow"], false, "{tag}: no horizontal overflow");
            assert_eq!(
                observed["footerOverflow"], false,
                "{tag}: all Settings guidance visible"
            );
        }
        std::fs::write(
            evidence_dir().join(format!("{tag}.json")),
            serde_json::to_vec_pretty(&serde_json::json!({
                "document": document, "paintAcknowledgment": ack, "observation": observed
            }))
            .unwrap(),
        )
        .map_err(|error| error.to_string())?;
        println!("CONTROLLER_SETTINGS {tag}: native paint and singular focus passed");
        Ok(observed)
    }

    fn session_document(
        &self,
        document: crest_synth::shell::SessionDocumentProjection,
        tag: &str,
    ) -> Result<Value, String> {
        let expected = serde_json::to_value(&document).map_err(|error| error.to_string())?;
        tauri::Emitter::emit(self.handle, "crest://session-document", &document)
            .map_err(|error| error.to_string())?;
        let script = r#"(function () {
          var expected = DOCUMENT, remaining = 120;
          function text(selector) { var node = document.querySelector(selector); return node ? node.textContent : null; }
          function inspect() {
            var marker = document.querySelector('[data-document-marker]');
            var name = text('[data-role="session-name"]');
            var status = text('[data-role="session-status"]');
            if (!marker || marker.dataset.documentMarker !== expected.marker || name !== expected.name || !status.includes(expected.status)) {
              if (--remaining > 0) { requestAnimationFrame(inspect); return; }
              throw new Error('Session document update did not paint');
            }
            window.__TAURI__.event.emit('EVENT', {phase:'TAG', observation:{
              name:name, marker:marker.dataset.documentMarker, status:status,
              dirty:text('[data-role="session-dirty"]'), failure:text('[data-role="session-failure"]'),
              disabled:Array.from(document.querySelectorAll('.session-file-row')).map(function (row) { return row.getAttribute('aria-disabled'); }),
              focusedRows:document.querySelectorAll('.session-file-row.focused').length
            }});
          }
          inspect();
        })();"#
            .replace("DOCUMENT", &expected.to_string())
            .replace("EVENT", HARNESS_EVENT)
            .replace("TAG", tag);
        self.window
            .eval(script)
            .map_err(|error| error.to_string())?;
        let observed =
            receive_phase(self.receiver, tag, Duration::from_secs(10))?["observation"].clone();
        assert_eq!(observed["name"], expected["name"]);
        assert_eq!(observed["failure"], expected["failure"]);
        assert_eq!(observed["focusedRows"], 1);
        assert_eq!(
            observed["dirty"],
            if document.dirty() {
                "● UNSAVED CHANGES"
            } else {
                "✓ NO UNSAVED CHANGES"
            }
        );
        for disabled in observed["disabled"].as_array().unwrap() {
            assert_eq!(
                disabled,
                if document.marker() == crest_synth::shell::SessionDocumentMarker::Busy {
                    "true"
                } else {
                    "false"
                }
            );
        }
        Ok(observed)
    }

    fn screenshot(&self, tag: &str) -> Result<(), String> {
        let path = evidence_dir().join(format!("{tag}.png"));
        sample_detail_native::capture(self.window, &path)?;
        println!("CONTROLLER_SETTINGS native screenshot: {}", path.display());
        Ok(())
    }
}

fn action(state: &mut AppState, action: SemanticAction) {
    let outcome = state
        .apply_semantic_action(action)
        .expect("Settings action admitted");
    assert!(!outcome.accepted().saved_session_changed());
    assert_eq!(outcome.audio_command(), None);
}

fn event(state: &mut AppState, event: ControllerEvent) {
    state
        .apply(AppEvent::Controller(event))
        .expect("controller event admitted");
}

pub(super) fn drive(
    handle: &tauri::AppHandle,
    receiver: &mpsc::Receiver<Value>,
    ready: &mpsc::Receiver<()>,
    painted: &PaintedAcks,
    errors: &RenderErrors,
) -> Result<(), String> {
    ready
        .recv_timeout(Duration::from_secs(30))
        .map_err(|_| "page transport readiness timed out")?;
    let window = handle
        .get_webview_window("main")
        .ok_or("owned window missing")?;
    window.show().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())?;
    for (name, viewport) in [
        ("wide", RepresentativeViewport::WideReference.fixture()),
        (
            "standard",
            RepresentativeViewport::StandardReference.fixture(),
        ),
    ] {
        window
            .set_size(tauri::LogicalSize::new(
                f64::from(viewport.width_px),
                f64::from(viewport.height_px),
            ))
            .map_err(|error| error.to_string())?;
        std::thread::sleep(Duration::from_millis(300));
        assert_page_viewport_width(
            &window,
            receiver,
            viewport.width_px,
            &format!("controller-{name}-viewport"),
        )?;
        let mut witness = Witness {
            handle,
            window: &window,
            receiver,
            painted,
            errors,
            channel: ProjectionChannel::new(),
        };
        let mut state = production_patch_state();
        event(
            &mut state,
            ControllerEvent::PreferencesLoaded { result: Ok(None) },
        );
        event(
            &mut state,
            ControllerEvent::DevicesChanged {
                devices: vec![ControllerDevice {
                    id: 7,
                    name: "Native witness gamepad".into(),
                }],
            },
        );
        action(&mut state, SemanticAction::Navigate(Direction::Down));
        let origin = state.interaction().clone();
        let saved = SavedSession::capture(&state);
        action(&mut state, SemanticAction::OpenMidiSettings);
        let midi = witness.paint(&state, &format!("controller-{name}-midi-entry"))?;
        assert_eq!(midi["currentPage"], "midiDeviceSettings");
        action(&mut state, SemanticAction::Navigate(Direction::Right));
        let normal = witness.paint(&state, &format!("controller-{name}-normal"))?;
        assert_eq!(normal["rows"][0]["value"], ControllerButton::DPadUp.label());
        assert!(normal["inspector"]
            .as_str()
            .unwrap()
            .contains("Native witness gamepad"));
        witness.screenshot(&format!("controller-{name}-normal"))?;
        action(&mut state, SemanticAction::Activate);
        let capture = witness.paint(&state, &format!("controller-{name}-capture"))?;
        assert_eq!(capture["capture"], "up");
        assert_eq!(capture["rows"][0]["value"], "PRESS A BUTTON…");
        witness.screenshot(&format!("controller-{name}-capture"))?;
        event(
            &mut state,
            ControllerEvent::ButtonCaptured {
                device_id: 7,
                button: ControllerButton::South,
            },
        );
        let swap = witness.paint(&state, &format!("controller-{name}-swap"))?;
        assert_eq!(swap["capture"], "");
        assert_eq!(swap["rows"][0]["value"], ControllerButton::South.label());
        assert_eq!(swap["rows"][4]["value"], ControllerButton::DPadUp.label());
        assert_eq!(swap["preferenceStatus"], "saving");
        let bindings = state.controller().bindings().clone();
        event(
            &mut state,
            ControllerEvent::PreferencesSaved {
                bindings,
                result: Err(ControllerFailure::PreferenceWrite),
            },
        );
        let failed = witness.paint(&state, &format!("controller-{name}-save-failed"))?;
        assert_eq!(failed["preferenceStatus"], "failed");
        assert_eq!(failed["status"], ControllerFailure::PreferenceWrite.label());
        assert_eq!(failed["rows"][0]["value"], ControllerButton::South.label());
        action(&mut state, SemanticAction::Activate);
        action(&mut state, SemanticAction::NavigatePage(Direction::Right));
        assert_eq!(
            state.interaction().active_surface(),
            SurfaceId::ControllerSettings
        );
        assert_eq!(state.controller().capture(), None);
        witness.paint(&state, &format!("controller-{name}-capture-cancel"))?;
        action(&mut state, SemanticAction::Activate);
        event(
            &mut state,
            ControllerEvent::DevicesChanged { devices: vec![] },
        );
        let disconnected = witness.paint(&state, &format!("controller-{name}-disconnected"))?;
        assert_eq!(disconnected["capture"], "");
        assert!(disconnected["inspector"]
            .as_str()
            .unwrap()
            .contains("No controllers connected"));
        assert!(
            disconnected["rows"].as_array().unwrap()[..ControllerRole::ALL.len()]
                .iter()
                .all(|row| row["editable"] == false)
        );
        for index in 0..ControllerRole::ALL.len() {
            action(&mut state, SemanticAction::Navigate(Direction::Down));
            witness.paint(&state, &format!("controller-{name}-row-{}", index + 1))?;
        }
        action(&mut state, SemanticAction::Activate);
        assert_eq!(
            state.controller().bindings(),
            &ControllerBindings::default()
        );
        let reset = witness.paint(&state, &format!("controller-{name}-reset"))?;
        assert_eq!(reset["rows"][0]["value"], ControllerButton::DPadUp.label());
        assert_eq!(reset["rows"][4]["value"], ControllerButton::South.label());
        let bindings = state.controller().bindings().clone();
        event(
            &mut state,
            ControllerEvent::PreferencesSaved {
                bindings,
                result: Ok(()),
            },
        );
        let saved_buttons = witness.paint(&state, &format!("controller-{name}-saved"))?;
        assert_eq!(saved_buttons["preferenceStatus"], "saved");
        action(&mut state, SemanticAction::Navigate(Direction::Left));
        let midi = witness.paint(&state, &format!("controller-{name}-midi-switch"))?;
        assert_eq!(midi["currentPage"], "midiDeviceSettings");
        action(&mut state, SemanticAction::Navigate(Direction::Right));
        witness.paint(&state, &format!("controller-{name}-controller-switch"))?;
        action(&mut state, SemanticAction::Navigate(Direction::Right));
        witness.paint(&state, &format!("save-load-{name}-entry"))?;
        use crest_synth::shell::{SessionDocumentMarker, SessionDocumentProjection};
        // These shell-only updates must paint without any product generation change.
        let generation = state.generation();
        for (suffix, marker, dirty, operation, status, failure) in [
            (
                "ready",
                SessionDocumentMarker::Ready,
                true,
                None,
                "READY",
                None,
            ),
            (
                "saving",
                SessionDocumentMarker::Busy,
                true,
                Some("SAVE"),
                "WRITING FILE",
                None,
            ),
            (
                "failed",
                SessionDocumentMarker::Error,
                true,
                None,
                "FAILED — PRIOR SESSION UNCHANGED",
                Some("Could not write the session. Check folder permissions and try Save As."),
            ),
            (
                "saved",
                SessionDocumentMarker::Ready,
                false,
                None,
                "READY",
                None,
            ),
        ] {
            witness.session_document(
                SessionDocumentProjection::new(
                    "Evening & <Morning>.crest",
                    dirty,
                    marker,
                    operation.map(str::to_owned),
                    status,
                    failure.map(str::to_owned),
                ),
                &format!("save-load-{name}-{suffix}"),
            )?;
            witness.screenshot(&format!("save-load-{name}-{suffix}"))?;
            assert_eq!(state.generation(), generation);
        }
        for row in 1..3 {
            action(&mut state, SemanticAction::Navigate(Direction::Down));
            witness.paint(&state, &format!("save-load-{name}-row-{row}"))?;
        }
        witness.screenshot(&format!("save-load-{name}-load"))?;
        action(&mut state, SemanticAction::NavigatePage(Direction::Right));
        assert_eq!(
            state.interaction(),
            &origin,
            "exact suspended page, focus, and mode restored"
        );
        assert_eq!(
            SavedSession::capture(&state),
            saved,
            "controller preferences leave saved session unchanged"
        );
        witness.paint(&state, &format!("controller-{name}-exact-return"))?;
    }
    assert!(
        errors.lock().expect("render error lock").is_empty(),
        "healthy Settings journey emits no render errors"
    );
    Ok(())
}
