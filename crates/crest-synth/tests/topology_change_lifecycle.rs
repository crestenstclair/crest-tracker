//! WP06 proofs: slot and return occupancy changes travel the one correlated
//! structural lifecycle — validated refusal (FR-013), prepared whole-graph
//! exchange (FR-012), observable outcome (FR-014), recovery (FR-015),
//! acknowledgement ordering (C-RT-13), chain-follows-Patch rerouting
//! (FR-018), and off-callback retirement at the widened size (FR-016,
//! NFR-006, C-RT-14). Every proof runs the production reducer, worker
//! request path, coordinator, and render path.

use crest_synth::adapter::atomic_audio_observation::AtomicAudioObservation;
use crest_synth::adapter::lock_free_audio_boundary::LockFreeAudioBoundary;
use crest_synth::adapter::lock_free_structural_graph_boundary::LockFreeStructuralGraphBoundary;
use crest_synth::adapter::production_effects::{
    production_default_bus_returns, production_effect_preparers, production_effect_registry,
};
use crest_synth::adapter::production_instruments::{
    production_capability_registry, production_instrument_preparers,
    production_instrument_providers,
};
use crest_synth::control::event_record::{EmittedEvent, EventSource};
use crest_synth::control::{
    AppEvent, AppLoop, AppState, Direction, EngineSelectionEffectKind, EngineSelectionFailure,
    EngineSelectionRequestId, EngineSelectionStatusKind, EventRejection, PatchControlId,
    SemanticAction, StateProjector, StructuralEditIntent, TopLevelContext,
};
use crest_synth::kernel::midi_channel::MidiChannel;
use crest_synth::kernel::midi_message::{MidiMessage, MidiMessageKind};
use crest_synth::kernel::patch_id::PatchId;
use crest_synth::mixer::bus_id::BusId;
use crest_synth::mixer::global_parameters::GlobalParameters;
use crest_synth::mixer::mixer_state::MixerState;
use crest_synth::mixer::mixer_track_id::MixerTrackId;
use crest_synth::mixer::mixer_track_parameters::MixerTrackParameters;
use crest_synth::mixer::patch_output::PatchOutput;
use crest_synth::mixer::patch_output::PatchOutputParameter;
use crest_synth::real_time::{
    AudioBoundary, AudioObservation, AudioRenderer, ControlAudioObservation, GraphHandoffStatus,
    GraphRevision, ParameterSnapshot, PreparedGraphBuilder, StructuralGraphBoundary,
};
use crest_synth::shell::audio_output::{AudioDeviceConfig, AudioSampleFormat};
use crest_synth::synth::effect_slot_id::{EffectSlotIndex, MAX_EFFECT_SLOTS};
use crest_synth::synth::sound_font_instrument::SoundFontInstrument;
use crest_synth::synth::{DescriptorDefaultConfigFactory, EffectCapabilityId, Patch};
use crest_synth::testing::automatic_midi_test::create_soundfont_config;
use crest_synth::testing::{
    DeterministicGraphPreparationHandle, DeterministicGraphPreparationWorker,
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

const SAMPLE_RATE: f32 = 48_000.0;
const FRAME_COUNT: usize = 2_048;
const SAMPLE_COUNT: usize = FRAME_COUNT * 2;

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
        record_callback_allocation();
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record_callback_allocation();
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        record_callback_deallocation();
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record_callback_allocation();
        record_callback_deallocation();
        unsafe { System.realloc(pointer, layout, size) }
    }
}

fn record_callback_allocation() {
    COUNT_CALLBACK_MEMORY.with(|enabled| {
        if enabled.get() {
            CALLBACK_ALLOCATIONS.with(|count| count.set(count.get().saturating_add(1)));
        }
    });
}

fn record_callback_deallocation() {
    COUNT_CALLBACK_MEMORY.with(|enabled| {
        if enabled.get() {
            CALLBACK_DEALLOCATIONS.with(|count| count.set(count.get().saturating_add(1)));
        }
    });
}

type FixtureRenderer = AudioRenderer<
    crest_synth::adapter::lock_free_audio_boundary::LockFreeAudioHandle,
    crest_synth::adapter::lock_free_structural_graph_boundary::LockFreeStructuralAudioHandle,
    crest_synth::adapter::atomic_audio_observation::AtomicAudioObservationWriter,
>;

struct Fixture {
    app_loop: AppLoop<crest_synth::adapter::lock_free_audio_boundary::LockFreeControlHandle>,
    renderer: FixtureRenderer,
    worker: DeterministicGraphPreparationHandle,
    observation: crest_synth::adapter::atomic_audio_observation::AtomicAudioObservationReader,
}

impl Fixture {
    /// Drives a fresh reducer-owned structural request through admission and
    /// submits it to the deterministic preparation worker.
    fn admit_pending_change(&mut self) {
        for expected in [
            EngineSelectionStatusKind::Validating,
            EngineSelectionStatusKind::Preparing,
        ] {
            let admission = self.app_loop.advance_structural().unwrap();
            assert_eq!(
                admission.engine_selection_lifecycle_advanced(),
                Some(expected)
            );
        }
    }

    /// Renders one block while counting allocator activity on this thread.
    fn counted_render(&mut self, output: &mut [f32]) -> (usize, usize) {
        CALLBACK_ALLOCATIONS.with(|count| count.set(0));
        CALLBACK_DEALLOCATIONS.with(|count| count.set(0));
        COUNT_CALLBACK_MEMORY.with(|enabled| enabled.set(true));
        self.renderer.render(output);
        COUNT_CALLBACK_MEMORY.with(|enabled| enabled.set(false));
        (
            CALLBACK_ALLOCATIONS.with(Cell::get),
            CALLBACK_DEALLOCATIONS.with(Cell::get),
        )
    }

    fn note_on(&mut self, patch_id: PatchId, channel: MidiChannel) {
        self.app_loop
            .dispatch_from(
                AppEvent::Midi {
                    patch_id,
                    message: MidiMessage::try_new(channel, MidiMessageKind::NoteOn, 64, 112)
                        .unwrap(),
                },
                EventSource::System,
            )
            .unwrap();
    }

    fn all_notes_off(&mut self, patch_id: PatchId, channel: MidiChannel) {
        self.app_loop
            .dispatch_from(
                AppEvent::Midi {
                    patch_id,
                    message: MidiMessage::all_notes_off(channel),
                },
                EventSource::System,
            )
            .unwrap();
    }

    /// Drives one occupancy request through preparation, activation at the
    /// next rendered block boundary, and off-callback acknowledgement.
    /// Returns the number of retired graphs collected on control.
    fn complete_pending_change(&mut self, output: &mut [f32]) -> u64 {
        self.admit_pending_change();
        assert!(self.worker.advance());
        let staged = self.app_loop.advance_structural().unwrap();
        assert_eq!(staged.rejected_worker_event(), None);
        assert!(staged.worker_result_polled());
        assert!(
            !staged.failure_dispatched(),
            "unexpected refusal: {:?}",
            self.app_loop.engine_selection_status()
        );
        assert!(staged.graph_stage().is_some());
        let before = self.renderer.active_revision();
        self.counted_render(output);
        assert!(
            self.renderer.active_revision() > before,
            "block-boundary activation"
        );
        let ack = self.app_loop.advance_structural().unwrap();
        assert!(ack.activation_acknowledged().is_some());
        assert_eq!(
            self.app_loop.engine_selection_status().kind(),
            EngineSelectionStatusKind::Ready
        );
        ack.collected_count()
    }
}

fn soundfont_patch(id: u32, channel: u8, track: u8) -> Patch {
    Patch::new(
        PatchId::new(id).unwrap(),
        format!("Topology {id}"),
        create_soundfont_config(
            &crest_synth::adapter::production_instruments::production_soundfont_capability()
                .unwrap(),
            SoundFontInstrument::new(0, channel, false).unwrap(),
        )
        .unwrap(),
        MidiChannel::new(channel).unwrap(),
        PatchOutput::to_track(MixerTrackId::new(track).unwrap()),
    )
}

