//! Sends exercise the production reducer, projection, preparation, and audio path.
use crest_synth::adapter::braids_capability::BraidsCapability;
use crest_synth::adapter::lock_free_audio_boundary::{
    LockFreeAudioBoundary, LockFreeAudioHandle, LockFreeControlHandle,
};
use crest_synth::adapter::lock_free_structural_graph_boundary::{
    LockFreeStructuralAudioHandle, LockFreeStructuralGraphBoundary,
};
use crest_synth::adapter::production_effects::{
    production_effect_preparers, production_effect_registry, production_startup_bus_returns,
};
use crest_synth::adapter::production_instruments::{
    production_capability_registry, production_instrument_preparers,
    production_instrument_providers,
};
use crest_synth::control::{
    AppEvent, AppLoop, AppState, Direction, EngineSelectionStatusKind, EventRejection,
    MixerControlId, PatchChoiceSubject, PatchControlId, SavedSession, SemanticAction,
    SemanticControlId, SemanticControlValue, SemanticGraphicalViewModel, SemanticResolver,
    SemanticSurfaceSummary, SendAction, SendControlId, StateProjector, SurfaceId, TopLevelContext,
};
use crest_synth::kernel::{MidiChannel, PatchId};
use crest_synth::mixer::bus_id::BusId;
use crest_synth::mixer::bus_return::BusReturnBank;
use crest_synth::mixer::global_parameters::GlobalParameters;
use crest_synth::mixer::mixer_track_id::MixerTrackId;
use crest_synth::mixer::patch_output::PatchOutput;
use crest_synth::real_time::audio_renderer::AudioRenderer;
use crest_synth::real_time::prepared_bus_return_rack::PreparedBusReturnRack;
use crest_synth::real_time::{
    AudioBoundary, GraphHandoffStatus, GraphRevision, PreparedGraphBuilder, RtBusReturnParameters,
    RtPostEffectParameters, StructuralGraphBoundary,
};
use crest_synth::shell::audio_output::{AudioDeviceConfig, AudioSampleFormat};
use crest_synth::shell::{KeyboardInputTranslator, WindowInput, WindowKey};
use crest_synth::synth::effect_slot_id::EffectSlotIndex;
use crest_synth::synth::{
    AssetFileId, AssetKind, DescriptorDefaultConfigFactory, EffectCapabilityId, EffectSlotId,
    FileBrowserFolderId, FileBrowserListing, FileBrowserRow, FileBrowserRowKind, ParameterKind,
    Patch, PreparedEffectError, PreparedPostEffect,
};
use crest_synth::testing::deterministic_graph_preparation_worker::{
    DeterministicGraphPreparationHandle, DeterministicGraphPreparationWorker,
};

const SAMPLE_RATE: f32 = 48_000.0;
const FRAMES: usize = 64;

fn state(returns: BusReturnBank) -> AppState {
    let patch = Patch::new(
        PatchId::new(7).unwrap(),
        "Lead".to_owned(),
        BraidsCapability::new().unwrap().default_config().unwrap(),
        MidiChannel::new(0).unwrap(),
        PatchOutput::to_track(MixerTrackId::new(3).unwrap()),
    );
    let mut state = AppState::new_with_effects(
        production_capability_registry().unwrap(),
        production_effect_registry().unwrap(),
        GlobalParameters::new(0.0).unwrap(),
    )
    .with_initial_returns(returns);
    state.apply(AppEvent::InstallPatches(vec![patch])).unwrap();
    state
        .apply(AppEvent::SelectContext(TopLevelContext::Patch))
        .unwrap();
    state
}

fn model(state: &AppState) -> SemanticGraphicalViewModel {
    StateProjector::for_graph(state.engine_selection().active_graph_revision())
        .project_with_shell(state)
        .unwrap()
        .3
        .semantic_model()
        .clone()
}

fn open(state: &mut AppState) {
    state
        .apply_semantic_action(SemanticAction::Send(SendAction::Open))
        .unwrap();
}

fn navigate_to(state: &mut AppState, control: SemanticControlId) {
    let path_count = match state.interaction().active_surface() {
        SurfaceId::Sends => SemanticResolver::new(state)
            .send_paths(state.interaction().selected_send())
            .unwrap()
            .len(),
        SurfaceId::PatchUtility => SemanticResolver::new(state)
            .patch_utility_paths(state.patches()[0].id())
            .unwrap()
            .len(),
        surface => panic!("unexpected test navigation surface: {surface:?}"),
    };
    for _ in 0..path_count {
        if state.interaction().focus_path().control_id() == &control {
            return;
        }
        state.apply(AppEvent::Navigate(Direction::Down)).unwrap();
    }
    panic!("control absent from production navigation: {control:?}");
}

