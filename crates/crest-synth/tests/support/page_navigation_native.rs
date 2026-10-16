//! Bounded actual AppKit capture followed by the production reducer and page.
use super::*;
use crest_synth::control::{SavedSession, SemanticAction};
#[cfg(target_os = "macos")]
use objc2::MainThreadMarker;
#[cfg(target_os = "macos")]
use objc2_app_kit::{NSApplication, NSEvent, NSEventModifierFlags, NSEventType};
#[cfg(target_os = "macos")]
use objc2_foundation::{NSPoint, NSString};
use tauri::Manager;

#[cfg(target_os = "macos")]
fn post_key(
    app: &NSApplication,
    window_number: isize,
    code: u16,
    pressed: bool,
    shift: bool,
    repeat: bool,
) -> Result<(), String> {
    let kind = if code == 56 {
        NSEventType::FlagsChanged
    } else if pressed {
        NSEventType::KeyDown
    } else {
        NSEventType::KeyUp
    };
    let flags = if shift {
        NSEventModifierFlags::Shift
    } else {
        NSEventModifierFlags::empty()
    };
    let characters = NSString::from_str("");
    let event = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
        kind, NSPoint::new(0.0, 0.0), flags,
        objc2_foundation::NSProcessInfo::processInfo().systemUptime(), window_number, None,
        &characters, &characters, repeat, code).ok_or("native key event did not construct")?;
    app.postEvent_atStart(&event, false);
    Ok(())
}

#[cfg(target_os = "macos")]
fn capture(
    handle: &tauri::AppHandle,
    keys: &mpsc::Receiver<SemanticAction>,
    code: u16,
    shift: bool,
    expected: &SemanticAction,
    tag: &str,
) -> Result<SemanticAction, String> {
    let (sent, received) = mpsc::channel();
    handle
        .run_on_main_thread(move || {
            let result = (|| {
                let app = NSApplication::sharedApplication(
                    MainThreadMarker::new().ok_or("native driver left main thread")?,
                );
                let window = app
                    .windows()
                    .iter()
                    .find(|window| {
                        window.title().to_string() == "crest-synth WP06 acceptance harness"
                    })
                    .ok_or("owned native window missing")?;
                #[allow(deprecated)]
                app.activateIgnoringOtherApps(true);
                window.makeKeyAndOrderFront(None);
                let number = window.windowNumber();
                if shift {
                    post_key(&app, number, 56, true, true, false)?;
                }
                post_key(&app, number, code, true, shift, false)?;
                if shift {
                    post_key(&app, number, code, true, shift, true)?;
                }
                post_key(&app, number, code, false, shift, false)?;
                if shift {
                    post_key(&app, number, 56, false, false, false)?;
                }
                Ok::<(), String>(())
            })();
            let _ = sent.send(result);
        })
        .map_err(|error| error.to_string())?;
    received
        .recv_timeout(Duration::from_secs(5))
        .map_err(|error| error.to_string())??;
    let action = keys
        .recv_timeout(Duration::from_secs(5))
        .map_err(|error| format!("{tag}: native input missing: {error}"))?;
    if &action != expected {
        return Err(format!("{tag}: expected {expected:?}, captured {action:?}"));
    }
    Ok(action)
}

#[cfg(target_os = "linux")]
fn capture(
    _handle: &tauri::AppHandle,
    keys: &mpsc::Receiver<SemanticAction>,
    code: u16,
    shift: bool,
    expected: &SemanticAction,
    tag: &str,
) -> Result<SemanticAction, String> {
    // Preserve the existing journey's physical-key fixtures on X11/XWayland.
    // Explicit window 0 uses XTest on the active window; a search result would
    // otherwise select XSendEvent, which GTK does not treat as physical input.
    let key = match code {
        14 => "e",
        12 => "q",
        126 => "Up",
        125 => "Down",
        123 => "Left",
        124 => "Right",
        13 => "w",
        1 => "s",
        0 => "a",
        2 => "d",
        _ => return Err(format!("{tag}: unmapped native fixture {code}")),
    };
    if shift {
        let repeat = Command::new("xset")
            .args(["r", "rate", "150", "2"])
            .status()
            .map_err(|error| error.to_string())?;
        if !repeat.success() {
            return Err(format!("{tag}: configuring native autorepeat failed"));
        }
    }
    let mut command = Command::new("xdotool");
    command.args([
        "search",
        "--sync",
        "--onlyvisible",
        "--name",
        "crest-synth WP06 acceptance harness",
        "windowactivate",
        "--sync",
    ]);
    if shift {
        command.args(["keydown", "--window", "0", "Shift_L"]);
    }
    command.args(["keydown", "--window", "0", key]);
    if shift {
        command.args(["sleep", "0.3"]);
    }
    command.args(["keyup", "--window", "0", key]);
    if shift {
        command.args(["keyup", "--window", "0", "Shift_L"]);
    }
    let output = command.output().map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "{tag}: native injection failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let action = keys
        .recv_timeout(Duration::from_secs(5))
        .map_err(|error| format!("{tag}: native input missing: {error}"))?;
    if &action != expected {
        return Err(format!("{tag}: expected {expected:?}, captured {action:?}"));
    }
    Ok(action)
}