/// Composes the complete production-path fixture: reducer, projector,
/// deterministic worker, coordinator, and renderer, with the production
/// default return occupancy and one track sending to bus 2.
fn fixture() -> Fixture {
    let registry = production_capability_registry().unwrap();
    let effects = production_effect_registry().unwrap();
    let bank = production_default_bus_returns(&effects).unwrap();

    // Track 0 sends to bus 2 so a later occupancy on bus 2 is audible;
    // track 5 begins muted for the FR-018 reroute proof.
    let mut sends = [0.0_f32; 8];
    sends[2] = 0.6;
    let mixer = MixerState::default()
        .with_track(
            MixerTrackId::new(0).unwrap(),
            MixerTrackParameters::from_values(0.0, 0.0, false, false, sends).unwrap(),
        )
        .with_track(
            MixerTrackId::new(5).unwrap(),
            MixerTrackParameters::from_values(0.0, 0.0, true, false, [0.0; 8]).unwrap(),
        );

    let mut state = AppState::for_graph_with_effects(
        registry.clone(),
        effects.clone(),
        GlobalParameters::new(0.0).unwrap(),
        GraphRevision::INITIAL,
    )
    .with_initial_returns(bank)
    .with_initial_mixer(mixer);
    state
        .apply(AppEvent::InstallPatches(vec![
            soundfont_patch(1, 0, 0),
            soundfont_patch(2, 1, 1),
        ]))
        .unwrap();

    let initial_transport = ParameterSnapshot::new(
        0,
        GlobalParameters::new(0.0).unwrap(),
        MixerState::default(),
        &[],
    )
    .unwrap();
    let boundary = LockFreeAudioBoundary::new(128, initial_transport);
    let (audio_control, audio_callback) = boundary.into_handles();
    let mut app_loop = AppLoop::new(
        state,
        StateProjector::for_graph(GraphRevision::INITIAL),
        audio_control,
    )
    .unwrap();

    let audio_config =
        AudioDeviceConfig::new(SAMPLE_RATE, 2, AudioSampleFormat::F32, FRAME_COUNT).unwrap();
    let instrument_preparers = production_instrument_preparers().unwrap();
    let effect_preparers = production_effect_preparers().unwrap();
    let initial_graph = PreparedGraphBuilder::new(&registry, &instrument_preparers)
        .with_effects(&effects, &effect_preparers)
        .with_returns(app_loop.bus_returns())
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
    let worker = DeterministicGraphPreparationWorker::new_with_effects(
        registry.clone(),
        production_instrument_preparers().unwrap(),
        effects.clone(),
        production_effect_preparers().unwrap(),
        audio_config,
    );
    let worker_handle = worker.advance_handle();
    app_loop
        .configure_engine_selection(
            DescriptorDefaultConfigFactory::new(
                registry,
                production_instrument_providers().unwrap(),
            ),
            worker,
            structural_control,
            &initial_graph,
            audio_config,
        )
        .unwrap();
    let (writer, reader) = AtomicAudioObservation::default().into_handles();
    let renderer =
        AudioRenderer::with_observation(audio_callback, structural_callback, initial_graph, writer);

    Fixture {
        app_loop,
        renderer,
        worker: worker_handle,
        observation: reader,
    }
}

fn entry(name: &str) -> EffectCapabilityId {
    EffectCapabilityId::new(name).unwrap()
}

fn block_energy(output: &[f32]) -> f64 {
    output
        .iter()
        .map(|sample| f64::from(*sample) * f64::from(*sample))
        .sum()
}

/// T032 + T034: both occupancy actions enter through the canonical semantic
/// action path, are prepared off-callback as complete graphs, activate at a
/// block boundary, and become audible; return-level state survives.
#[test]
fn occupancy_changes_travel_the_canonical_structural_path_and_become_audible() {
    let mut fixture = fixture();
    let mut output = [0.0_f32; SAMPLE_COUNT];
    let patch_id = PatchId::new(1).unwrap();
    let channel = MidiChannel::new(0).unwrap();

    // Baseline block with the production default occupancy.
    fixture.note_on(patch_id, channel);
    fixture.counted_render(&mut output);
    let baseline = output;

    // Occupy bus 2 with a registry entry through the semantic action path.
    fixture
        .app_loop
        .dispatch_action_from(
            SemanticAction::SetReturnOccupancy {
                bus: BusId::new(2).unwrap(),
                entry: Some(entry("effect.chorus")),
            },
            EventSource::Keyboard,
        )
        .unwrap();
    let status = fixture.app_loop.engine_selection_status();
    assert_eq!(status.kind(), EngineSelectionStatusKind::Loading);
    assert!(matches!(
        status.correlation().unwrap().intent(),
        StructuralEditIntent::SetReturnOccupancy { bus, .. } if bus.index() == 2
    ));
    fixture.complete_pending_change(&mut output);

    // The canonical bank, the snapshot, and the audible mix all agree.
    assert!(fixture
        .app_loop
        .bus_returns()
        .bus_return(BusId::new(2).unwrap())
        .is_occupied());
    assert!(fixture.app_loop.current_parameters().returns()[2].is_active());
    fixture.counted_render(&mut output);
    assert_ne!(baseline[..], output[..], "bus 2 occupancy is audible");

    // Occupy Patch 1 slot 1 through the same lifecycle.
    fixture
        .app_loop
        .dispatch_action_from(
            SemanticAction::SetSlotOccupancy {
                patch_id,
                slot: EffectSlotIndex::new(1).unwrap(),
                entry: Some(entry("effect.chorus")),
            },
            EventSource::Keyboard,
        )
        .unwrap();
    fixture.complete_pending_change(&mut output);
    let patch = &fixture.app_loop.patches()[0];
    assert!(patch
        .effect_slot(EffectSlotIndex::new(1).unwrap())
        .is_some());
    assert!(fixture.app_loop.current_parameters().patches()[0].effects()[1].is_active());

    // Clearing travels the same path; `None` clears, never substitutes.
    fixture
        .app_loop
        .dispatch_action_from(
            SemanticAction::SetSlotOccupancy {
                patch_id,
                slot: EffectSlotIndex::new(1).unwrap(),
                entry: None,
            },
            EventSource::Keyboard,
        )
        .unwrap();
    fixture.complete_pending_change(&mut output);
    assert!(fixture.app_loop.patches()[0]
        .effect_slot(EffectSlotIndex::new(1).unwrap())
        .is_none());

    let observation = fixture.observation.read_latest_on_control();
    assert_eq!(observation.non_finite_samples(), 0);
}

