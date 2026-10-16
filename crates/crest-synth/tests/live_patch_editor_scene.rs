//! Deterministic acceptance for the functional Patch editor live scene's
//! **plan**: what it declares, where its work lands, and that its declared
//! controlled negative fails on reach rather than on its own scaffold.
//!
//! The live run itself needs a real window, a physical audio device, and a
//! display seating 1920×1080. Everything below is the half of that run which
//! is decidable without any of them: the plan is frozen from the production
//! fixture's own installed state, and every claim here is checked by
//! dispatching the plan's own events through the production `AppLoop` and
//! reducer — never by reading the plan back as its own confirmation.

#[allow(dead_code)]
mod support;

use crest_synth::adapter::braids_capability::BraidsCapability;
use crest_synth::adapter::lock_free_audio_boundary::LockFreeAudioBoundary;
use crest_synth::adapter::production_effects::{
    production_chorus_config, production_effect_providers, production_effect_registry,
};
use crest_synth::adapter::production_instruments::{
    production_capability_registry, production_instrument_providers,
    production_soundfont_capability,
};
use crest_synth::control::app_event::{AppEvent, Direction};
use crest_synth::control::app_loop::AppLoop;
use crest_synth::control::app_state::{AppState, EventRejection};
use crest_synth::control::event_log::EventLog;
use crest_synth::control::event_record::EventSource;
use crest_synth::control::state_projector::StateProjector;
use crest_synth::control::{PatchPositionId, SemanticAction, SemanticControlId};
use crest_synth::kernel::midi_channel::MidiChannel;
use crest_synth::kernel::patch_id::PatchId;
use crest_synth::mixer::mixer_state::MixerState;
use crest_synth::mixer::mixer_track_id::MixerTrackId;
use crest_synth::mixer::patch_output::PatchOutput;
use crest_synth::real_time::audio_boundary::AudioBoundary;
use crest_synth::real_time::graph_revision::GraphRevision;
use crest_synth::real_time::parameter_snapshot::ParameterSnapshot;
use crest_synth::synth::effect_slot_id::EffectSlotIndex;
use crest_synth::synth::patch::Patch;
use crest_synth::synth::sound_font_instrument::SoundFontInstrument;
use crest_synth::synth::voice_limit::VoiceLimit;
use crest_synth::testing::automatic_midi_test::{create_soundfont_config, AutomaticMidiTest};
use crest_synth::testing::live_demo_scene::LiveDemoScene;
use crest_synth::testing::live_patch_editor_scene::{
    from_installed_state, PatchSelectionMode, PATCH_EDITOR_SCENE_NAME,
};
use crest_synth::testing::{LiveTopologySupport, LiveTopologyTransition};
use support::{globals, FixtureMidiSource};

fn production_creation_blueprint(
    registry: &crest_synth::synth::CapabilityRegistry,
) -> crest_synth::control::PatchCreationBlueprint {
    crest_synth::control::PatchCreationBlueprint::resolve(
        &crest_synth::synth::CapabilityId::new(
            crest_synth::adapter::hidef_soundfont_capability::HIDEF_CAPABILITY_ID,
        )
        .expect("the production default capability identity is valid"),
        &crest_synth::synth::DescriptorDefaultConfigFactory::new(
            registry.clone(),
            production_instrument_providers().expect("production providers are valid"),
        ),
    )
    .expect("the production default Patch blueprint resolves")
}

/// The production fixture, installed through the production composition.
fn installed_fixture(
) -> AppLoop<crest_synth::adapter::lock_free_audio_boundary::LockFreeControlHandle> {
    let global = globals();
    let initial = ParameterSnapshot::new(0, global, MixerState::default(), &[])
        .expect("initial parameters are valid");
    let boundary = LockFreeAudioBoundary::new(512, initial);
    let (control, _audio) = boundary.into_handles();
    let event_log = EventLog::new(4096).expect("fixture journal capacity is valid");
    let providers = production_instrument_providers().expect("production providers are valid");
    let registry = production_capability_registry().expect("production registry is valid");
    let effect_providers =
        production_effect_providers().expect("production effect providers are valid");
    let effects = production_effect_registry().expect("production effect registry is valid");
    let mut app_loop = AppLoop::with_event_log(
        AppState::new_with_effects(registry.clone(), effects.clone(), global)
            .with_initial_returns(
                crest_synth::adapter::production_effects::startup_bus_returns(&effects),
            )
            .with_patch_creation_blueprint(production_creation_blueprint(&registry)),
        StateProjector::for_graph(GraphRevision::INITIAL),
        control,
        event_log,
    )
    .expect("initial application state projects");
    let mut automatic = AutomaticMidiTest::new(FixtureMidiSource::new());
    automatic
        .initialize_with_effects(&providers, &effect_providers, &mut app_loop)
        .expect("fixture initializes through AppLoop");
    app_loop
}