#[test]
fn startup_sixteen_empty_sends_have_no_instrument_and_keyboard_selection_restores_focus() {
    let effects = production_effect_registry().unwrap();
    let bank = production_startup_bus_returns(&effects).unwrap();
    assert_eq!(bank.len(), 16);
    assert!(bank
        .returns()
        .iter()
        .all(|send| send.name() == "INIT" && send.effects().is_empty()));
    let mut state = state(bank);
    state.apply(AppEvent::Navigate(Direction::Down)).unwrap();
    let patch_focus = state.interaction().focus_path().clone();
    state
        .apply(AppEvent::SelectContext(TopLevelContext::Mixer))
        .unwrap();
    state.apply(AppEvent::Navigate(Direction::Right)).unwrap();
    let mixer_focus = state.interaction().focus_path().clone();
    let mut keyboard = KeyboardInputTranslator::new();
    let action = keyboard
        .translate(WindowInput::key_down(WindowKey::Digit4))
        .unwrap();
    assert_eq!(action, SemanticAction::Send(SendAction::Open));
    state.apply_semantic_action(action).unwrap();
    let projected = model(&state);
    let screen = projected.surface(SurfaceId::Sends).unwrap();
    assert_eq!(screen.controls().len(), 3);
    assert!(screen
        .controls()
        .iter()
        .all(|row| matches!(row.path().control_id(), SemanticControlId::Send(_))));
    assert_eq!(projected.focus_path(), state.interaction().focus_path());
    let before = state.clone();
    assert_eq!(
        state.apply_semantic_action(SemanticAction::SelectPatch(Direction::Left)),
        Err(EventRejection::ParameterAtBoundary)
    );
    assert_eq!(state, before);
    for index in 1..16 {
        let action = keyboard
            .translate(WindowInput::key_down(WindowKey::E))
            .unwrap();
        keyboard.translate(WindowInput::key_up(WindowKey::E));
        state.apply_semantic_action(action).unwrap();
        assert_eq!(state.interaction().selected_send().index(), index);
        assert!(SemanticResolver::new(&state).resolves(state.interaction().focus_path()));
    }
    let before = state.clone();
    assert_eq!(
        state.apply_semantic_action(SemanticAction::SelectPatch(Direction::Right)),
        Err(EventRejection::ParameterAtBoundary)
    );
    assert_eq!(state, before);
    state
        .apply(AppEvent::SelectContext(TopLevelContext::Patch))
        .unwrap();
    assert_eq!(state.interaction().focus_path(), &patch_focus);
    state
        .apply(AppEvent::SelectContext(TopLevelContext::Mixer))
        .unwrap();
    assert_eq!(state.interaction().focus_path(), &mixer_focus);
    open(&mut state);
    assert_eq!(state.interaction().selected_send().index(), 15);
}

#[test]
fn send_effect_choices_exactly_match_patch_registry_and_cancel_keeps_origin() {
    let mut state = state(BusReturnBank::default());
    let bus = BusId::new(0).unwrap();
    let slot_id = state.bus_returns().bus_return(bus).next_slot_id().unwrap();
    let subject = PatchChoiceSubject::new(
        state.patches()[0].id(),
        PatchControlId::EffectSlot(EffectSlotIndex::ALL[0]),
    );
    let resolver = SemanticResolver::new(&state);
    let patch_choices = resolver.choice_source(&subject).unwrap();
    let send_choices = resolver.send_choices(bus, slot_id).unwrap();
    assert_eq!(send_choices.as_slice(), patch_choices.options());
    assert_eq!(send_choices.len(), state.effects().descriptors().len() + 1);
    open(&mut state);
    navigate_to(
        &mut state,
        SemanticControlId::Send(SendControlId::EffectSlot { bus, slot_id }),
    );
    let origin = state.interaction().focus_path().clone();
    let saved = SavedSession::capture(&state);
    state.apply(AppEvent::Activate).unwrap();
    assert!(state.interaction().send_choice_origin().is_some());
    let projected = model(&state);
    assert_eq!(
        projected
            .surface(SurfaceId::Sends)
            .unwrap()
            .controls()
            .len(),
        send_choices.len()
    );
    assert_eq!(projected.focus_path(), state.interaction().focus_path());
    for row in projected
        .surface(SurfaceId::Sends)
        .unwrap()
        .controls()
        .iter()
        .filter(|row| row.enabled())
    {
        assert!(
            !row.valid_actions().is_empty(),
            "enabled picker row needs reducer-backed actions: {:?}",
            row.path()
        );
        let SemanticControlId::Send(SendControlId::Choice { entry, .. }) = row.path().control_id()
        else {
            panic!("picker row must have a stable choice identity");
        };
        let category = send_choices
            .iter()
            .find(|choice| choice.id() == entry)
            .unwrap()
            .category();
        for _ in 0..send_choices.len() * 2 {
            if state.interaction().focus_path() == row.path() {
                break;
            }
            let SemanticControlId::Send(SendControlId::Choice { entry, .. }) =
                state.interaction().focus_path().control_id()
            else {
                panic!("picker navigation stays modal");
            };
            let current_category = send_choices
                .iter()
                .find(|choice| choice.id() == entry)
                .unwrap()
                .category();
            state
                .apply(AppEvent::Navigate(if current_category == category {
                    Direction::Down
                } else {
                    Direction::Right
                }))
                .unwrap();
        }
        assert_eq!(state.interaction().focus_path(), row.path());
        assert_eq!(
            row.valid_actions(),
            SemanticResolver::new(&state).valid_actions()
        );
        assert_eq!(
            model(&state).focused_control().unwrap().valid_actions(),
            row.valid_actions()
        );
    }
    state.apply(AppEvent::Return).unwrap();
    assert_eq!(state.interaction().focus_path(), &origin);
    assert_eq!(SavedSession::capture(&state), saved);
}