/// T033: every invalid class produces a distinct, attributable refusal and
/// mutates nothing. Out-of-range positions are unrepresentable at the type
/// boundary; unresolvable entries are rejected before publication.
#[test]
fn invalid_occupancy_changes_are_refused_before_publication_without_mutation() {
    // Out-of-range positions cannot even be constructed (rejected, not clamped).
    assert!(EffectSlotIndex::new(MAX_EFFECT_SLOTS).is_err());

    let mut fixture = fixture();
    let before_hash = fixture
        .app_loop
        .current_state_tree()
        .state_hash()
        .to_owned();
    let before_generation = fixture.app_loop.current_state_tree().generation();

    // Bus identities are representable independently of configured capacity.
    let absent_bus = BusId::new(fixture.app_loop.bus_returns().len() as u16).unwrap();
    assert_eq!(
        fixture.app_loop.dispatch_action_from(
            SemanticAction::SetReturnOccupancy {
                bus: absent_bus,
                entry: Some(entry("effect.chorus"))
            },
            EventSource::Keyboard,
        ),
        Err(EventRejection::InvalidSelection)
    );

    // Unknown Patch identity.
    assert_eq!(
        fixture.app_loop.dispatch_action_from(
            SemanticAction::SetSlotOccupancy {
                patch_id: PatchId::new(99).unwrap(),
                slot: EffectSlotIndex::new(0).unwrap(),
                entry: Some(entry("effect.chorus")),
            },
            EventSource::Keyboard,
        ),
        Err(EventRejection::UnknownPatch)
    );

    // Unresolvable registry entry, slot side.
    assert_eq!(
        fixture.app_loop.dispatch_action_from(
            SemanticAction::SetSlotOccupancy {
                patch_id: PatchId::new(1).unwrap(),
                slot: EffectSlotIndex::new(0).unwrap(),
                entry: Some(entry("effect.absent")),
            },
            EventSource::Keyboard,
        ),
        Err(EventRejection::InvalidEffectConfig)
    );

    // Unresolvable registry entry, return side.
    assert_eq!(
        fixture.app_loop.dispatch_action_from(
            SemanticAction::SetReturnOccupancy {
                bus: BusId::new(4).unwrap(),
                entry: Some(entry("effect.absent")),
            },
            EventSource::Keyboard,
        ),
        Err(EventRejection::InvalidEffectConfig)
    );

    // No rejection mutated state, and nothing entered the lifecycle.
    let tree = fixture.app_loop.current_state_tree();
    assert_eq!(tree.state_hash(), before_hash);
    assert_eq!(tree.generation(), before_generation);
    assert_eq!(
        fixture.app_loop.engine_selection_status().kind(),
        EngineSelectionStatusKind::Ready
    );
    assert!(fixture.app_loop.staged_graph_revision().is_none());
    assert!(fixture.app_loop.in_flight_graph_revision().is_none());
}

/// T035 (C-RT-10): a controlled preparation refusal publishes no graph,
/// leaves the active graph and configuration untouched, names the exact
/// position it refused, and audio continues sample-exactly — proven by
/// comparing every rendered block against an identical run with no request.
#[test]
fn controlled_rejection_leaves_the_active_graph_intact_with_sample_exact_audio() {
    let mut refused = fixture();
    let mut untouched = fixture();
    let patch_id = PatchId::new(1).unwrap();
    let channel = MidiChannel::new(0).unwrap();
    let mut refused_output = [0.0_f32; SAMPLE_COUNT];
    let mut untouched_output = [0.0_f32; SAMPLE_COUNT];

    refused.note_on(patch_id, channel);
    untouched.note_on(patch_id, channel);
    refused.counted_render(&mut refused_output);
    untouched.counted_render(&mut untouched_output);
    assert_eq!(refused_output[..], untouched_output[..]);
    assert!(block_energy(&refused_output) > 0.0, "audio is playing");

    let bank_before = refused.app_loop.bus_returns().clone();
    let patches_before = refused.app_loop.patches().to_vec();

    // Drive a refusal while audio is playing.
    refused
        .app_loop
        .dispatch_action_from(
            SemanticAction::SetReturnOccupancy {
                bus: BusId::new(3).unwrap(),
                entry: Some(entry("effect.delay")),
            },
            EventSource::Keyboard,
        )
        .unwrap();
    refused
        .worker
        .fail_next(EngineSelectionFailure::PreparationFailed);
    refused.admit_pending_change();
    assert!(refused.worker.advance());
    let progress = refused.app_loop.advance_structural().unwrap();
    assert!(progress.failure_dispatched());

    // The refusal is attributable to the exact refused position.
    let status = refused.app_loop.engine_selection_status();
    assert_eq!(status.kind(), EngineSelectionStatusKind::Failed);
    assert_eq!(
        status.failure(),
        Some(EngineSelectionFailure::PreparationFailed)
    );
    assert!(matches!(
        status.correlation().unwrap().intent(),
        StructuralEditIntent::SetReturnOccupancy { bus, .. } if bus.index() == 3
    ));

    // No graph was published; the active graph is untouched.
    assert!(refused.app_loop.staged_graph_revision().is_none());
    assert!(refused.app_loop.in_flight_graph_revision().is_none());
    assert_eq!(refused.renderer.active_revision(), GraphRevision::INITIAL);
    assert_eq!(refused.app_loop.bus_returns(), &bank_before);
    assert_eq!(refused.app_loop.patches(), patches_before.as_slice());

    // Sample-exact continuity: every subsequent block matches the run that
    // never issued the request.
    for _ in 0..3 {
        refused.counted_render(&mut refused_output);
        untouched.counted_render(&mut untouched_output);
        assert_eq!(refused_output[..], untouched_output[..]);
    }
    let observation = refused.observation.read_latest_on_control();
    assert!(observation.primary_patch_rms() > 0.0, "no dropout");
    assert_eq!(observation.non_finite_samples(), 0);
}

/// T037 steps 1–3: recovery after refusal, acknowledgement ordering under a
/// second request (C-RT-13) including stale-acknowledgement discard, and
/// scalar/structural coexistence within one block at the widened size.
#[test]
fn recovery_ordering_and_scalar_structural_coexistence_hold() {
    let mut fixture = fixture();
    let mut output = [0.0_f32; SAMPLE_COUNT];
    let patch_id = PatchId::new(1).unwrap();
    let channel = MidiChannel::new(0).unwrap();
    fixture.note_on(patch_id, channel);
    fixture.counted_render(&mut output);
    let baseline = output;

    // A refused change...
    fixture
        .app_loop
        .dispatch_action_from(
            SemanticAction::SetReturnOccupancy {
                bus: BusId::new(2).unwrap(),
                entry: Some(entry("effect.chorus")),
            },
            EventSource::Keyboard,
        )
        .unwrap();
    fixture
        .worker
        .fail_next(EngineSelectionFailure::PreparationFailed);
    fixture.admit_pending_change();
    assert!(fixture.worker.advance());
    fixture.app_loop.advance_structural().unwrap();
    assert_eq!(
        fixture.app_loop.engine_selection_status().kind(),
        EngineSelectionStatusKind::Failed
    );

    // ...recovers: the next valid change is accepted, becomes audible, and
    // leaves no residue of the failed attempt.
    fixture
        .app_loop
        .dispatch_action_from(
            SemanticAction::SetReturnOccupancy {
                bus: BusId::new(2).unwrap(),
                entry: Some(entry("effect.chorus")),
            },
            EventSource::Keyboard,
        )
        .unwrap();
    let first_request_id = fixture
        .app_loop
        .engine_selection_status()
        .correlation()
        .unwrap()
        .request_id();

    // Ordering: a second change requested before the first acknowledgement is
    // refused as busy — visibly, never queued out of order and never dropped
    // silently.
    assert_eq!(
        fixture.app_loop.dispatch_action_from(
            SemanticAction::SetSlotOccupancy {
                patch_id,
                slot: EffectSlotIndex::new(0).unwrap(),
                entry: Some(entry("effect.chorus")),
            },
            EventSource::Keyboard,
        ),
        Err(EventRejection::StructuralEditBusy)
    );

    // Complete the first change; coexistence: a scalar edit accepted while
    // the structural activation is pending survives the same rendered block.
    fixture.admit_pending_change();
    assert!(fixture.worker.advance());
    fixture.app_loop.advance_structural().unwrap();
    fixture
        .app_loop
        .dispatch_from(AppEvent::Adjust(Direction::Up), EventSource::Keyboard)
        .unwrap();
    let expected_level = fixture
        .app_loop
        .current_parameters()
        .mixer_track(MixerTrackId::new(0).unwrap())
        .level_db();
    let before = fixture.renderer.active_revision();
    fixture.counted_render(&mut output);
    assert!(fixture.renderer.active_revision() > before);
    assert_eq!(
        fixture
            .renderer
            .parameters()
            .mixer_track(MixerTrackId::new(0).unwrap())
            .level_db(),
        expected_level,
        "scalar and structural changes coexist within one block"
    );
    let ack = fixture.app_loop.advance_structural().unwrap();
    assert!(ack.activation_acknowledged().is_some());
    let first_target = fixture.renderer.active_revision();

    // A stale duplicate acknowledgement for the completed request is
    // discarded, never applied twice.
    assert_eq!(
        fixture.app_loop.dispatch_from(
            AppEvent::EngineActivationAcknowledged {
                request_id: first_request_id,
                intent: StructuralEditIntent::SetReturnOccupancy {
                    bus: BusId::new(2).unwrap(),
                    entry: Some(entry("effect.chorus")),
                },
                target_graph_revision: first_target,
                retired_graph_revision: GraphRevision::INITIAL,
                collected: true,
            },
            EventSource::Worker,
        ),
        Err(EventRejection::StaleEngineSelection)
    );

    // The second request is accepted after the first acknowledgement and
    // completes with its own correlation identity.
    fixture
        .app_loop
        .dispatch_action_from(
            SemanticAction::SetSlotOccupancy {
                patch_id,
                slot: EffectSlotIndex::new(0).unwrap(),
                entry: Some(entry("effect.chorus")),
            },
            EventSource::Keyboard,
        )
        .unwrap();
    let second_request_id = fixture
        .app_loop
        .engine_selection_status()
        .correlation()
        .unwrap()
        .request_id();
    assert!(second_request_id > first_request_id);
    fixture.complete_pending_change(&mut output);

    // The acknowledgements arrived exactly once each, in request order.
    let acknowledged: Vec<EngineSelectionRequestId> = fixture
        .app_loop
        .event_log_ref()
        .records()
        .iter()
        .flat_map(|record| record.emitted_events())
        .filter_map(|event| match event {
            EmittedEvent::EngineSelection { effect }
                if effect.kind() == EngineSelectionEffectKind::ActivationAcknowledged =>
            {
                Some(effect.request_id())
            }
            _ => None,
        })
        .collect();
    assert_eq!(acknowledged, vec![first_request_id, second_request_id]);

    // Final state matches the last accepted request: both changes present.
    assert!(fixture
        .app_loop
        .bus_returns()
        .bus_return(BusId::new(2).unwrap())
        .is_occupied());
    assert!(fixture.app_loop.patches()[0]
        .effect_slot(EffectSlotIndex::new(0).unwrap())
        .is_some());
    fixture.counted_render(&mut output);
    assert_ne!(baseline[..], output[..], "the recovered change is audible");
}