/// A reducer fixture with the same production registries and an explicitly
/// sized installed order. The live roster is longer than the two-Patch MIDI
/// unit fixture, so boundary planning must be exercised against that shape.
fn installed_fixture_with_patch_count(
    patch_count: usize,
) -> AppLoop<crest_synth::adapter::lock_free_audio_boundary::LockFreeControlHandle> {
    assert!((2..=MixerTrackId::COUNT).contains(&patch_count));
    let global = globals();
    let soundfont = production_soundfont_capability().expect("production capability is valid");
    let braids = BraidsCapability::new().expect("production capability is valid");
    let registry = production_capability_registry().expect("production registry is valid");
    let effects = production_effect_registry().expect("production effect registry is valid");
    let patches = (0..patch_count)
        .map(|index| {
            let mut patch = Patch::new(
                PatchId::new(index as u32 + 1).expect("fixture PatchId is valid"),
                format!("Roster Patch {}", index + 1),
                if index % 2 == 0 {
                    create_soundfont_config(
                        &soundfont,
                        SoundFontInstrument::new(0, index as u8, false)
                            .expect("fixture preset is valid"),
                    )
                    .expect("fixture config matches the production descriptor")
                } else {
                    braids
                        .default_config()
                        .expect("fixture config matches the production descriptor")
                },
                MidiChannel::new(index as u8).expect("fixture channel is valid"),
                PatchOutput::to_track(
                    MixerTrackId::new(index as u8).expect("fixture track is valid"),
                ),
            );
            if index == 0 {
                let slot = EffectSlotIndex::ALL[0];
                patch = patch.with_effect_slot(
                    slot,
                    production_chorus_config(slot.instance_identity())
                        .expect("fixture Chorus config is valid"),
                );
            }
            patch
        })
        .collect();
    let mut state = AppState::new_with_effects(registry.clone(), effects.clone(), global)
        .with_initial_returns(
            crest_synth::adapter::production_effects::startup_bus_returns(&effects),
        )
        .with_patch_creation_blueprint(production_creation_blueprint(&registry));
    state
        .apply(AppEvent::InstallPatches(patches))
        .expect("installing the explicit roster is accepted");

    let initial = ParameterSnapshot::new(0, global, MixerState::default(), &[])
        .expect("initial parameters are valid");
    let boundary = LockFreeAudioBoundary::new(512, initial);
    let (control, _audio) = boundary.into_handles();
    AppLoop::with_event_log(
        state,
        StateProjector::for_graph(GraphRevision::INITIAL),
        control,
        EventLog::new(4096).expect("fixture journal capacity is valid"),
    )
    .expect("explicit roster projects through the production AppLoop")
}

fn transition<'a>(scene: &'a LiveDemoScene, id: &str) -> &'a LiveTopologyTransition {
    scene
        .expected_topology_transitions()
        .iter()
        .find(|transition| transition.identifier() == id)
        .unwrap_or_else(|| panic!("the scene declares a {id} transition"))
}

/// Every `AppEvent` one support script dispatches, in order.
fn support_events(items: &[LiveTopologySupport]) -> Vec<AppEvent> {
    items
        .iter()
        .filter_map(|item| match item {
            LiveTopologySupport::Event { event } => Some(event.clone()),
            _ => None,
        })
        .collect()
}

