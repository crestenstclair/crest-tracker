use crest_synth::adapter::production_instruments::production_capability_registry;
use crest_synth::control::app_event::{AppEvent, Direction};
use crest_synth::control::app_loop::AppLoop;
use crest_synth::control::app_state::AppState;
use crest_synth::control::event_log::EventLog;
use crest_synth::control::state_projector::StateProjector;
use crest_synth::control::{EngineSelectionStatusKind, TopLevelContext};
use crest_synth::kernel::midi_channel::MidiChannel;
use crest_synth::kernel::midi_message::{MidiMessage, MidiMessageKind};
use crest_synth::kernel::patch_id::PatchId;
use crest_synth::mixer::global_parameters::GlobalParameters;
use crest_synth::mixer::mixer_track_id::MixerTrackId;
use crest_synth::mixer::patch_output::PatchOutput;
use crest_synth::real_time::audio_boundary::{BoundaryFull, ControlAudioBoundary};
use crest_synth::real_time::audio_command::AudioCommand;
use crest_synth::real_time::parameter_snapshot::ParameterSnapshot;
use crest_synth::synth::patch::Patch;
use crest_synth::synth::sound_font_instrument::SoundFontInstrument;
use crest_synth::testing::automatic_midi_test::create_soundfont_config;
use std::time::{Duration, Instant};

const PATCH_COUNT: usize = 15;
const DISPATCH_COUNT: usize = 512;
const MAX_DISPATCH_DURATION: Duration = Duration::from_millis(50);

#[derive(Default)]
struct NoopBoundary {
    commands: usize,
    publications: usize,
}

impl ControlAudioBoundary for NoopBoundary {
    fn push_command(&mut self, _command: AudioCommand) -> Result<(), BoundaryFull> {
        self.commands += 1;
        Ok(())
    }

    fn publish_parameters(&mut self, _parameters: ParameterSnapshot) {
        self.publications += 1;
    }
}

fn globals() -> GlobalParameters {
    GlobalParameters::new(0.0).unwrap()
}

fn installed_state() -> AppState {
    let provider =
        crest_synth::adapter::production_instruments::production_soundfont_capability().unwrap();
    let mut state = AppState::new(production_capability_registry().unwrap(), globals());
    let patches = (0..PATCH_COUNT)
        .map(|index| {
            let id = PatchId::new(index as u32 + 1).unwrap();
            let channel = MidiChannel::new(index as u8).unwrap();
            Patch::new(
                id,
                format!("Performance Patch {}", index + 1),
                create_soundfont_config(
                    &provider,
                    SoundFontInstrument::new(0, index as u8, false).unwrap(),
                )
                .unwrap(),
                channel,
                PatchOutput::to_track(MixerTrackId::new(index as u8).unwrap()),
            )
        })
        .collect();
    state.apply(AppEvent::InstallPatches(patches)).unwrap();
    state
        .apply(AppEvent::SelectContext(TopLevelContext::Patch))
        .unwrap();
    state.apply(AppEvent::Adjust(Direction::Right)).unwrap();
    state
}

#[test]
fn fifteen_patch_midi_dispatch_uses_the_complete_production_control_path() {
    let state = installed_state();
    assert_eq!(
        state.engine_selection().kind(),
        EngineSelectionStatusKind::Loading
    );
    let initial_generation = state.generation();
    let mut eager_state = state.clone();
    let event_log = EventLog::new(DISPATCH_COUNT).unwrap();
    let mut app_loop = AppLoop::with_event_log(
        state,
        StateProjector::new(),
        NoopBoundary::default(),
        event_log,
    )
    .unwrap();
    let events = (0..DISPATCH_COUNT)
        .map(|sequence| {
            let patch_index = sequence % PATCH_COUNT;
            let patch_id = PatchId::new(patch_index as u32 + 1).unwrap();
            let channel = MidiChannel::new(patch_index as u8).unwrap();
            let kind = if sequence % 2 == 0 {
                MidiMessageKind::NoteOn
            } else {
                MidiMessageKind::NoteOff
            };
            let message =
                MidiMessage::try_new(channel, kind, 48 + (sequence % 24) as u8, 96).unwrap();
            AppEvent::Midi { patch_id, message }
        })
        .collect::<Vec<_>>();
    let started = Instant::now();

    for event in &events {
        app_loop.dispatch(event.clone()).unwrap();
    }

    let elapsed = started.elapsed();
    for event in events {
        eager_state.apply(event).unwrap();
    }
    let (eager_snapshot, eager_page, eager_text, eager_parameters, eager_tree) =
        StateProjector::new()
            .project_with_tree(&eager_state)
            .unwrap();
    let log = app_loop.event_log();
    assert_eq!(log.total_observed(), DISPATCH_COUNT as u64);
    assert_eq!(log.dropped_records(), 0);
    assert_eq!(
        app_loop.current_state_tree().generation(),
        initial_generation + DISPATCH_COUNT as u64
    );
    assert_eq!(
        app_loop.current_patch_page().unwrap().engine().status(),
        EngineSelectionStatusKind::Loading
    );
    assert_eq!(app_loop.current_patch_page(), eager_page);
    assert_eq!(app_loop.current_text(), eager_text);
    assert_eq!(app_loop.current_parameters(), &eager_parameters);
    assert_eq!(app_loop.current_state_tree(), eager_tree);
    assert_eq!(
        app_loop.current_state_tree().json().as_bytes(),
        eager_tree.json().as_bytes()
    );
    assert_eq!(
        app_loop.current_state_tree().state_hash(),
        eager_snapshot.hash()
    );
    assert!(
        elapsed <= MAX_DISPATCH_DURATION,
        "15-Patch MIDI dispatch took {elapsed:?}, exceeding {MAX_DISPATCH_DURATION:?}"
    );
    println!(
        "CREST_ACCEPTANCE control_dispatch_performance passed patches={PATCH_COUNT} events={DISPATCH_COUNT} elapsed_ms={} budget_ms={} events_per_second={:.1}",
        elapsed.as_millis(),
        MAX_DISPATCH_DURATION.as_millis(),
        DISPATCH_COUNT as f64 / elapsed.as_secs_f64()
    );
}