/// T037 step 4 (FR-018): a configured multi-slot chain — including per-
/// instance state excited before the reroute — follows its Patch to a
/// different mixer track; nothing about the chain attaches to the vacated or
/// receiving track.
#[test]
fn a_configured_chain_and_its_live_state_follow_the_patch_across_a_reroute() {
    let mut fixture = fixture();
    let mut output = [0.0_f32; SAMPLE_COUNT];
    let patch_id = PatchId::new(1).unwrap();
    let channel = MidiChannel::new(0).unwrap();

    // Build the multi-slot chain through the topology lifecycle:
    // slot 0 chorus, slot 1 delay (250 ms default). The delay replaces its
    // input with the delayed signal, so everything this Patch contributes is
    // per-instance state that was excited at least 250 ms earlier.
    for (slot, name) in [(0_usize, "effect.chorus"), (1, "effect.delay")] {
        fixture
            .app_loop
            .dispatch_action_from(
                SemanticAction::SetSlotOccupancy {
                    patch_id,
                    slot: EffectSlotIndex::new(slot).unwrap(),
                    entry: Some(entry(name)),
                },
                EventSource::Keyboard,
            )
            .unwrap();
        fixture.complete_pending_change(&mut output);
    }
    let chain_before = fixture.app_loop.patches()[0].effect_slots().clone();
    let projected_before = fixture.app_loop.current_parameters().patches()[0]
        .effects()
        .clone();

    // Excite the chain, then silence the instrument. The delayed signal is
    // not yet due: blocks stay silent while the tail rings inside the chain.
    fixture.note_on(patch_id, channel);
    fixture.counted_render(&mut output);
    fixture.counted_render(&mut output);
    fixture.all_notes_off(patch_id, channel);
    for _ in 0..3 {
        fixture.counted_render(&mut output);
        assert!(
            block_energy(&output) < 1.0e-9,
            "the delayed chain output is not yet due"
        );
    }

    // Reroute the Patch to muted track 5 through the validated scalar route
    // path (the prepared graph already owns all sixteen destinations; the
    // reroute replaces no graph, so per-instance state cannot be lost).
    fixture
        .app_loop
        .dispatch_from(
            AppEvent::SelectContext(TopLevelContext::Patch),
            EventSource::Keyboard,
        )
        .unwrap();
    fixture
        .app_loop
        .dispatch_from(
            AppEvent::EnterSurface(crest_synth::control::SurfaceId::PatchUtility),
            EventSource::Keyboard,
        )
        .unwrap();
    // Walk to the output-track row through the declared five-row Utility order
    // rather than assuming it sits one step below the entry row.
    for _ in 0..utility_rows_from_entry(&PatchControlId::Output(PatchOutputParameter::OutputTrack))
    {
        fixture
            .app_loop
            .dispatch_from(AppEvent::Navigate(Direction::Down), EventSource::Keyboard)
            .unwrap();
    }
    for _ in 0..5 {
        fixture
            .app_loop
            .dispatch_from(AppEvent::Adjust(Direction::Right), EventSource::Keyboard)
            .unwrap();
    }
    assert_eq!(
        fixture.app_loop.patches()[0].output().track_id(),
        MixerTrackId::new(5).unwrap()
    );

    // The chain and its parameter values followed the Patch untouched.
    assert_eq!(fixture.app_loop.patches()[0].effect_slots(), &chain_before);
    assert_eq!(
        fixture.app_loop.current_parameters().patches()[0].effects(),
        &projected_before
    );

    // Receiving track still muted: the tail emerges inside the chain but is
    // gated at track 5 — and nothing sounds on the vacated (unmuted) track 0,
    // so no chain state stayed attached to it.
    fixture.counted_render(&mut output);
    fixture.counted_render(&mut output);
    assert!(
        block_energy(&output) < 1.0e-9,
        "the vacated track carries nothing and the receiving track is muted"
    );

    // Unmute track 5 through the reducer: the pre-reroute tail becomes
    // audible on the receiving track — the per-instance state followed.
    fixture
        .app_loop
        .dispatch_from(
            AppEvent::SelectContext(TopLevelContext::Mixer),
            EventSource::Keyboard,
        )
        .unwrap();
    for _ in 0..2 {
        fixture
            .app_loop
            .dispatch_from(AppEvent::Navigate(Direction::Down), EventSource::Keyboard)
            .unwrap();
    }
    for _ in 0..5 {
        fixture
            .app_loop
            .dispatch_from(AppEvent::Navigate(Direction::Right), EventSource::Keyboard)
            .unwrap();
    }
    fixture
        .app_loop
        .dispatch_from(AppEvent::Adjust(Direction::Right), EventSource::Keyboard)
        .unwrap();
    let mut tail_energy = 0.0_f64;
    for _ in 0..3 {
        fixture.counted_render(&mut output);
        tail_energy += block_energy(&output);
    }
    assert!(
        tail_energy > 1.0e-9,
        "the audible tail started before the reroute follows the Patch"
    );
}