fn installed_ids(
    app_loop: &AppLoop<crest_synth::adapter::lock_free_audio_boundary::LockFreeControlHandle>,
) -> Vec<PatchId> {
    app_loop
        .patches()
        .iter()
        .map(crest_synth::synth::patch::Patch::id)
        .collect()
}

/// Which Patch the canonical projection currently speaks for.
fn focused_patch(
    app_loop: &AppLoop<crest_synth::adapter::lock_free_audio_boundary::LockFreeControlHandle>,
) -> Option<PatchId> {
    app_loop.current_semantic_model().focus_path().patch_id()
}

fn focused_patch_position(
    app_loop: &AppLoop<crest_synth::adapter::lock_free_audio_boundary::LockFreeControlHandle>,
) -> Option<PatchPositionId> {
    app_loop
        .current_semantic_model()
        .focus_path()
        .patch_position()
}

/// The scene's subject after the switch is the **second** installed Patch, and
/// it gets there by dispatching the production gesture through the production
/// reducer — not by naming it.
///
/// The assertion is made by replaying the plan's own switch script through
/// `AppLoop` and reading the reducer's focus back, so a plan that declared the
/// right Patch and dispatched nothing would fail here.
#[test]
fn the_switch_reaches_the_second_patch_through_the_production_gesture() {
    let mut app_loop = installed_fixture();
    let installed = installed_ids(&app_loop);
    assert!(
        installed.len() > 1,
        "the reach claim is meaningless on a one-Patch fixture",
    );
    let scene = from_installed_state(&app_loop.current_state_tree(), PatchSelectionMode::Gesture)
        .expect("the installed fixture produces a patch-editor scene");
    assert_eq!(scene.name(), PATCH_EDITOR_SCENE_NAME);

    let switch = transition(&scene, "PatchEditor.switchToSubject");
    let events = support_events(switch.support_before());
    assert!(
        events
            .iter()
            .any(|event| matches!(event, AppEvent::SelectPatch(Direction::Right))),
        "the switch must travel the SelectPatch gesture, not a direct poke",
    );
    for event in events {
        app_loop
            .dispatch_from(event.clone(), EventSource::DemoScene)
            .unwrap_or_else(|rejection| {
                panic!("the plan's {event:?} must be accepted: {rejection}")
            });
    }

    // The canonical projection — what the shell paints — names the second
    // installed Patch, and so does the PATCH surface it speaks for.
    assert_eq!(
        app_loop.current_semantic_model().focus_path().patch_id(),
        Some(installed[1]),
    );
    assert_eq!(focused_patch(&app_loop), Some(installed[1]));
    assert_ne!(installed[1], installed[0]);
    assert_eq!(switch.sounding_patch(), installed[1]);
}

/// Every declared slot position is visited on the second Patch, and every
/// declared occupancy action names that Patch — never a list carried across
/// from the first.
#[test]
fn the_effect_slot_journey_targets_every_position_on_the_second_patch() {
    let app_loop = installed_fixture();
    let installed = installed_ids(&app_loop);
    let scene = from_installed_state(&app_loop.current_state_tree(), PatchSelectionMode::Gesture)
        .expect("the installed fixture produces a patch-editor scene");

    let mut filled = Vec::new();
    let mut restored = Vec::new();
    for transition in scene.expected_topology_transitions() {
        let Some(SemanticAction::SetSlotOccupancy {
            patch_id,
            slot,
            entry,
        }) = transition.action()
        else {
            continue;
        };
        assert_eq!(
            *patch_id,
            installed[1],
            "{} targets the wrong Patch",
            transition.identifier(),
        );
        if transition.identifier().starts_with("PatchEditor.slotFill.") {
            filled.push(*slot);
            assert!(entry.is_some(), "a fill must occupy the position");
        } else {
            restored.push(*slot);
        }
        // The journey walks to the slot's own row and proves the landing.
        assert!(
            transition.support_before().iter().any(|item| matches!(
                item,
                LiveTopologySupport::VerifyPatchFocus {
                    control: crest_synth::control::PatchControlId::EffectSlot(actual)
                } if actual == slot
            )),
            "{} must verify focus on its own slot row",
            transition.identifier(),
        );
        // And it proves which Patch the projection speaks for before it edits.
        assert!(
            transition.support_before().iter().any(|item| matches!(
                item,
                LiveTopologySupport::VerifyPatchSubject { patch_id: actual } if *actual == installed[1]
            )),
            "{} must verify its subject before editing",
            transition.identifier(),
        );
    }
    filled.sort_by_key(|slot| slot.index());
    restored.sort_by_key(|slot| slot.index());
    assert_eq!(filled, EffectSlotIndex::ALL.to_vec());
    assert_eq!(
        restored,
        EffectSlotIndex::ALL.to_vec(),
        "the journey restores the grid it found",
    );
}