fn paint(
    window: &tauri::WebviewWindow,
    receiver: &mpsc::Receiver<Value>,
    state: &AppState,
    tag: &str,
) -> Result<(), String> {
    let model = StateProjector::new()
        .project_with_shell(state)
        .map_err(|error| error.to_string())?
        .3;
    let document =
        serde_json::to_value(model.semantic_model()).map_err(|error| error.to_string())?;
    let observed = observe_render(window, receiver, &document.to_string(), tag)?;
    assert_eq!(observed["generation"], document["generation"], "{tag}");
    assert_eq!(
        observed["paintAcknowledgment"]["generation"], document["generation"],
        "{tag}"
    );
    assert_eq!(
        observed["semanticEvidence"]["activeSurface"], document["activeSurface"],
        "{tag}"
    );
    assert_eq!(
        observed["semanticEvidence"]["returnIdentity"], document["returnPath"],
        "{tag}"
    );
    assert_eq!(
        observed["focus"]["matchCount"], 1,
        "{tag}: one painted focus"
    );
    let focus: Value = serde_json::from_str(
        observed["focus"]["semanticPath"]
            .as_str()
            .expect("painted focus path"),
    )
    .unwrap();
    assert_eq!(
        focus, document["focusPath"],
        "{tag}: painted exact identity"
    );
    assert_eq!(
        observed["focus"]["semanticVisible"], true,
        "{tag}: focus visible"
    );
    let expected_guidance = document["validActions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|action| {
            action["hint"]
                .as_str()
                .map(|hint| format!("{hint}:{}", hint_label(action["label"].as_str().unwrap())))
        })
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(
        observed["footer"]["guidance"]
            .as_str()
            .unwrap()
            .split_whitespace()
            .collect::<Vec<_>>(),
        expected_guidance.split_whitespace().collect::<Vec<_>>(),
        "{tag}: truthful footer"
    );
    assert_eq!(observed["actionGuidance"]["offFooterCount"], 0, "{tag}");
    std::fs::write(
        evidence_dir().join(format!("{tag}.json")),
        serde_json::to_vec_pretty(&observed).unwrap(),
    )
    .map_err(|error| error.to_string())?;
    println!(
        "PAGE_NAVIGATION {tag}: {:?}, {:?}",
        state.interaction().active_surface(),
        state.interaction().focus_path()
    );
    Ok(())
}