/// WP10/T058 (AS-1.5, SC-001, NFR-001, NFR-004): held notes survive an
/// accepted slot-occupancy activation, and the untouched Patch is
/// sample-continuous across the activation block — proven by comparing every
/// rendered block against an identical run that never issued the change.
///
/// The untouched Patch carries a stateful chorus chain so continuity covers
/// engine voices AND effect-instance state (LFO phase), not merely a sustained
/// level. The changed position's fresh instance is proven to restart from
/// reset local state: a delay occupying the changed slot outputs its wet-only
/// silence for exactly its delay time before the held note's signal emerges —
/// while the same Patch's voice demonstrably kept sounding into it.
#[test]
fn held_voices_carry_over_a_slot_occupancy_activation_sample_continuously() {
    const POST_BLOCKS: usize = 13;
    // 250 ms delay at 48 kHz in 2048-frame blocks: the first four complete
    // post-activation blocks (170 ms) precede the delay time.
    const SILENT_WET_BLOCKS: usize = 4;

    let mut changed = fixture();
    let mut control = fixture();
    let untouched_patch = PatchId::new(1).unwrap();
    let changed_patch = PatchId::new(2).unwrap();
    let mut output = [0.0_f32; SAMPLE_COUNT];

    // Both runs: give the untouched Patch a stateful chorus chain through the
    // production lifecycle.
    for fixture in [&mut changed, &mut control] {
        fixture
            .app_loop
            .dispatch_action_from(
                SemanticAction::SetSlotOccupancy {
                    patch_id: untouched_patch,
                    slot: EffectSlotIndex::new(0).unwrap(),
                    entry: Some(entry("effect.chorus")),
                },
                EventSource::Keyboard,
            )
            .unwrap();
        fixture.complete_pending_change(&mut output);
    }

    // Hold one note on each Patch and prove the two runs are aligned.
    for fixture in [&mut changed, &mut control] {
        fixture.note_on(untouched_patch, MidiChannel::new(0).unwrap());
        fixture.note_on(changed_patch, MidiChannel::new(1).unwrap());
    }
    let mut changed_output = [0.0_f32; SAMPLE_COUNT];
    let mut control_output = [0.0_f32; SAMPLE_COUNT];
    for _ in 0..2 {
        changed.counted_render(&mut changed_output);
        control.counted_render(&mut control_output);
        assert_eq!(changed_output[..], control_output[..]);
    }
    assert!(block_energy(&changed_output) > 0.0, "both notes ring");

    // The accepted delta: occupy the second Patch's empty slot 0 with the
    // wet-only delay while both notes ring.
    changed
        .app_loop
        .dispatch_action_from(
            SemanticAction::SetSlotOccupancy {
                patch_id: changed_patch,
                slot: EffectSlotIndex::new(0).unwrap(),
                entry: Some(entry("effect.delay")),
            },
            EventSource::Keyboard,
        )
        .unwrap();
    changed.admit_pending_change();
    assert!(changed.worker.advance());
    let staged = changed.app_loop.advance_structural().unwrap();
    assert!(staged.graph_stage().is_some());

    // Activation block plus the post-activation window, block-aligned across
    // both runs, with callback discipline measured on every changed-run block.
    let source_revision = changed.renderer.active_revision();
    let mut changed_allocations = 0_usize;
    let mut changed_deallocations = 0_usize;
    let mut untouched_stems: Vec<(Vec<f32>, Vec<f32>)> = Vec::new();
    let mut changed_wet_energy: Vec<f64> = Vec::new();
    let mut control_dry_energy: Vec<f64> = Vec::new();
    for block in 0..=POST_BLOCKS {
        let memory = changed.counted_render(&mut changed_output);
        changed_allocations += memory.0;
        changed_deallocations += memory.1;
        control.counted_render(&mut control_output);
        if block == 0 {
            assert!(
                changed.renderer.active_revision() > source_revision,
                "block-boundary activation"
            );
        }
        let changed_stems = changed.renderer.active_patch_audio().stems();
        let control_stems = control.renderer.active_patch_audio().stems();
        assert_eq!(changed_stems[0].patch_id(), Some(untouched_patch));
        assert_eq!(changed_stems[1].patch_id(), Some(changed_patch));
        untouched_stems.push((
            changed_stems[0].samples().to_vec(),
            control_stems[0].samples().to_vec(),
        ));
        changed_wet_energy.push(block_energy(changed_stems[1].samples()));
        control_dry_energy.push(block_energy(control_stems[1].samples()));
    }
    let ack = changed.app_loop.advance_structural().unwrap();
    assert!(ack.activation_acknowledged().is_some());

    // Callback discipline across carry-over activation and every block after:
    // zero allocations, zero deallocations, zero drops (a drop deallocates).
    assert_eq!(changed_allocations, 0);
    assert_eq!(changed_deallocations, 0);

    // Sample-level continuity: the untouched Patch's stem — engine voices
    // plus its carried chorus state — is byte-identical to the run that never
    // issued the change, on the activation block and every block after.
    for (block, (changed_stem, control_stem)) in untouched_stems.iter().enumerate() {
        assert_eq!(
            changed_stem, control_stem,
            "untouched Patch stem diverged at post-activation block {block}"
        );
        assert!(
            block_energy(changed_stem) > 0.0,
            "the held note remains audible at post-activation block {block}"
        );
    }

    // Held notes remain active through the production observation.
    let observation = changed.observation.read_latest_on_control();
    assert_eq!(observation.active_notes(), 2, "both held notes survive");
    assert!(observation.primary_active_notes() > 0);
    assert_eq!(observation.non_finite_samples(), 0);

    // The changed position restarted from reset local state: the fresh
    // wet-only delay contributes silence for its full delay time (an instance
    // carrying superseded state would sound immediately), while the control
    // run's unprocessed stem proves the same Patch's voice kept sounding.
    for block in 0..=SILENT_WET_BLOCKS {
        assert!(
            changed_wet_energy[block] < 1.0e-9,
            "fresh delay must start from an empty line at block {block}"
        );
        assert!(
            control_dry_energy[block] > 0.0,
            "the changed Patch's held voice keeps sounding into the fresh instance"
        );
    }
    // ...and the held note then emerges through the fresh delay: the voice
    // survived on the changed Patch too; only the slot's local state restarted.
    let emerged: f64 = changed_wet_energy[SILENT_WET_BLOCKS + 2..].iter().sum();
    assert!(
        emerged > 0.0,
        "the held note emerges through the fresh delay after its delay time"
    );

    // The delta landed exactly at its named position.
    assert!(changed.app_loop.patches()[1]
        .effect_slot(EffectSlotIndex::new(0).unwrap())
        .is_some());
    assert!(changed.app_loop.patches()[0]
        .effect_slot(EffectSlotIndex::new(0).unwrap())
        .is_some());
    assert_eq!(control.renderer.active_revision(), source_revision);
}