/// `patches.first()` never resolves this scene's subject. The negative is the
/// one place the first Patch is the subject, and it is reached by removing the
/// gesture rather than by renaming the target.
#[test]
fn the_defeated_scene_removes_the_gesture_and_stays_on_the_first_patch() {
    let mut app_loop = installed_fixture();
    let installed = installed_ids(&app_loop);
    let defeated =
        from_installed_state(&app_loop.current_state_tree(), PatchSelectionMode::Defeated)
            .expect("the installed fixture produces a defeated patch-editor scene");

    let switch = transition(&defeated, "PatchEditor.switchToSubject");
    let events = support_events(switch.support_before());
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AppEvent::SelectPatch(_))),
        "the defeat is the removal of the gesture",
    );
    for event in events {
        app_loop
            .dispatch_from(event.clone(), EventSource::DemoScene)
            .unwrap_or_else(|rejection| {
                panic!("the defeated plan's {event:?} must still be accepted: {rejection}")
            });
    }
    assert_eq!(
        focused_patch(&app_loop),
        Some(installed[0]),
        "the defeated scene never leaves the first instrument",
    );

    // The work still happens — three slots, an audible edit, a voice-limit
    // burst — which is exactly why the counters must key off where it landed
    // and not off how much of it there was.
    let targeted: Vec<PatchId> = defeated
        .expected_topology_transitions()
        .iter()
        .filter_map(|transition| match transition.action() {
            Some(SemanticAction::SetSlotOccupancy { patch_id, .. }) => Some(*patch_id),
            _ => None,
        })
        .collect();
    assert_eq!(targeted.len(), EffectSlotIndex::ALL.len() * 2);
    assert!(targeted.iter().all(|id| *id == installed[0]));

    // Both scenes declare the same transition identities, so the negative
    // exercises the same journey and can only differ in where it lands. The
    // comparison is made from a *fresh* fixture: the loop above has moved this
    // one, and a scene frozen from a moved state would be a different scene.
    let gesture = from_installed_state(
        &installed_fixture().current_state_tree(),
        PatchSelectionMode::Gesture,
    )
    .expect("the installed fixture produces a patch-editor scene");
    let ids = |scene: &LiveDemoScene| {
        scene
            .expected_topology_transitions()
            .iter()
            .map(|transition| transition.identifier().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&defeated), ids(&gesture));
}