pub(super) fn drive(
    handle: &tauri::AppHandle,
    receiver: &mpsc::Receiver<Value>,
    ready: &mpsc::Receiver<()>,
    keys: &mpsc::Receiver<SemanticAction>,
) -> Result<(), String> {
    ready
        .recv_timeout(Duration::from_secs(30))
        .map_err(|_| "page transport readiness timed out")?;
    let window = handle
        .get_webview_window("main")
        .ok_or("owned window missing")?;
    window.show().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())?;
    window.eval(format!("requestAnimationFrame(function() {{ window.__TAURI__.event.emit('{HARNESS_EVENT}', {{phase:'page-ready'}}); }});")).map_err(|error| error.to_string())?;
    receive_phase(receiver, "page-ready", Duration::from_secs(10))?;
    let mut state = production_patch_state();
    let saved = SavedSession::capture(&state);
    let first_patch = state.patches()[0].id();
    let second_patch = state.patches()[1].id();
    let engine_origin = state.interaction().focus_path().clone();
    paint(&window, receiver, &state, "page-startup-overview")?;
    let steps = [
        (
            14,
            false,
            SemanticAction::SelectPatch(Direction::Right),
            SurfaceId::PatchMain,
            "page-e-next",
        ),
        (
            12,
            false,
            SemanticAction::SelectPatch(Direction::Left),
            SurfaceId::PatchMain,
            "page-q-previous",
        ),
        (
            126,
            true,
            SemanticAction::NavigatePage(Direction::Up),
            SurfaceId::PatchDetail,
            "page-instrument-detail",
        ),
        (
            125,
            true,
            SemanticAction::NavigatePage(Direction::Down),
            SurfaceId::PatchMain,
            "page-instrument-return",
        ),
        (
            125,
            false,
            SemanticAction::Navigate(Direction::Down),
            SurfaceId::PatchMain,
            "page-highlight-effect",
        ),
        (
            126,
            true,
            SemanticAction::NavigatePage(Direction::Up),
            SurfaceId::PatchDetail,
            "page-effect-detail",
        ),
        (
            125,
            true,
            SemanticAction::NavigatePage(Direction::Down),
            SurfaceId::PatchMain,
            "page-effect-return",
        ),
        (
            125,
            true,
            SemanticAction::NavigatePage(Direction::Down),
            SurfaceId::MixerMain,
            "page-open-mixer",
        ),
        (
            124,
            false,
            SemanticAction::Navigate(Direction::Right),
            SurfaceId::MixerMain,
            "page-mixer-track",
        ),
        (
            125,
            false,
            SemanticAction::Navigate(Direction::Down),
            SurfaceId::MixerMain,
            "page-mixer-pan",
        ),
        (
            126,
            true,
            SemanticAction::NavigatePage(Direction::Up),
            SurfaceId::PatchMain,
            "page-restore-patch",
        ),
        (
            125,
            true,
            SemanticAction::NavigatePage(Direction::Down),
            SurfaceId::MixerMain,
            "page-restore-mixer",
        ),
        (
            126,
            true,
            SemanticAction::NavigatePage(Direction::Up),
            SurfaceId::PatchMain,
            "page-restore-patch-again",
        ),
        (
            123,
            true,
            SemanticAction::NavigatePage(Direction::Left),
            SurfaceId::MidiDeviceSettings,
            "page-settings",
        ),
        (
            124,
            true,
            SemanticAction::NavigatePage(Direction::Right),
            SurfaceId::PatchMain,
            "page-settings-return",
        ),
        (
            13,
            true,
            SemanticAction::NavigatePage(Direction::Up),
            SurfaceId::PatchDetail,
            "page-wasd-detail",
        ),
        (
            1,
            true,
            SemanticAction::NavigatePage(Direction::Down),
            SurfaceId::PatchMain,
            "page-wasd-return",
        ),
    ];
    let mut effect_origin = None;
    let mut mixer_origin = None;
    for (code, shift, expected, surface, tag) in steps {
        let generation = state.generation();
        let action = capture(handle, keys, code, shift, &expected, tag)?;
        let outcome = state
            .apply_semantic_action(action)
            .map_err(|error| format!("{tag}: reducer rejected {error:?}"))?;
        assert_eq!(state.generation(), generation + 1, "{tag}: one transition");
        assert_eq!(
            state.interaction().active_surface(),
            surface,
            "{tag}: destination"
        );
        assert!(!outcome.accepted().saved_session_changed());
        assert_eq!(outcome.audio_command(), None);
        assert_eq!(outcome.engine_selection_effect(), None);
        assert_eq!(SavedSession::capture(&state), saved);
        match tag {
            "page-e-next" => assert_eq!(state.interaction().patch_focus(), Some(second_patch)),
            "page-q-previous" | "page-instrument-return" => {
                assert_eq!(state.interaction().focus_path(), &engine_origin)
            }
            "page-instrument-detail" => assert!(matches!(
                state.interaction().detail_subject(),
                Some(crest_synth::control::PatchDetailSubject::Instrument { .. })
            )),
            "page-highlight-effect" => {
                effect_origin = Some(state.interaction().focus_path().clone())
            }
            "page-effect-detail" | "page-wasd-detail" => {
                assert_eq!(state.interaction().patch_focus(), Some(first_patch));
                assert_eq!(
                    state.interaction().return_path().unwrap().origin(),
                    effect_origin.as_ref().unwrap()
                );
                assert!(
                    matches!(state.interaction().detail_subject(), Some(crest_synth::control::PatchDetailSubject::Effect { slot_id, .. }) if *slot_id == EffectSlotIndex::ALL[0].instance_identity())
                );
            }
            "page-mixer-pan" => mixer_origin = Some(state.interaction().focus_path().clone()),
            "page-restore-mixer" => assert_eq!(
                state.interaction().focus_path(),
                mixer_origin.as_ref().unwrap()
            ),
            "page-effect-return"
            | "page-restore-patch"
            | "page-restore-patch-again"
            | "page-settings-return"
            | "page-wasd-return" => assert_eq!(
                state.interaction().focus_path(),
                effect_origin.as_ref().unwrap()
            ),
            _ => {}
        }
        paint(&window, receiver, &state, tag)?;
        if let Ok(extra) = keys.recv_timeout(Duration::from_millis(80)) {
            return Err(format!("{tag}: extra native activation {extra:?}"));
        }
    }
    println!("PAGE_NAVIGATION authored journey passed: 17 native activations, including Shift+Right Settings return.");
    Ok(())
}