/// WP10/T058 — the operator-ruled CLEAR case (2026-07-31 narrowing; AS-1.5,
/// the narrowed SC-001, NFR-001, NFR-004): a delta that only CLEARS a slot
/// occupancy preserves every sounding voice across the swap, envelope and
/// channel state intact, with zero callback allocation or destruction.
///
/// Sample-level continuity of the engine output is proven against a third
/// run that never installed the cleared occupant at all: after the clear,
/// the cleared Patch's stem must be byte-identical to that never-installed
/// run's raw stem on the activation block and every block after. A retrigger
/// would restart the envelope and diverge; a gap would go silent; a leaked
/// superseded delay tail would add energy — all three fail the equality.
/// The untouched Patch is simultaneously proven continuous (engine voices
/// plus carried chorus state) against a run that never issued the clear.
#[test]
fn held_voices_carry_over_a_slot_clear_activation_sample_continuously() {
    const POST_BLOCKS: usize = 8;

    let mut cleared = fixture();
    let mut control = fixture();
    let mut never_installed = fixture();
    let untouched_patch = PatchId::new(1).unwrap();
    let cleared_patch = PatchId::new(2).unwrap();
    let mut output = [0.0_f32; SAMPLE_COUNT];

    // All three runs: the untouched Patch carries a stateful chorus chain
    // through the production lifecycle.
    for fixture in [&mut cleared, &mut control, &mut never_installed] {
        fixture
            .app_loop
            .dispatch_action_from(
                SemanticAction::SetSlotOccupancy {
                    patch_id: untouched_patch,
                    slot: EffectSlotIndex::new(0).unwrap(),
                    entry: Some(entry("effect.chorus")),
                },
                EventSource::Keyboard,
            )
            .unwrap();
        fixture.complete_pending_change(&mut output);
    }
    // The cleared and control runs install the delay that will later be
    // cleared; the reference run renders one plain block instead, so all
    // three runs stay block-aligned with identical engine trajectories.
    for fixture in [&mut cleared, &mut control] {
        fixture
            .app_loop
            .dispatch_action_from(
                SemanticAction::SetSlotOccupancy {
                    patch_id: cleared_patch,
                    slot: EffectSlotIndex::new(0).unwrap(),
                    entry: Some(entry("effect.delay")),
                },
                EventSource::Keyboard,
            )
            .unwrap();
        fixture.complete_pending_change(&mut output);
    }
    never_installed.counted_render(&mut output);

    // Hold one note on each Patch in every run and let them ring.
    for fixture in [&mut cleared, &mut control, &mut never_installed] {
        fixture.note_on(untouched_patch, MidiChannel::new(0).unwrap());
        fixture.note_on(cleared_patch, MidiChannel::new(1).unwrap());
    }
    let mut cleared_output = [0.0_f32; SAMPLE_COUNT];
    let mut control_output = [0.0_f32; SAMPLE_COUNT];
    let mut reference_output = [0.0_f32; SAMPLE_COUNT];
    for _ in 0..2 {
        cleared.counted_render(&mut cleared_output);
        control.counted_render(&mut control_output);
        never_installed.counted_render(&mut reference_output);
        assert_eq!(cleared_output[..], control_output[..]);
    }
    assert!(block_energy(&cleared_output) > 0.0, "both notes ring");

    // The accepted delta: CLEAR the second Patch's occupied slot 0 while
    // both notes ring.
    cleared
        .app_loop
        .dispatch_action_from(
            SemanticAction::SetSlotOccupancy {
                patch_id: cleared_patch,
                slot: EffectSlotIndex::new(0).unwrap(),
                entry: None,
            },
            EventSource::Keyboard,
        )
        .unwrap();
    cleared.admit_pending_change();
    assert!(cleared.worker.advance());
    let staged = cleared.app_loop.advance_structural().unwrap();
    assert!(staged.graph_stage().is_some());

    // Activation block plus the post-activation window, block-aligned across
    // all three runs, with callback discipline measured on every cleared-run
    // block.
    let source_revision = cleared.renderer.active_revision();
    let mut cleared_allocations = 0_usize;
    let mut cleared_deallocations = 0_usize;
    let mut untouched_stems: Vec<(Vec<f32>, Vec<f32>)> = Vec::new();
    let mut cleared_stems: Vec<(Vec<f32>, Vec<f32>)> = Vec::new();
    for block in 0..=POST_BLOCKS {
        let memory = cleared.counted_render(&mut cleared_output);
        cleared_allocations += memory.0;
        cleared_deallocations += memory.1;
        control.counted_render(&mut control_output);
        never_installed.counted_render(&mut reference_output);
        if block == 0 {
            assert!(
                cleared.renderer.active_revision() > source_revision,
                "block-boundary activation"
            );
        }
        let cleared_run = cleared.renderer.active_patch_audio().stems();
        let control_run = control.renderer.active_patch_audio().stems();
        let reference_run = never_installed.renderer.active_patch_audio().stems();
        assert_eq!(cleared_run[0].patch_id(), Some(untouched_patch));
        assert_eq!(cleared_run[1].patch_id(), Some(cleared_patch));
        untouched_stems.push((
            cleared_run[0].samples().to_vec(),
            control_run[0].samples().to_vec(),
        ));
        cleared_stems.push((
            cleared_run[1].samples().to_vec(),
            reference_run[1].samples().to_vec(),
        ));
    }
    let ack = cleared.app_loop.advance_structural().unwrap();
    assert!(ack.activation_acknowledged().is_some());

    // Callback discipline across the clear activation and every block after:
    // zero allocations, zero deallocations, zero drops (a drop deallocates).
    assert_eq!(cleared_allocations, 0);
    assert_eq!(cleared_deallocations, 0);

    // The untouched Patch — engine voices plus carried chorus state — is
    // byte-identical to the run that never issued the clear.
    for (block, (cleared_stem, control_stem)) in untouched_stems.iter().enumerate() {
        assert_eq!(
            cleared_stem, control_stem,
            "untouched Patch stem diverged at post-clear block {block}"
        );
        assert!(
            block_energy(cleared_stem) > 0.0,
            "the untouched held note remains audible at post-clear block {block}"
        );
    }

    // The CLEARED Patch's sounding voice survived the swap sample-exactly:
    // from the activation block on, its stem is byte-identical to the run
    // where the same held note never had a delay installed at all — same
    // envelope phase, same channel state, no retrigger, no gap, and no
    // residual superseded delay tail.
    for (block, (cleared_stem, reference_stem)) in cleared_stems.iter().enumerate() {
        assert_eq!(
            cleared_stem, reference_stem,
            "cleared Patch voice diverged at post-clear block {block}"
        );
        assert!(
            block_energy(cleared_stem) > 0.0,
            "the cleared Patch's held note remains audible at post-clear block {block}"
        );
    }

    // Held notes remain active through the production observation.
    let observation = cleared.observation.read_latest_on_control();
    assert_eq!(observation.active_notes(), 2, "both held notes survive");
    assert!(observation.primary_active_notes() > 0);
    assert_eq!(observation.non_finite_samples(), 0);

    // The delta landed exactly at its named position and nowhere else.
    assert!(cleared.app_loop.patches()[1]
        .effect_slot(EffectSlotIndex::new(0).unwrap())
        .is_none());
    assert!(cleared.app_loop.patches()[0]
        .effect_slot(EffectSlotIndex::new(0).unwrap())
        .is_some());
    assert_eq!(control.renderer.active_revision(), source_revision);
}