#[test]
fn send_convolution_asset_browser_navigation_and_cancel_restore_stable_origin() {
    let effects = production_effect_registry().unwrap();
    let capability = EffectCapabilityId::new("effect.fft.convolver").unwrap();
    let descriptor = effects
        .descriptor(&capability)
        .expect("production ConvolutionReverb is registered");
    assert!(descriptor.availability().is_enabled());
    let parameter = descriptor
        .parameters()
        .find(|spec| spec.kind() == ParameterKind::Asset)
        .expect("ConvolutionReverb exposes its impulse response asset")
        .id()
        .clone();
    let bus = BusId::new(2).unwrap();
    let mut returns = BusReturnBank::default();
    let slot_id = returns.bus_return(bus).next_slot_id().unwrap();
    returns
        .set_effect_slot(&effects, bus, slot_id, Some(&capability))
        .unwrap();
    let mut state = state(returns);
    open(&mut state);
    for _ in 0..bus.index() {
        state
            .apply(AppEvent::SelectPatch(Direction::Right))
            .unwrap();
    }
    navigate_to(
        &mut state,
        SemanticControlId::Send(SendControlId::EffectParameter {
            bus,
            slot_id,
            parameter,
        }),
    );
    let origin = state.interaction().focus_path().clone();
    let saved = SavedSession::capture(&state);
    assert_eq!(model(&state).focus_path(), &origin);
    state
        .apply_semantic_action(SemanticAction::OpenRelated)
        .unwrap();
    assert_eq!(state.interaction().active_surface(), SurfaceId::FileBrowser);
    assert_eq!(state.file_browser().origin(), Some(&origin));
    assert_eq!(
        state.file_browser().asset_kind(),
        AssetKind::ImpulseResponse
    );
    assert_eq!(model(&state).focus_path(), state.interaction().focus_path());
    let folder = FileBrowserFolderId::default();
    state
        .apply(AppEvent::FileCatalogRefreshed {
            asset_kind: AssetKind::ImpulseResponse,
            folder: folder.clone(),
            listing: Ok(FileBrowserListing::new(
                folder,
                vec![
                    FileBrowserRow::new(
                        "room",
                        "Room.wav",
                        FileBrowserRowKind::File(AssetFileId::new("Room.wav").unwrap()),
                        Some(128),
                    )
                    .unwrap(),
                    FileBrowserRow::new("cancel", "Cancel", FileBrowserRowKind::Cancel, None)
                        .unwrap(),
                ],
            )
            .unwrap()),
        })
        .unwrap();
    let first = state.interaction().focus_path().clone();
    assert_eq!(model(&state).focus_path(), &first);
    state.apply(AppEvent::Navigate(Direction::Down)).unwrap();
    assert_ne!(state.interaction().focus_path(), &first);
    assert_eq!(model(&state).focus_path(), state.interaction().focus_path());
    state.apply(AppEvent::Activate).unwrap();
    assert_eq!(state.interaction().active_surface(), SurfaceId::Sends);
    assert_eq!(state.interaction().focus_path(), &origin);
    assert_eq!(model(&state).focus_path(), &origin);
    assert_eq!(SavedSession::capture(&state), saved);
    assert!(state.file_browser().import_request().is_none());
}