/// The end-of-order refusal must be a genuine boundary in whichever scene
/// declares it. The gesture scene reaches the trailing empty position before
/// probing right; the controlled negative still probes left of the first
/// created Patch.
#[test]
fn the_end_of_order_refusal_is_a_real_boundary_in_both_modes() {
    for mode in [PatchSelectionMode::Gesture, PatchSelectionMode::Defeated] {
        let mut app_loop = installed_fixture_with_patch_count(5);
        let installed = installed_ids(&app_loop);
        let scene = from_installed_state(&app_loop.current_state_tree(), mode)
            .expect("the installed fixture produces a patch-editor scene");
        let refusal = transition(&scene, "PatchEditor.switchRefusedAtEnd");
        assert_eq!(refusal.expected_rejection(), Some("parameterAtBoundary"));
        let Some(SemanticAction::SelectPatch(direction)) = refusal.action() else {
            panic!("the end-of-order refusal must be a SelectPatch request");
        };

        // Walk to this mode's subject, then run the boundary transition's own
        // roster-derived support. The gesture scene reaches the actual last
        // Patch even when the roster has more than two entries; the defeated
        // scene remains on the first Patch and probes left. The right-hand
        // walk must include the interaction-only empty position.
        let switch = transition(&scene, "PatchEditor.switchToSubject");
        for event in support_events(switch.support_before()) {
            app_loop
                .dispatch_from(event, EventSource::DemoScene)
                .expect("the plan's switch script is accepted");
        }
        let subject = match mode {
            PatchSelectionMode::Gesture => installed[1],
            PatchSelectionMode::Defeated => installed[0],
        };
        assert_eq!(focused_patch(&app_loop), Some(subject));
        for event in support_events(refusal.support_before()) {
            app_loop
                .dispatch_from(event, EventSource::DemoScene)
                .expect("the boundary walk is accepted");
        }
        let expected_boundary = match mode {
            PatchSelectionMode::Gesture => PatchPositionId::TrailingEmpty,
            PatchSelectionMode::Defeated => PatchPositionId::Created(installed[0]),
        };
        assert_eq!(focused_patch_position(&app_loop), Some(expected_boundary));
        let focus_before = focused_patch_position(&app_loop);
        assert_eq!(
            app_loop.dispatch_from(AppEvent::SelectPatch(*direction), EventSource::DemoScene),
            Err(EventRejection::ParameterAtBoundary),
            "{mode:?}: the declared boundary direction must actually be a boundary",
        );
        assert_eq!(focused_patch_position(&app_loop), focus_before);
        for event in support_events(refusal.support_after()) {
            app_loop
                .dispatch_from(event, EventSource::DemoScene)
                .expect("the boundary return walk is accepted");
        }
        assert_eq!(
            focused_patch(&app_loop),
            Some(subject),
            "{mode:?}: the boundary probe restores the scene subject",
        );
    }
}

/// The voice-limit script is derived, not guessed: every declared step is
/// accepted, the ceiling lands exactly on the declared minimum, and one more
/// step would be a boundary refusal.
#[test]
fn the_voice_limit_script_lands_exactly_on_the_declared_minimum() {
    let mut app_loop = installed_fixture();
    let installed = installed_ids(&app_loop);
    let scene = from_installed_state(&app_loop.current_state_tree(), PatchSelectionMode::Gesture)
        .expect("the installed fixture produces a patch-editor scene");
    let bite = transition(&scene, "PatchEditor.voiceLimitBite");
    let ceiling_before = projected_voice_limit(&app_loop, installed[1])
        .expect("the subject publishes its own ceiling");

    for event in support_events(transition(&scene, "PatchEditor.switchToSubject").support_before())
    {
        app_loop
            .dispatch_from(event, EventSource::DemoScene)
            .expect("the plan's switch script is accepted");
    }
    let mut note_ons = 0_usize;
    for event in support_events(bite.support_before()) {
        if matches!(event, AppEvent::Midi { .. }) {
            note_ons += 1;
        }
        app_loop
            .dispatch_from(event.clone(), EventSource::DemoScene)
            .unwrap_or_else(|rejection| {
                panic!("the voice-limit script's {event:?} must be accepted: {rejection}")
            });
    }

    let descriptor = VoiceLimit::descriptor();
    assert_eq!(
        projected_voice_limit(&app_loop, installed[1]),
        Some(u32::from(descriptor.minimum())),
        "the derived step count must land exactly on the declared minimum",
    );
    assert!(
        note_ons > usize::from(descriptor.minimum()),
        "the burst must sound past the lowered ceiling for the limit to bite",
    );

    // The restoring script walks back to the same row. Halfway through it —
    // focused on the ceiling row in Edit mode — one more step down is a
    // boundary refusal, which is what makes the derived count exact rather
    // than merely sufficient.
    let restore = support_events(bite.support_after());
    let first_raise = restore
        .iter()
        .position(|event| matches!(event, AppEvent::Adjust(Direction::Up)))
        .expect("the restoring script raises the ceiling it lowered");
    for event in &restore[..first_raise] {
        app_loop
            .dispatch_from(event.clone(), EventSource::DemoScene)
            .unwrap_or_else(|rejection| {
                panic!("the restoring script's {event:?} must be accepted: {rejection}")
            });
        for completion in support::topology_activation_events(app_loop.engine_selection_status()) {
            app_loop
                .dispatch_from(completion, EventSource::System)
                .unwrap();
        }
    }
    assert_eq!(
        app_loop.dispatch_from(AppEvent::Adjust(Direction::Down), EventSource::DemoScene),
        Err(EventRejection::ParameterAtBoundary),
        "the ceiling is already at the declared minimum",
    );
    for event in &restore[first_raise..] {
        app_loop
            .dispatch_from(event.clone(), EventSource::DemoScene)
            .unwrap_or_else(|rejection| {
                panic!("the restoring script's {event:?} must be accepted: {rejection}")
            });
        for completion in support::topology_activation_events(app_loop.engine_selection_status()) {
            app_loop
                .dispatch_from(completion, EventSource::System)
                .unwrap();
        }
    }
    assert_eq!(
        projected_voice_limit(&app_loop, installed[1]),
        Some(ceiling_before),
        "the restoring script must put the ceiling back where it found it",
    );
}