/// The RETURN-side twin of the slot-clear proof directly above: a delta that
/// only CLEARS an occupied
/// bus return preserves every sounding voice across the swap.
///
/// Fixture: track 0 sends 0.6 to bus 2 (the raised send), and both the
/// clearing run and its untouched twin occupy bus 2 with a chorus so the
/// return is audibly wet while notes are held; the dry Patch carries a
/// stateful chorus chain so continuity covers engine voices AND carried
/// effect-instance state. The clear activates at a block boundary while both
/// notes ring.
///
/// The protected (dry/track) path — each held voice's per-Patch stem — must
/// be byte-identical to the untouched twin on the activation block and every
/// block after. The cleared return's declared semantics — it contributes
/// exact silence afterward, never a torn or clicking tail — is proven by
/// comparing the cleared run's full mix byte-exactly against a third run
/// whose return was never occupied at all. The untouched twin's mix keeps
/// its wet contribution on those same blocks: that wet-tail difference is
/// deliberately excluded from the continuity comparison, and its continued
/// presence is asserted so the exclusion is never vacuous.
#[test]
fn return_clear_held_note_continuity_preserves_held_voices_sample_exactly() {
    const POST_BLOCKS: usize = 8;

    let mut cleared = fixture();
    let mut untouched = fixture();
    let mut never_occupied = fixture();
    let sending_patch = PatchId::new(1).unwrap();
    let dry_patch = PatchId::new(2).unwrap();
    let cleared_bus = BusId::new(2).unwrap();
    let mut output = [0.0_f32; SAMPLE_COUNT];

    // All three runs: the dry Patch carries a stateful chorus chain through
    // the production lifecycle, so the protected path includes carried
    // effect-instance state, not merely engine voices.
    for fixture in [&mut cleared, &mut untouched, &mut never_occupied] {
        fixture
            .app_loop
            .dispatch_action_from(
                SemanticAction::SetSlotOccupancy {
                    patch_id: dry_patch,
                    slot: EffectSlotIndex::new(0).unwrap(),
                    entry: Some(entry("effect.chorus")),
                },
                EventSource::Keyboard,
            )
            .unwrap();
        fixture.complete_pending_change(&mut output);
    }
    // The clearing run and its untouched twin occupy the return that will
    // later be cleared; the reference run renders one plain block instead,
    // so all three runs stay block-aligned with identical engine
    // trajectories.
    for fixture in [&mut cleared, &mut untouched] {
        fixture
            .app_loop
            .dispatch_action_from(
                SemanticAction::SetReturnOccupancy {
                    bus: cleared_bus,
                    entry: Some(entry("effect.chorus")),
                },
                EventSource::Keyboard,
            )
            .unwrap();
        fixture.complete_pending_change(&mut output);
    }
    never_occupied.counted_render(&mut output);

    // Hold one note on each Patch in every run and let them ring: the
    // occupied return receiving the raised send is audibly wet before the
    // clear, so clearing it is a real change, not a no-op.
    for fixture in [&mut cleared, &mut untouched, &mut never_occupied] {
        fixture.note_on(sending_patch, MidiChannel::new(0).unwrap());
        fixture.note_on(dry_patch, MidiChannel::new(1).unwrap());
    }
    let mut cleared_output = [0.0_f32; SAMPLE_COUNT];
    let mut untouched_output = [0.0_f32; SAMPLE_COUNT];
    let mut reference_output = [0.0_f32; SAMPLE_COUNT];
    for _ in 0..2 {
        cleared.counted_render(&mut cleared_output);
        untouched.counted_render(&mut untouched_output);
        never_occupied.counted_render(&mut reference_output);
        assert_eq!(cleared_output[..], untouched_output[..]);
    }
    assert!(block_energy(&cleared_output) > 0.0, "both notes ring");
    assert_ne!(
        cleared_output[..],
        reference_output[..],
        "the occupied return is audibly wet before the clear"
    );

    // The accepted delta: CLEAR bus 2's return occupancy while both notes
    // ring — and nothing else. The correlated intent is the clear itself.
    let patches_before = cleared.app_loop.patches().to_vec();
    cleared
        .app_loop
        .dispatch_action_from(
            SemanticAction::SetReturnOccupancy {
                bus: cleared_bus,
                entry: None,
            },
            EventSource::Keyboard,
        )
        .unwrap();
    assert!(matches!(
        cleared
            .app_loop
            .engine_selection_status()
            .correlation()
            .unwrap()
            .intent(),
        StructuralEditIntent::SetReturnOccupancy { bus, entry: None } if bus.index() == 2
    ));
    cleared.admit_pending_change();
    assert!(cleared.worker.advance());
    let staged = cleared.app_loop.advance_structural().unwrap();
    assert!(staged.graph_stage().is_some());

    // Activation block plus the post-activation window, block-aligned across
    // all three runs, with callback discipline measured on every cleared-run
    // block.
    let source_revision = cleared.renderer.active_revision();
    let mut cleared_allocations = 0_usize;
    let mut cleared_deallocations = 0_usize;
    let mut protected_stems: Vec<[(Vec<f32>, Vec<f32>); 2]> = Vec::new();
    let mut mixes: Vec<(Vec<f32>, Vec<f32>, Vec<f32>)> = Vec::new();
    for block in 0..=POST_BLOCKS {
        let memory = cleared.counted_render(&mut cleared_output);
        cleared_allocations += memory.0;
        cleared_deallocations += memory.1;
        untouched.counted_render(&mut untouched_output);
        never_occupied.counted_render(&mut reference_output);
        if block == 0 {
            assert!(
                cleared.renderer.active_revision() > source_revision,
                "block-boundary activation"
            );
        }
        let cleared_run = cleared.renderer.active_patch_audio().stems();
        let untouched_run = untouched.renderer.active_patch_audio().stems();
        assert_eq!(cleared_run[0].patch_id(), Some(sending_patch));
        assert_eq!(cleared_run[1].patch_id(), Some(dry_patch));
        protected_stems.push([
            (
                cleared_run[0].samples().to_vec(),
                untouched_run[0].samples().to_vec(),
            ),
            (
                cleared_run[1].samples().to_vec(),
                untouched_run[1].samples().to_vec(),
            ),
        ]);
        mixes.push((
            cleared_output.to_vec(),
            untouched_output.to_vec(),
            reference_output.to_vec(),
        ));
    }
    let ack = cleared.app_loop.advance_structural().unwrap();
    assert!(ack.activation_acknowledged().is_some());

    // Callback discipline across the clear activation and every block after:
    // zero allocations, zero deallocations, zero drops (a drop deallocates).
    assert_eq!(cleared_allocations, 0);
    assert_eq!(cleared_deallocations, 0);

    // The protected (dry/track) path: every held voice's stem — the sending
    // Patch's raw engine voice and the dry Patch's voice through its carried
    // chorus chain — is byte-identical to the untouched twin on the
    // activation block and every block after. A retrigger, gap, or click at
    // the boundary fails this equality.
    for (block, stems) in protected_stems.iter().enumerate() {
        for (patch_index, (cleared_stem, untouched_stem)) in stems.iter().enumerate() {
            assert_eq!(
                cleared_stem, untouched_stem,
                "protected Patch {patch_index} stem diverged at post-clear block {block}"
            );
            assert!(
                block_energy(cleared_stem) > 0.0,
                "held note on Patch {patch_index} remains audible at post-clear block {block}"
            );
        }
    }

    // The cleared return contributes exact silence from the activation block
    // on: the cleared run's full mix is byte-identical to the run whose
    // return was never occupied — no torn or clicking wet tail. On the same
    // blocks the untouched twin still carries its wet contribution: that
    // difference is the wet tail deliberately excluded from the continuity
    // comparison, asserted present so the exclusion is not vacuous.
    for (block, (cleared_mix, untouched_mix, reference_mix)) in mixes.iter().enumerate() {
        assert_eq!(
            cleared_mix, reference_mix,
            "cleared return contributed non-silence at post-clear block {block}"
        );
        assert_ne!(
            untouched_mix, reference_mix,
            "the untouched twin's excluded wet contribution vanished at post-clear block {block}"
        );
    }

    // Held notes remain active through the production observation.
    let observation = cleared.observation.read_latest_on_control();
    assert_eq!(observation.active_notes(), 2, "both held notes survive");
    assert!(observation.primary_active_notes() > 0);
    assert_eq!(observation.non_finite_samples(), 0);

    // The delta landed exactly at its named position and nowhere else: the
    // return is cleared canonically and in the projection, the Patches —
    // including the carried chorus chain — are untouched, the production
    // default returns survive, and the twin never advanced its revision.
    assert!(!cleared
        .app_loop
        .bus_returns()
        .bus_return(cleared_bus)
        .is_occupied());
    assert!(!cleared.app_loop.current_parameters().returns()[2].is_active());
    assert_eq!(cleared.app_loop.patches(), patches_before.as_slice());
    for bus in BusId::ALL {
        if bus == cleared_bus {
            continue;
        }
        assert_eq!(
            cleared.app_loop.bus_returns().bus_return(bus).is_occupied(),
            untouched
                .app_loop
                .bus_returns()
                .bus_return(bus)
                .is_occupied(),
            "the clear-only delta must not touch bus {}",
            bus.index()
        );
    }
    assert!(untouched
        .app_loop
        .bus_returns()
        .bus_return(cleared_bus)
        .is_occupied());
    assert_eq!(untouched.renderer.active_revision(), source_revision);
}