#[test]
fn patch_utility_hides_empty_chains_and_edits_independent_patch_sends() {
    let effects = production_effect_registry().unwrap();
    let mut bank = BusReturnBank::default();
    let bus = BusId::new(5).unwrap();
    let entry = EffectCapabilityId::new("effect.chorus").unwrap();
    let slot = bank.bus_return(bus).next_slot_id().unwrap();
    bank.set_effect_slot(&effects, bus, slot, Some(&entry))
        .unwrap();
    bank.set_name(bus, "Wide room").unwrap();
    let seed = state(bank);
    let first = seed.patches()[0].clone();
    let second = Patch::new(
        PatchId::new(8).unwrap(),
        "Second".to_owned(),
        first.instrument_config().clone(),
        first.channel(),
        first.output(),
    );
    let mut state = AppState::new_with_effects(
        seed.capabilities().clone(),
        seed.effects().clone(),
        *seed.global(),
    )
    .with_initial_returns(seed.bus_returns().clone());
    state
        .apply(AppEvent::InstallPatches(vec![
            first.clone(),
            second.clone(),
        ]))
        .unwrap();
    state
        .apply(AppEvent::SelectContext(TopLevelContext::Patch))
        .unwrap();
    let second = state.patches()[1].clone();
    assert!(state
        .patches()
        .iter()
        .all(|patch| patch.sends().iter().all(|send| *send == 0.0)));
    let mixer = state.mixer().clone();
    let projected = model(&state);
    let rows = projected
        .surface(SurfaceId::PatchUtility)
        .unwrap()
        .controls()
        .iter()
        .filter(|row| {
            matches!(
                row.path().control_id(),
                SemanticControlId::Patch(PatchControlId::Send(_))
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].path().control_id(),
        &SemanticControlId::Patch(PatchControlId::Send(bus))
    );
    assert!(rows[0].label().contains("Wide room"));
    assert_eq!(rows[0].value(), &SemanticControlValue::Scalar(0.0));
    state
        .apply(AppEvent::EnterSurface(SurfaceId::PatchUtility))
        .unwrap();
    navigate_to(
        &mut state,
        SemanticControlId::Patch(PatchControlId::Send(bus)),
    );
    state.apply(AppEvent::Adjust(Direction::Right)).unwrap();
    let send = state.patches()[0].send(bus);
    assert!(send > 0.0);
    assert_eq!(&state.patches()[1], &second);
    assert_eq!(state.mixer(), &mixer);
    let (_, _, _, shell, parameters) = StateProjector::new().project_with_shell(&state).unwrap();
    assert_eq!(
        parameters.patch(first.id()).unwrap().sends()[bus.index()],
        send
    );
    assert_eq!(
        parameters.patch(second.id()).unwrap().sends()[bus.index()],
        0.0
    );
    assert_eq!(
        shell.semantic_model().focused_control().unwrap().value(),
        &SemanticControlValue::Scalar(f64::from(send))
    );
    state.apply(AppEvent::Return).unwrap();
    state
        .apply(AppEvent::SelectPatch(Direction::Right))
        .unwrap();
    state
        .apply(AppEvent::EnterSurface(SurfaceId::PatchUtility))
        .unwrap();
    navigate_to(
        &mut state,
        SemanticControlId::Patch(PatchControlId::Send(bus)),
    );
    assert_eq!(
        model(&state).focused_control().unwrap().value(),
        &SemanticControlValue::Scalar(0.0)
    );
    state.apply(AppEvent::Adjust(Direction::Up)).unwrap();
    let second_send = state.patches()[1].send(bus);
    assert!(second_send > send);
    assert_eq!(state.patches()[0].send(bus), send);
    assert_eq!(state.patches()[0].output(), state.patches()[1].output());
    assert_eq!(state.mixer(), &mixer);
    let parameters = StateProjector::new().parameter_snapshot(&state).unwrap();
    assert_eq!(
        parameters.patch(first.id()).unwrap().sends()[bus.index()],
        send
    );
    assert_eq!(
        parameters.patch(second.id()).unwrap().sends()[bus.index()],
        second_send
    );
}

#[test]
fn keyboard_name_commit_cancel_and_restore_preserve_send_identity() {
    let mut state = state(BusReturnBank::default());
    let bus = BusId::new(0).unwrap();
    open(&mut state);
    let mut keyboard = KeyboardInputTranslator::new();
    state
        .apply_semantic_action(
            keyboard
                .translate(WindowInput::key_down(WindowKey::K))
                .unwrap(),
        )
        .unwrap();
    let adjusting = state.clone();
    assert_eq!(
        state.apply(AppEvent::Activate),
        Err(EventRejection::ActionUnavailableInContext)
    );
    assert_eq!(state, adjusting);
    assert!(!state.interaction().send_name_editing());
    state
        .apply_semantic_action(
            keyboard
                .translate(WindowInput::key_up(WindowKey::K))
                .unwrap(),
        )
        .unwrap();
    state.apply(AppEvent::Activate).unwrap();
    let before = state.clone();
    assert_eq!(
        state.apply_semantic_action(SemanticAction::Send(SendAction::Rename {
            bus,
            name: "\n".to_owned()
        })),
        Err(EventRejection::InvalidParameterValue)
    );
    assert_eq!(state, before);
    state
        .apply_semantic_action(SemanticAction::Send(SendAction::CancelRename))
        .unwrap();
    assert_eq!(state.bus_returns().bus_return(bus).name(), "INIT");
    assert!(!state.interaction().send_name_editing());
    state.apply(AppEvent::Activate).unwrap();
    state
        .apply_semantic_action(SemanticAction::Send(SendAction::Rename {
            bus,
            name: "  Long room α  ".to_owned(),
        }))
        .unwrap();
    assert_eq!(state.bus_returns().bus_return(bus).name(), "Long room α");
    assert!(!state.interaction().send_name_editing());
    let saved = SavedSession::capture(&state);
    let decoded = SavedSession::from_json(&saved.to_json().unwrap(), state.capabilities()).unwrap();
    assert_eq!(decoded, saved);
    let restored = decoded
        .prepare_restore(
            state.capabilities().clone(),
            state.effects().clone(),
            &production_instrument_preparers().unwrap(),
            &production_effect_preparers().unwrap(),
            GraphRevision::INITIAL.checked_next().unwrap(),
            SAMPLE_RATE,
            FRAMES,
        )
        .unwrap();
    assert_eq!(restored.state().bus_returns(), state.bus_returns());
    assert_eq!(SavedSession::capture(restored.state()), saved);
}

struct Fixture {
    app: AppLoop<LockFreeControlHandle>,
    renderer: AudioRenderer<LockFreeAudioHandle, LockFreeStructuralAudioHandle>,
    worker: DeterministicGraphPreparationHandle,
}

impl Fixture {
    fn new(state: AppState) -> Self {
        let registry = state.capabilities().clone();
        let effects = state.effects().clone();
        let parameters = StateProjector::new().project(&state).unwrap().2;
        let (control, audio) = LockFreeAudioBoundary::new(32, parameters).into_handles();
        let mut app = AppLoop::new(state, StateProjector::new(), control).unwrap();
        let preparers = production_instrument_preparers().unwrap();
        let effect_preparers = production_effect_preparers().unwrap();
        let graph = PreparedGraphBuilder::new(&registry, &preparers)
            .with_effects(&effects, &effect_preparers)
            .with_returns(app.bus_returns())
            .build(
                GraphRevision::INITIAL,
                app.patches(),
                app.current_parameters().clone(),
                SAMPLE_RATE,
                FRAMES,
            )
            .unwrap();
        let (structural_control, structural_audio) = LockFreeStructuralGraphBoundary::new(
            1,
            1,
            GraphHandoffStatus::with_active(GraphRevision::INITIAL),
        )
        .unwrap()
        .into_handles();
        let audio_config =
            AudioDeviceConfig::new(SAMPLE_RATE, 2, AudioSampleFormat::F32, FRAMES).unwrap();
        let worker = DeterministicGraphPreparationWorker::new_with_effects(
            registry.clone(),
            production_instrument_preparers().unwrap(),
            effects,
            production_effect_preparers().unwrap(),
            audio_config,
        );
        let worker_handle = worker.advance_handle();
        app.configure_engine_selection(
            DescriptorDefaultConfigFactory::new(
                registry,
                production_instrument_providers().unwrap(),
            ),
            worker,
            structural_control,
            &graph,
            audio_config,
        )
        .unwrap();
        Self {
            app,
            renderer: AudioRenderer::new(audio, structural_audio, graph),
            worker: worker_handle,
        }
    }

    fn choose_effect(&mut self, bus: BusId, slot_id: EffectSlotId, entry: &str) {
        let target = SemanticControlId::Send(SendControlId::EffectSlot { bus, slot_id });
        let count = self
            .app
            .current_semantic_model()
            .surface(SurfaceId::Sends)
            .unwrap()
            .controls()
            .len();
        for _ in 0..count {
            if self.app.current_semantic_model().focus_path().control_id() == &target {
                break;
            }
            self.app
                .dispatch_action(SemanticAction::Navigate(Direction::Down))
                .unwrap();
        }
        assert_eq!(
            self.app.current_semantic_model().focus_path().control_id(),
            &target
        );
        self.app.dispatch_action(SemanticAction::Activate).unwrap();
        let target = SemanticControlId::Send(SendControlId::Choice {
            bus,
            slot_id,
            entry: entry.to_owned(),
        });
        let count = self
            .app
            .current_semantic_model()
            .surface(SurfaceId::Sends)
            .unwrap()
            .controls()
            .len();
        for _ in 0..count * 2 {
            let model = self.app.current_semantic_model();
            if model.focus_path().control_id() == &target {
                self.app.dispatch_action(SemanticAction::Activate).unwrap();
                return;
            }
            let target_visible = model
                .surface(SurfaceId::Sends)
                .unwrap()
                .controls()
                .iter()
                .find(|row| row.path().control_id() == &target)
                .expect("registered choice projects")
                .visible();
            self.app
                .dispatch_action(SemanticAction::Navigate(if target_visible {
                    Direction::Down
                } else {
                    Direction::Right
                }))
                .unwrap();
        }
        panic!("registered effect is unreachable through send picker: {entry}");
    }

    fn assert_pending_send_projection(&self) {
        let status = self.app.engine_selection_status();
        let crest_synth::control::StructuralEditIntent::SetSendEffect {
            bus,
            slot_id,
            entry,
        } = status.correlation().unwrap().intent()
        else {
            panic!("this fixture prepares send effect edits");
        };
        let target = SemanticControlId::Send(SendControlId::EffectSlot {
            bus: *bus,
            slot_id: *slot_id,
        });
        let expected_requested = entry
            .as_ref()
            .map(|entry| self.app.effects().descriptor(entry).unwrap().label())
            .unwrap_or("Empty");
        let expected_active = self
            .app
            .bus_returns()
            .bus_return(*bus)
            .effect_at(*slot_id)
            .map(|config| {
                self.app
                    .effects()
                    .descriptor(config.capability_id())
                    .unwrap()
                    .label()
            })
            .unwrap_or("EMPTY");
        let projected = self.app.current_semantic_model();
        let rows = projected.surface(SurfaceId::Sends).unwrap().controls();
        let row = rows
            .iter()
            .find(|row| row.path().control_id() == &target)
            .unwrap();
        assert_eq!(
            row.value(),
            &SemanticControlValue::Identity(expected_active.to_owned())
        );
        assert_eq!(
            row.requested_value(),
            Some(&SemanticControlValue::Identity(
                expected_requested.to_owned()
            ))
        );
        assert_eq!(row.status().unwrap().kind(), status.kind());
        assert_eq!(
            rows.iter()
                .filter(|row| row.requested_value().is_some())
                .count(),
            1
        );
    }

    fn activate_pending(&mut self) {
        let before = self.app.bus_returns().clone();
        let source = self.renderer.active_revision();
        self.assert_pending_send_projection();
        for expected in [
            EngineSelectionStatusKind::Validating,
            EngineSelectionStatusKind::Preparing,
        ] {
            assert_eq!(
                self.app
                    .advance_structural()
                    .unwrap()
                    .engine_selection_lifecycle_advanced(),
                Some(expected)
            );
            assert_eq!(self.app.bus_returns(), &before);
            self.assert_pending_send_projection();
        }
        assert!(self.worker.advance());
        assert!(self
            .app
            .advance_structural()
            .unwrap()
            .graph_stage()
            .is_some());
        assert_eq!(self.app.bus_returns(), &before);
        assert_eq!(self.renderer.active_revision(), source);
        self.assert_pending_send_projection();
        let mut output = [0.0; FRAMES * 2];
        self.renderer.render(&mut output);
        assert!(output.iter().all(|value| value.is_finite()));
        assert_eq!(
            self.renderer.active_revision(),
            source.checked_next().unwrap()
        );
        assert!(self
            .app
            .advance_structural()
            .unwrap()
            .activation_acknowledged()
            .is_some());
        assert_eq!(
            self.app.engine_selection_status().kind(),
            EngineSelectionStatusKind::Ready
        );
        assert_eq!(
            self.app.current_parameters().graph_revision(),
            self.renderer.active_revision()
        );
    }
}

#[test]
fn ordered_chain_commits_only_after_worker_activation_and_busy_or_invalid_edits_are_atomic() {
    let mut fixture = Fixture::new(state(BusReturnBank::default()));
    let bus = BusId::new(0).unwrap();
    fixture
        .app
        .dispatch_action(SemanticAction::Send(SendAction::Open))
        .unwrap();
    let invalid_before = fixture.app.capture_saved_session();
    let invalid_slot = fixture
        .app
        .bus_returns()
        .bus_return(bus)
        .next_slot_id()
        .unwrap();
    assert!(fixture
        .app
        .dispatch_action(SemanticAction::Send(SendAction::SetEffect {
            bus,
            slot_id: invalid_slot,
            entry: Some(EffectCapabilityId::new("effect.not-installed").unwrap()),
        }))
        .is_err());
    assert_eq!(fixture.app.capture_saved_session(), invalid_before);
    assert_eq!(fixture.renderer.active_revision(), GraphRevision::INITIAL);
    let mut slots = Vec::new();
    for name in ["effect.chorus", "effect.delay"] {
        let slot_id = fixture
            .app
            .bus_returns()
            .bus_return(bus)
            .next_slot_id()
            .unwrap();
        slots.push(slot_id);
        let before = fixture.app.capture_saved_session();
        fixture.choose_effect(bus, slot_id, name);
        assert_eq!(fixture.app.capture_saved_session(), before);
        assert_eq!(
            fixture.app.engine_selection_status().kind(),
            EngineSelectionStatusKind::Loading
        );
        assert_eq!(
            fixture
                .app
                .dispatch_action(SemanticAction::Send(SendAction::SetEffect {
                    bus,
                    slot_id,
                    entry: Some(EffectCapabilityId::new("effect.reverb").unwrap()),
                })),
            Err(EventRejection::StructuralEditBusy)
        );
        assert_eq!(fixture.app.capture_saved_session(), before);
        fixture.activate_pending();
    }
    let chain = fixture.app.bus_returns().bus_return(bus).effects();
    assert_eq!(
        chain
            .iter()
            .map(|effect| effect.slot_id())
            .collect::<Vec<_>>(),
        slots
    );
    assert_eq!(
        chain
            .iter()
            .map(|effect| effect.capability_id().as_str())
            .collect::<Vec<_>>(),
        ["effect.chorus", "effect.delay"]
    );
    assert_eq!(
        fixture
            .app
            .current_parameters()
            .bus_return(bus)
            .effect_count(),
        2
    );
    assert_eq!(
        fixture.renderer.parameters().bus_return(bus).effect_count(),
        2
    );
    let projected = fixture.app.current_semantic_model();
    assert_eq!(
        projected
            .surface(SurfaceId::Sends)
            .unwrap()
            .sections()
            .len(),
        4
    );
    fixture
        .app
        .dispatch_action(SemanticAction::Send(SendAction::SetEffect {
            bus,
            slot_id: slots[0],
            entry: None,
        }))
        .unwrap();
    fixture.activate_pending();
    assert_eq!(
        fixture.app.bus_returns().bus_return(bus).effects()[0].slot_id(),
        slots[1]
    );
    assert_eq!(
        fixture
            .app
            .current_parameters()
            .bus_return(bus)
            .effect_count(),
        1
    );
    fixture
        .app
        .dispatch_action(SemanticAction::Send(SendAction::SetEffect {
            bus,
            slot_id: slots[1],
            entry: None,
        }))
        .unwrap();
    fixture.activate_pending();
    assert!(!fixture.app.current_parameters().bus_return(bus).is_active());
    fixture
        .app
        .dispatch_action(SemanticAction::SelectContext(TopLevelContext::Patch))
        .unwrap();
    assert!(fixture
        .app
        .current_semantic_model()
        .surface(SurfaceId::PatchUtility)
        .unwrap()
        .controls()
        .iter()
        .all(|row| !matches!(
            row.path().control_id(),
            SemanticControlId::Patch(PatchControlId::Send(_))
        )));
}

#[test]
fn more_than_sixteen_sends_project_prepare_and_render_without_default_count_cap() {
    let count = 19;
    let bus = BusId::new(18).unwrap();
    let effects = production_effect_registry().unwrap();
    let mut bank = BusReturnBank::with_count(count).unwrap();
    let slot_id = bank.bus_return(bus).next_slot_id().unwrap();
    bank.set_effect_slot(
        &effects,
        bus,
        slot_id,
        Some(&EffectCapabilityId::new("effect.chorus").unwrap()),
    )
    .unwrap();
    let mut state = state(bank);
    open(&mut state);
    for _ in 1..count {
        state
            .apply(AppEvent::SelectPatch(Direction::Right))
            .unwrap();
    }
    let projected = model(&state);
    assert!(
        matches!(projected.surface(SurfaceId::Sends).unwrap().summary(), SemanticSurfaceSummary::Sends { bus: selected, count: size, .. } if *selected == bus && *size == count)
    );
    let mut fixture = Fixture::new(state);
    assert_eq!(fixture.app.current_parameters().returns().len(), count);
    assert_eq!(
        fixture
            .app
            .current_parameters()
            .bus_return(bus)
            .effect_count(),
        1
    );
    assert!(fixture
        .app
        .current_parameters()
        .mixer_tracks()
        .iter()
        .all(|track| track.sends().len() == count));
    let mut output = [0.0; FRAMES * 2];
    fixture.renderer.render(&mut output);
    assert!(output.iter().all(|sample| sample.is_finite()));
    assert_eq!(fixture.renderer.parameters().returns().len(), count);
}

#[test]
fn async_clear_repairs_patch_utility_focus_after_leaving_sends_during_preparation() {
    let effects = production_effect_registry().unwrap();
    let bus = BusId::new(4).unwrap();
    let mut returns = BusReturnBank::default();
    let slot_id = returns.bus_return(bus).next_slot_id().unwrap();
    returns
        .set_effect_slot(
            &effects,
            bus,
            slot_id,
            Some(&EffectCapabilityId::new("effect.chorus").unwrap()),
        )
        .unwrap();
    let mut fixture = Fixture::new(state(returns));
    fixture
        .app
        .dispatch_action(SemanticAction::Send(SendAction::Open))
        .unwrap();
    fixture
        .app
        .dispatch_action(SemanticAction::Send(SendAction::SetEffect {
            bus,
            slot_id,
            entry: None,
        }))
        .unwrap();
    for expected in [
        EngineSelectionStatusKind::Validating,
        EngineSelectionStatusKind::Preparing,
    ] {
        assert_eq!(
            fixture
                .app
                .advance_structural()
                .unwrap()
                .engine_selection_lifecycle_advanced(),
            Some(expected)
        );
    }
    fixture
        .app
        .dispatch_action(SemanticAction::SelectContext(TopLevelContext::Patch))
        .unwrap();
    fixture
        .app
        .dispatch_action(SemanticAction::EnterSurface(SurfaceId::PatchUtility))
        .unwrap();
    let target = SemanticControlId::Patch(PatchControlId::Send(bus));
    let count = fixture
        .app
        .current_semantic_model()
        .surface(SurfaceId::PatchUtility)
        .unwrap()
        .controls()
        .len();
    for _ in 0..count {
        if fixture
            .app
            .current_semantic_model()
            .focus_path()
            .control_id()
            == &target
        {
            break;
        }
        fixture
            .app
            .dispatch_action(SemanticAction::Navigate(Direction::Down))
            .unwrap();
    }
    assert_eq!(
        fixture
            .app
            .current_semantic_model()
            .focus_path()
            .control_id(),
        &target
    );
    assert!(fixture.worker.advance());
    assert!(fixture
        .app
        .advance_structural()
        .unwrap()
        .graph_stage()
        .is_some());
    assert_eq!(
        fixture
            .app
            .current_semantic_model()
            .focus_path()
            .control_id(),
        &target
    );
    fixture.renderer.render(&mut [0.0; FRAMES * 2]);
    assert!(fixture
        .app
        .advance_structural()
        .unwrap()
        .activation_acknowledged()
        .is_some());
    assert!(!fixture.app.bus_returns().bus_return(bus).is_occupied());
    let model = fixture.app.current_semantic_model();
    assert_eq!(model.focus_path().surface(), SurfaceId::PatchUtility);
    assert_ne!(model.focus_path().control_id(), &target);
    assert!(model.focused_control().is_some());
    assert!(model
        .surface(SurfaceId::PatchUtility)
        .unwrap()
        .controls()
        .iter()
        .all(|row| row.path().control_id() != &target));
    fixture
        .app
        .dispatch_action(SemanticAction::Navigate(Direction::Up))
        .unwrap();
    assert!(fixture
        .app
        .current_semantic_model()
        .focused_control()
        .is_some());
}

#[test]
fn async_first_send_effect_edit_repairs_old_mixer_inspector_parameter_focus() {
    assert_async_first_send_effect_repairs_inspector(false);
}

#[test]
fn async_first_send_effect_edit_repairs_mixer_inspector_focus_suspended_under_settings() {
    assert_async_first_send_effect_repairs_inspector(true);
}

fn assert_async_first_send_effect_repairs_inspector(suspend_in_settings: bool) {
    for replacement in [None, Some("effect.reverb")] {
        let effects = production_effect_registry().unwrap();
        let chorus = EffectCapabilityId::new("effect.chorus").unwrap();
        let parameter = effects
            .descriptor(&chorus)
            .unwrap()
            .parameters()
            .next()
            .unwrap()
            .id()
            .clone();
        let bus = BusId::new(4).unwrap();
        let mut returns = BusReturnBank::default();
        let first = returns.bus_return(bus).next_slot_id().unwrap();
        returns
            .set_effect_slot(&effects, bus, first, Some(&chorus))
            .unwrap();
        let second = returns.bus_return(bus).next_slot_id().unwrap();
        returns
            .set_effect_slot(
                &effects,
                bus,
                second,
                Some(&EffectCapabilityId::new("effect.delay").unwrap()),
            )
            .unwrap();
        let mut fixture = Fixture::new(state(returns));
        fixture
            .app
            .dispatch_action(SemanticAction::Send(SendAction::Open))
            .unwrap();
        fixture
            .app
            .dispatch_action(SemanticAction::Send(SendAction::SetEffect {
                bus,
                slot_id: first,
                entry: replacement.map(|entry| EffectCapabilityId::new(entry).unwrap()),
            }))
            .unwrap();
        for expected in [
            EngineSelectionStatusKind::Validating,
            EngineSelectionStatusKind::Preparing,
        ] {
            assert_eq!(
                fixture
                    .app
                    .advance_structural()
                    .unwrap()
                    .engine_selection_lifecycle_advanced(),
                Some(expected)
            );
        }
        fixture
            .app
            .dispatch_action(SemanticAction::SelectContext(TopLevelContext::Mixer))
            .unwrap();
        fixture
            .app
            .dispatch_action(SemanticAction::EnterSurface(SurfaceId::MixerInspector))
            .unwrap();
        let target = SemanticControlId::Mixer(MixerControlId::ReturnEffect { bus, parameter });
        let count = fixture
            .app
            .current_semantic_model()
            .surface(SurfaceId::MixerInspector)
            .unwrap()
            .controls()
            .len();
        for _ in 0..count {
            if fixture
                .app
                .current_semantic_model()
                .focus_path()
                .control_id()
                == &target
            {
                break;
            }
            fixture
                .app
                .dispatch_action(SemanticAction::Navigate(Direction::Down))
                .unwrap();
        }
        assert_eq!(
            fixture
                .app
                .current_semantic_model()
                .focus_path()
                .control_id(),
            &target
        );
        let settings_focus = suspend_in_settings.then(|| {
            fixture
                .app
                .dispatch_action(SemanticAction::OpenMidiSettings)
                .unwrap();
            let model = fixture.app.current_semantic_model();
            assert_eq!(model.active_surface(), SurfaceId::MidiDeviceSettings);
            model.focus_path().clone()
        });
        assert!(fixture.worker.advance());
        assert!(fixture
            .app
            .advance_structural()
            .unwrap()
            .graph_stage()
            .is_some());
        if let Some(settings_focus) = &settings_focus {
            assert_eq!(
                fixture.app.current_semantic_model().focus_path(),
                settings_focus
            );
        } else {
            assert_eq!(
                fixture
                    .app
                    .current_semantic_model()
                    .focus_path()
                    .control_id(),
                &target
            );
        }
        fixture.renderer.render(&mut [0.0; FRAMES * 2]);
        assert!(fixture
            .app
            .advance_structural()
            .unwrap()
            .activation_acknowledged()
            .is_some());
        let send = fixture.app.bus_returns().bus_return(bus);
        assert_eq!(
            send.effects().len(),
            if replacement.is_some() { 2 } else { 1 }
        );
        assert_eq!(send.effects().last().unwrap().slot_id(), second);
        if let Some(settings_focus) = &settings_focus {
            assert_eq!(
                fixture.app.current_semantic_model().focus_path(),
                settings_focus
            );
            fixture.app.dispatch_action(SemanticAction::Return).unwrap();
        }
        let model = fixture.app.current_semantic_model();
        assert_eq!(model.focus_path().surface(), SurfaceId::MixerInspector);
        assert_ne!(model.focus_path().control_id(), &target);
        assert!(model.focused_control().is_some());
        assert!(model
            .surface(SurfaceId::MixerInspector)
            .unwrap()
            .controls()
            .iter()
            .all(|row| row.path().control_id() != &target));
        fixture
            .app
            .dispatch_action(SemanticAction::Navigate(Direction::Up))
            .unwrap();
        assert!(fixture
            .app
            .current_semantic_model()
            .focused_control()
            .is_some());
    }
}

/// Noncommuting, zero-preserving processors distinguish serial order exactly.
struct ArithmeticEffect {
    slot_id: EffectSlotId,
    square: bool,
}

impl PreparedPostEffect for ArithmeticEffect {
    fn patch_id(&self) -> PatchId {
        PatchId::new(1).unwrap()
    }
    fn slot_id(&self) -> EffectSlotId {
        self.slot_id
    }
    fn process(
        &mut self,
        samples: &mut [f32],
        _frames: usize,
        _parameters: &RtPostEffectParameters,
    ) -> Result<(), PreparedEffectError> {
        for sample in samples {
            *sample = if self.square {
                *sample * *sample
            } else {
                *sample * 2.0
            };
        }
        Ok(())
    }
}

#[test]
fn prepared_send_chain_processes_in_order_once_and_empty_send_contributes_exact_silence() {
    let bus = BusId::new(0).unwrap();
    let empty = BusId::new(1).unwrap();
    let input = [0.25; 8];
    let mut results = Vec::new();
    for order in [[false, true], [true, false]] {
        let mut rack = PreparedBusReturnRack::new(4).unwrap();
        let mut parameters = Vec::new();
        for (index, square) in order.into_iter().enumerate() {
            let slot_id = EffectSlotId::new(index as u16 + 1).unwrap();
            let values = RtPostEffectParameters::new(slot_id, &[]).unwrap();
            rack.append(
                bus,
                Box::new(ArithmeticEffect { slot_id, square }),
                values.clone(),
                0.5,
            )
            .unwrap();
            parameters.push(values);
        }
        let live = RtBusReturnParameters::chain(parameters, 0.5).unwrap();
        let mut output = [0.125; 8];
        assert!(rack.process_return(bus, &input, &mut output, &live) > 0.0);
        results.push(output);
        let before = output;
        assert_eq!(
            rack.process_return(empty, &input, &mut output, &RtBusReturnParameters::EMPTY),
            0.0
        );
        assert_eq!(output, before);
        let mut silent = [0.0; 8];
        assert_eq!(rack.process_return(bus, &[0.0; 8], &mut silent, &live), 0.0);
        assert_eq!(silent, [0.0; 8]);
    }
    assert_eq!(results[0], [0.25; 8]);
    assert_eq!(results[1], [0.1875; 8]);
}