/// The subject's published voice ceiling, read from the canonical state tree's
/// own parameter snapshot.
fn projected_voice_limit(
    app_loop: &AppLoop<crest_synth::adapter::lock_free_audio_boundary::LockFreeControlHandle>,
    patch_id: PatchId,
) -> Option<u32> {
    let tree: serde_json::Value =
        serde_json::from_str(app_loop.current_state_tree().json()).ok()?;
    tree.get("parameters")?
        .get("patches")?
        .as_array()?
        .iter()
        .find(|patch| {
            patch.get("patchId").and_then(serde_json::Value::as_u64)
                == Some(u64::from(patch_id.value()))
        })?
        .get("voiceLimit")?
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
}

/// The detail journey serves two subjects and refuses entry from a position
/// with no occupant to describe — asserted against the production reducer, on
/// the second Patch.
#[test]
fn detail_entry_is_refused_from_an_empty_slot_row_on_the_second_patch() {
    let mut app_loop = installed_fixture();
    let installed = installed_ids(&app_loop);
    let scene = from_installed_state(&app_loop.current_state_tree(), PatchSelectionMode::Gesture)
        .expect("the installed fixture produces a patch-editor scene");
    let round_trip = transition(&scene, "PatchEditor.detailRoundTrip");
    assert!(
        round_trip.support_before().iter().any(|item| matches!(
            item,
            LiveTopologySupport::ExpectRejected {
                event: AppEvent::EnterSurface(crest_synth::control::SurfaceId::PatchDetail),
                ..
            }
        )),
        "the journey must falsify detail entry, not only perform it",
    );

    // Reach the second Patch and focus its still-empty third slot row through
    // the production path, then require the typed refusal.
    for event in support_events(transition(&scene, "PatchEditor.switchToSubject").support_before())
    {
        app_loop
            .dispatch_from(event, EventSource::DemoScene)
            .expect("the plan's switch script is accepted");
    }
    let empty_slot = crest_synth::control::PatchControlId::EffectSlot(EffectSlotIndex::ALL[2]);
    // Walk the focused Patch's own order downward, reading focus back from the
    // canonical projection each step, until the empty position is reached.
    // Bounded so a plan that never arrives fails here rather than looping.
    let focused_control =
        |app_loop: &AppLoop<_>| match app_loop.current_semantic_model().focus_path().control_id() {
            SemanticControlId::Patch(control) => Some(control.clone()),
            _ => None,
        };
    let mut arrived = false;
    for _ in 0..64 {
        if focused_control(&app_loop).as_ref() == Some(&empty_slot) {
            arrived = true;
            break;
        }
        app_loop
            .dispatch_from(AppEvent::Navigate(Direction::Down), EventSource::DemoScene)
            .expect("walking the focused Patch's own order is accepted");
    }
    assert!(
        arrived,
        "the second Patch must host its own third occupancy row",
    );
    assert_eq!(
        app_loop.dispatch_from(
            AppEvent::EnterSurface(crest_synth::control::SurfaceId::PatchDetail),
            EventSource::DemoScene,
        ),
        Err(EventRejection::ActionUnavailableInContext),
        "an empty position has no capability to describe",
    );
    assert_eq!(focused_patch(&app_loop), Some(installed[1]));
}