/// T038 (C-RT-14): repeated topology changes at the widened size retire every
/// superseded graph off-callback with zero callback allocation, deallocation,
/// or destruction, and leave nothing owned at exit.
#[test]
fn repeated_topology_changes_retire_off_callback_with_clean_ownership_at_exit() {
    let mut fixture = fixture();
    let mut output = [0.0_f32; SAMPLE_COUNT];
    let patch_id = PatchId::new(1).unwrap();
    let channel = MidiChannel::new(0).unwrap();
    fixture.note_on(patch_id, channel);

    let changes: [SemanticAction; 3] = [
        SemanticAction::SetReturnOccupancy {
            bus: BusId::new(2).unwrap(),
            entry: Some(entry("effect.chorus")),
        },
        SemanticAction::SetSlotOccupancy {
            patch_id,
            slot: EffectSlotIndex::new(2).unwrap(),
            entry: Some(entry("effect.reverb")),
        },
        SemanticAction::SetReturnOccupancy {
            bus: BusId::new(2).unwrap(),
            entry: None,
        },
    ];
    let mut collected = 0_u64;
    let mut callback_allocations = 0_usize;
    let mut callback_deallocations = 0_usize;
    for change in changes {
        fixture
            .app_loop
            .dispatch_action_from(change, EventSource::Keyboard)
            .unwrap();
        fixture.admit_pending_change();
        assert!(fixture.worker.advance());
        fixture.app_loop.advance_structural().unwrap();
        let memory = fixture.counted_render(&mut output);
        callback_allocations += memory.0;
        callback_deallocations += memory.1;
        let ack = fixture.app_loop.advance_structural().unwrap();
        assert!(ack.activation_acknowledged().is_some());
        collected += ack.collected_count();
    }

    // Every superseded graph came back on control ownership, none was
    // destroyed on the callback path, and the callback never allocated.
    assert_eq!(collected, changes_len());
    assert_eq!(callback_allocations, 0);
    assert_eq!(callback_deallocations, 0);

    // Clean ownership at exit: nothing structural stays owned by control
    // orchestration once the renderer and worker are torn down.
    drop(fixture.renderer);
    assert_eq!(fixture.app_loop.owned_structural_graphs_on_control(), 0);
    fixture
        .app_loop
        .shutdown_engine_selection_on_control()
        .unwrap();
}

const fn changes_len() -> u64 {
    3
}

/// T036 (FR-014): the topology change's status — pending, refused with its
/// reason, accepted — is projected from reducer-owned state with the exact
/// position it applies to. The UI renders this tree; it computes nothing.
#[test]
fn topology_status_is_projected_with_reason_and_position() {
    let mut fixture = fixture();
    let mut output = [0.0_f32; SAMPLE_COUNT];

    // Pending: the projected status names the position being prepared.
    fixture
        .app_loop
        .dispatch_action_from(
            SemanticAction::SetReturnOccupancy {
                bus: BusId::new(6).unwrap(),
                entry: Some(entry("effect.delay")),
            },
            EventSource::Keyboard,
        )
        .unwrap();
    let tree: serde_json::Value =
        serde_json::from_str(fixture.app_loop.current_state_tree().json()).unwrap();
    assert_eq!(tree["engineSelection"]["kind"], "loading");
    assert_eq!(
        tree["engineSelection"]["correlation"]["intent"]["kind"],
        "setReturnOccupancy"
    );
    assert_eq!(tree["engineSelection"]["correlation"]["intent"]["bus"], 6);
    assert_eq!(
        tree["engineSelection"]["correlation"]["intent"]["entry"],
        "effect.delay"
    );
    assert!(tree["engineSelection"]["failure"].is_null());

    // Refused: the reason and the exact position stay projected together.
    fixture
        .worker
        .fail_next(EngineSelectionFailure::PreparationFailed);
    for expected in [
        EngineSelectionStatusKind::Validating,
        EngineSelectionStatusKind::Preparing,
    ] {
        let admission = fixture.app_loop.advance_structural().unwrap();
        assert_eq!(
            admission.engine_selection_lifecycle_advanced(),
            Some(expected)
        );
    }
    assert!(fixture.worker.advance());
    fixture.app_loop.advance_structural().unwrap();
    let tree: serde_json::Value =
        serde_json::from_str(fixture.app_loop.current_state_tree().json()).unwrap();
    assert_eq!(tree["engineSelection"]["kind"], "failed");
    assert_eq!(tree["engineSelection"]["failure"], "preparationFailed");
    assert_eq!(tree["engineSelection"]["correlation"]["intent"]["bus"], 6);

    // Accepted: a completed slot change reaches Ready; the serialized Patch
    // carries the committed occupancy at its exact position.
    fixture
        .app_loop
        .dispatch_action_from(
            SemanticAction::SetSlotOccupancy {
                patch_id: PatchId::new(1).unwrap(),
                slot: EffectSlotIndex::new(0).unwrap(),
                entry: Some(entry("effect.chorus")),
            },
            EventSource::Keyboard,
        )
        .unwrap();
    let tree: serde_json::Value =
        serde_json::from_str(fixture.app_loop.current_state_tree().json()).unwrap();
    assert_eq!(
        tree["engineSelection"]["correlation"]["intent"]["kind"],
        "setSlotOccupancy"
    );
    assert_eq!(
        tree["engineSelection"]["correlation"]["intent"]["patchId"],
        1
    );
    assert_eq!(tree["engineSelection"]["correlation"]["intent"]["slot"], 0);
    fixture.complete_pending_change(&mut output);
    let tree: serde_json::Value =
        serde_json::from_str(fixture.app_loop.current_state_tree().json()).unwrap();
    assert_eq!(tree["engineSelection"]["kind"], "ready");
    assert!(tree["engineSelection"]["correlation"].is_null());
    assert_eq!(
        tree["patches"][0]["postEffects"][0]["capabilityId"],
        "effect.chorus"
    );
    assert_eq!(
        tree["parameters"]["patches"][0]["effects"][0]["active"],
        true
    );
}

/// The complete lifecycle is deterministic: two independent runs of the same
/// scenario produce byte-identical audio and identical canonical state.
#[test]
fn the_occupancy_lifecycle_is_deterministic_across_two_runs() {
    fn run() -> (Vec<u8>, String) {
        let mut fixture = fixture();
        let mut output = [0.0_f32; SAMPLE_COUNT];
        let patch_id = PatchId::new(1).unwrap();
        let channel = MidiChannel::new(0).unwrap();
        fixture.note_on(patch_id, channel);
        let mut audio = Vec::new();
        fixture.counted_render(&mut output);
        audio.extend(output.iter().flat_map(|sample| sample.to_le_bytes()));

        fixture
            .app_loop
            .dispatch_action_from(
                SemanticAction::SetReturnOccupancy {
                    bus: BusId::new(2).unwrap(),
                    entry: Some(entry("effect.chorus")),
                },
                EventSource::Keyboard,
            )
            .unwrap();
        fixture.complete_pending_change(&mut output);
        audio.extend(output.iter().flat_map(|sample| sample.to_le_bytes()));
        fixture
            .app_loop
            .dispatch_action_from(
                SemanticAction::SetSlotOccupancy {
                    patch_id,
                    slot: EffectSlotIndex::new(0).unwrap(),
                    entry: Some(entry("effect.delay")),
                },
                EventSource::Keyboard,
            )
            .unwrap();
        fixture.complete_pending_change(&mut output);
        audio.extend(output.iter().flat_map(|sample| sample.to_le_bytes()));
        fixture.counted_render(&mut output);
        audio.extend(output.iter().flat_map(|sample| sample.to_le_bytes()));
        (audio, fixture.app_loop.current_state_tree().into_json())
    }

    let (first_audio, first_state) = run();
    let (second_audio, second_state) = run();
    assert_eq!(first_audio, second_audio);
    assert_eq!(first_state, second_state);
}

/// Returns how many downward steps separate the PATCH Utility entry row
/// (TrimGain) from `control` in the one declared five-row order.
fn utility_rows_from_entry(control: &PatchControlId) -> usize {
    let order = PatchControlId::utility_surface_descriptor();
    let index = |target: &PatchControlId| {
        order
            .iter()
            .position(|candidate| candidate == target)
            .expect("the declared Utility order hosts this row")
    };
    index(control) - index(&PatchControlId::Output(PatchOutputParameter::TrimGain))
}
