mod chord_library;
mod clock_runtime;
pub(crate) mod corridors;
mod demo;
mod expression;
mod external_expression;
mod file_dialog;
mod file_runtime;
#[cfg(test)]
mod fixture_authoring;
mod inventory;
mod midi_controls;
mod performance;
mod playback_timing;
use performance::PendingOpen;
#[cfg(test)]
mod mpe_tests;
pub(crate) mod resources;
mod scene;
mod transport;
#[cfg(test)]
mod witness;
use crate::library::{self, PinnedSound};
use crate::midi_output::MidiOutputService;
use crate::recovery::{RecoverySnapshot, RecoveryWorker};
// The single integration boundary. Tracker never imports Synth implementation types.
use crate::app::{App, CaptureAction, FileAction, Request, SynthPage, TransportScope, View};
use crate::persistence::{atomic_write, Document, LoadWorker, Loaded, SaveMode, SaveWorker};
use crest_synth::adapter::lock_free_audio_boundary::LockFreeControlHandle;
use crest_synth::control::{SemanticAction, SessionCommand};
use crest_synth::kernel::midi_message::{MidiMessage, MidiMessageKind};
use crest_synth::kernel::{MidiChannel, PatchId};
use crest_synth::shell::hosted_application::{ApplicationHost, HostClock};
use crest_synth::shell::hosted_synth::HostedSynthControl;
use crest_synth::shell::standalone_application::{production_application, ApplicationConfig};
use crest_synth::shell::webview::host_presentation::HostPresentation;
use crest_synth::shell::webview::TauriWebviewWindow;
use crest_synth::shell::{SessionDocumentMarker, SessionDocumentProjection};
use crest_synth::synth::MidiVoiceId;
use crest_tracker::composition::*;
#[cfg(test)]
use crest_tracker::editing::Editor;
use crest_tracker::editing::{Command, Occurrence};
use crest_tracker::playback::Owner;
use crest_tracker::playback::{Message, Playback, PlaybackBudget, Snapshot, TempoMap, TimedEvent};
use crest_tracker::recording::{CaptureInput, CaptureMessage, CaptureState, Recorder};
use crest_tracker::time::Beat;
use serde_json::Value;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

type Sound<'a> = HostedSynthControl<'a, LockFreeControlHandle>;

/// Current Synth-owned admission result; absence of an error means MPE is ready.
#[derive(serde::Serialize)]
struct RecordingDestination {
    instrument: InstrumentId,
    mpe_error: Option<String>,
}

type SoundPublication = (
    InstrumentId,
    u64,
    std::sync::mpsc::Receiver<Result<PinnedSound, String>>,
);
type ResolvedSound = (
    PinnedSound,
    crest_synth::control::saved_session::SoundDefinition,
);
type SoundLibraryRead = (
    InstrumentId,
    u64,
    std::sync::mpsc::Receiver<Result<ResolvedSound, String>>,
);

/// Identities outlive the release packet until all events at that time have
/// been delivered, including expression at the exact physical release time.
struct ScheduledVoices {
    entries: BTreeMap<Owner, (MidiVoiceId, Option<Beat>)>,
    next: u64,
    at: Option<Beat>,
}
fn poll_once<T>(
    receiver: &std::sync::mpsc::Receiver<Result<T, String>>,
) -> Option<Result<T, String>> {
    match receiver.try_recv() {
        Ok(result) => Some(result),
        Err(std::sync::mpsc::TryRecvError::Empty) => None,
        Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(Err(
            "Background operation disconnected before completion".into(),
        )),
    }
}
impl Default for ScheduledVoices {
    fn default() -> Self {
        Self {
            entries: BTreeMap::new(),
            next: 1,
            at: None,
        }
    }
}
impl ScheduledVoices {
    fn allocate(&mut self) -> Result<MidiVoiceId, String> {
        let id = MidiVoiceId::new(self.next).ok_or("Voice identity exhausted")?;
        self.next = self.next.checked_add(1).ok_or("Voice identity exhausted")?;
        Ok(id)
    }
    fn voice_for(&mut self, event: &TimedEvent) -> Result<Option<MidiVoiceId>, String> {
        if self.at != Some(event.at) {
            self.entries
                .retain(|_, (_, released)| released.is_none_or(|at| at >= event.at));
            self.at = Some(event.at);
        }
        match event.message {
            Message::NoteOn { .. } => {
                let id = self.allocate()?;
                self.entries.insert(event.owner.clone(), (id, None));
                Ok(Some(id))
            }
            Message::NoteOff { .. } => {
                let (id, released) = self
                    .entries
                    .get_mut(&event.owner)
                    .ok_or("Release has no owned attack")?;
                *released = Some(released.unwrap_or(event.at).max(event.at));
                Ok(Some(*id))
            }
            Message::ExpressionLifetime { duration } => {
                let (_, until) = self
                    .entries
                    .get_mut(&event.owner)
                    .ok_or("Expression lifetime has no owned attack")?;
                *until = Some(event.at.checked_add(duration).map_err(|e| e.to_string())?);
                Ok(None)
            }
            Message::EndExpression => {
                self.entries
                    .remove(&event.owner)
                    .ok_or("Expression end has no owned attack")?;
                Ok(None)
            }
            Message::Expression { .. } | Message::MasterExpression { .. } => self
                .entries
                .get(&event.owner)
                .map(|(id, _)| Some(*id))
                .ok_or_else(|| "Expression has no owned attack".into()),
            _ => Ok(None),
        }
    }
}

pub(crate) fn write_inventory() -> Result<(), Box<dyn std::error::Error>> {
    serde_json::to_writer_pretty(std::io::stdout().lock(), &inventory::capture()?)?;
    Ok(())
}

const DEFAULT_INSTRUMENT: &str = "instrument.mutable.plaits";

pub fn run(app: App) -> Result<(), Box<dyn std::error::Error>> {
    let app = Rc::new(RefCell::new(app));
    let runtime = Runtime::new(Rc::clone(&app))?;
    let projection = Rc::clone(&app);
    let events = Rc::clone(&app);
    let presentation = HostPresentation {
        projection: Rc::new(move || crate::workspace::project(&projection.borrow())),
        event: Rc::new(move |event| crate::workspace::event(&mut events.borrow_mut(), event)),
    };
    let window = TauriWebviewWindow::new("Crest Tracker").with_host(presentation);
    production_application(window, ApplicationConfig::default())?
        .with_default_session_blueprint(crest_synth::control::DefaultSessionBlueprint::new(
            crest_synth::synth::CapabilityId::new(DEFAULT_INSTRUMENT)?,
        ))
        .run_with_host(runtime)
        .map_err(Into::into)
}

pub fn run_demo() -> Result<(), Box<dyn std::error::Error>> {
    scene::run()
}

struct Runtime {
    app: Rc<RefCell<App>>,
    sticks: crate::workspace::sticks::Sticks,
    playback: Option<Playback>,
    sync: clock_runtime::ClockRuntime,
    tempo: TempoMap,
    anchor: u64,
    transport_start: Beat,
    transport_offset_frames: u64,
    transport_selection: transport::PlaybackSelection,
    chord_library_publishing: bool,
    chord_library_worker:
        Option<std::sync::mpsc::Receiver<Result<Vec<library::chords::PublishedChord>, String>>>,
    library_chords: Vec<library::chords::PublishedChord>,
    last_scheduled_frame: u64,
    last_clock: Option<HostClock>,
    graph_revision: u64,
    saved_sound: Option<Value>,
    initial_sound: Option<Value>,
    pending_open: Option<PendingOpen>,
    performance: Option<crest_tracker::performance::Performance>,
    saved_performance: Option<crest_tracker::performance::Document>,
    performance_playback: Option<performance::Bank>,
    performance_launch_grid: crate::app::PerformanceLaunchGrid,
    performance_live: Option<performance::Live>,
    performance_surface: Option<performance::Surface>,
    performance_edit_token: u64,
    performance_recovery: Option<performance::RecoveryCapture>,
    performance_preparing: Option<performance::PendingRecord>,
    loading: Option<(LoadWorker, PathBuf)>,
    resource_candidate: Option<resources::repair::Candidate>,
    repairing_resource: Option<std::sync::mpsc::Receiver<Result<Document, String>>>,
    replacement_dirty: bool,
    importing: bool,
    saver: SaveWorker,
    saving: bool,
    exporting: Option<std::sync::mpsc::Receiver<Result<(), String>>>,
    after_save: Option<FileAction>,
    dialog: Option<file_runtime::ActiveDialog>,
    dialog_token: u64,
    close_requested: bool,
    midi_controls: midi_controls::MidiControls,
    elapsed_since_capture: std::time::Duration,
    recorder: Recorder,
    capture_target: Option<Target>,
    capture_revision: Option<u64>,
    capture_overflow: u64,
    capture_monitor: bool,
    capture_end: Option<Beat>,
    capture_last: Beat,
    monitor_pending: Vec<MidiMessage>,
    monitor_cleanup: bool,
    phrase_loop: Option<Occurrence>,
    voices: ScheduledVoices,
    expression: expression::Ledger,
    external_expression: external_expression::Ledger,
    monitor_owned_pending: Vec<crest_tracker::mpe::MpeEvent>,
    monitor_owned: BTreeMap<u64, expression::VoiceState>,
    send_leases: BTreeMap<(Owner, u8), (crest_synth::real_time::SendLeaseId, Vec<PatchId>)>,
    next_send_lease: u64,
    outputs: MidiOutputService,
    output_error_count: u64,
    audio_error_counts: (u64, u64),
    pinned_sounds: Vec<PinnedSound>,
    resources: Vec<resources::Resource>,
    publishing: Option<SoundPublication>,
    adopting: Option<(InstrumentId, u64, PinnedSound)>,
    library_read: Option<SoundLibraryRead>,
    recovery_worker: RecoveryWorker,
    recovery_loading: Option<std::sync::mpsc::Receiver<Result<Option<RecoverySnapshot>, String>>>,
    recovery_available: Option<RecoverySnapshot>,
    recovery_load_error: Option<String>,
    recovering: Option<RecoverySnapshot>,
    recovery_preparing: Option<std::sync::mpsc::Receiver<Result<RecoverySnapshot, String>>>,
    capture_preparing: Option<std::sync::mpsc::Receiver<Result<Document, String>>>,
    capture_baseline: Option<Document>,
    checkpoint_dirty: bool,
    checkpoint_elapsed: std::time::Duration,
    checkpoint_generation: u64,
    checkpoint_saved: u64,
}
impl Runtime {
    fn new(app: Rc<RefCell<App>>) -> Result<Self, Box<dyn std::error::Error>> {
        let shared =
            crest_synth::adapter::filesystem_midi_input_preference::per_user_midi_config_directory(
            )
            .ok_or("User configuration directory unavailable")?;
        Self::with_storage(app, library::data_directory(), Some(shared))
    }
    fn with_data_directory(
        app: Rc<RefCell<App>>,
        directory: PathBuf,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::with_storage(app, directory, None)
    }
    fn with_storage(
        app: Rc<RefCell<App>>,
        directory: PathBuf,
        shared: Option<PathBuf>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let sync = clock_runtime::ClockRuntime::new(directory.clone(), shared)?;
        let recovery_worker = RecoveryWorker::new(directory.clone())?;
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let _ = send.send(RecoveryWorker::load(&directory));
        });
        Ok(Self {
            app,
            sticks: crate::workspace::sticks::Sticks::default(),
            playback: None,
            sync,
            tempo: TempoMap::new(crest_tracker::time::Tempo::DEFAULT),
            anchor: 0,
            transport_start: Beat::ZERO,
            transport_offset_frames: 0,
            transport_selection: transport::PlaybackSelection::Song,
            chord_library_worker: None,
            chord_library_publishing: false,
            library_chords: Vec::new(),
            last_scheduled_frame: 0,
            last_clock: None,
            graph_revision: 0,
            saved_sound: None,
            initial_sound: None,
            pending_open: None,
            performance: None,
            saved_performance: None,
            performance_playback: None,
            performance_launch_grid: crate::app::PerformanceLaunchGrid::default(),
            performance_live: None,
            performance_surface: None,
            performance_edit_token: 0,
            performance_recovery: None,
            performance_preparing: None,
            loading: None,
            resource_candidate: None,
            repairing_resource: None,
            replacement_dirty: false,
            importing: false,
            saver: SaveWorker::new(),
            saving: false,
            exporting: None,
            after_save: None,
            dialog: None,
            dialog_token: 0,
            close_requested: false,
            midi_controls: midi_controls::MidiControls::default(),
            elapsed_since_capture: std::time::Duration::ZERO,
            recorder: Recorder::default(),
            capture_target: None,
            capture_revision: None,
            capture_overflow: 0,
            capture_monitor: true,
            capture_end: None,
            capture_last: Beat::ZERO,
            monitor_pending: Vec::new(),
            monitor_cleanup: false,
            phrase_loop: None,
            voices: ScheduledVoices::default(),
            expression: expression::Ledger::default(),
            external_expression: external_expression::Ledger::default(),
            monitor_owned_pending: Vec::new(),
            monitor_owned: BTreeMap::new(),
            send_leases: BTreeMap::new(),
            next_send_lease: 1,
            outputs: MidiOutputService::new(65536)?,
            output_error_count: 0,
            audio_error_counts: (0, 0),
            pinned_sounds: Vec::new(),
            resources: Vec::new(),
            publishing: None,
            adopting: None,
            library_read: None,
            recovery_worker,
            recovery_loading: Some(receive),
            recovery_available: None,
            recovery_load_error: None,
            recovering: None,
            recovery_preparing: None,
            capture_preparing: None,
            capture_baseline: None,
            checkpoint_dirty: false,
            checkpoint_elapsed: std::time::Duration::ZERO,
            checkpoint_generation: 0,
            checkpoint_saved: 0,
        })
    }
    fn initialize(&mut self, sound: &mut Sound<'_>) -> Result<(), String> {
        if self.initial_sound.is_some() {
            return Ok(());
        }
        let saved = serde_json::to_value(sound.capture_saved_session().map_err(|v| v.to_string())?)
            .map_err(|v| v.to_string())?;
        self.initial_sound = Some(saved.clone());
        self.saved_sound = Some(saved);
        let mut app = self.app.borrow_mut();
        let mut song = app.editor.song().clone();
        for patch in sound.installed_patches() {
            let id = InstrumentId::new(song.next_identity().map_err(|v| v.to_string())?)
                .map_err(|v| v.to_string())?;
            song.instruments.insert(
                id,
                InstrumentUse {
                    name: patch.name().to_owned(),
                    resource: None,
                },
            );
            app.mappings.insert(id, patch.id().value());
        }
        app.replace_song(song).map_err(|v| v.to_string())?;
        Ok(())
    }
    fn stop(&mut self, sound: &mut Sound<'_>) -> Result<(), String> {
        self.stop_with_reason(sound, "Transport stopped; take retained")
    }
    fn stop_with_reason(&mut self, sound: &mut Sound<'_>, reason: &str) -> Result<(), String> {
        let result = self.stop_outputs(sound, reason);
        self.sync.stop();
        result
    }
    fn stop_outputs(&mut self, sound: &mut Sound<'_>, reason: &str) -> Result<(), String> {
        let performance_result = self.finish_performance_input(sound, false);
        let sound_result = sound.cancel_scheduled().map_err(|v| v.to_string());
        let output_result = self.outputs.cancel().map_err(|v| v.to_string());
        let capture_result = self.interrupt_capture(reason);
        self.monitor_cleanup = true;
        self.voices.entries.clear();
        self.expression.clear();
        self.external_expression.clear();
        self.send_leases.clear();
        self.playback = None;
        let performance_was_playing = self.performance_playback.take().is_some();
        let mut app = self.app.borrow_mut();
        app.playing = false;
        if app.status == "PLAYING" || performance_was_playing {
            app.status = "STOPPED".into();
        }
        app.play_row = None;
        app.play_occurrence = None;
        app.play_phrase_beat = None;
        app.play_heads.clear();
        app.play_beat = None;
        app.playback_revision = None;
        app.playback_pending = false;
        sound_result?;
        output_result?;
        performance_result?;
        capture_result
    }
    fn interrupt_capture(&mut self, reason: &str) -> Result<(), String> {
        if matches!(
            self.recorder.state(),
            CaptureState::Idle | CaptureState::Interrupted { .. }
        ) {
            return Ok(());
        }
        self.checkpoint_dirty = true;
        self.monitor_cleanup = true;
        self.capture_end = None;
        let at = self.capture_cutoff()?;
        self.recorder
            .interrupt(at, reason)
            .map_err(|e| e.to_string())
    }
    fn play(&mut self, sound: &mut Sound<'_>, clock: HostClock) -> Result<(), String> {
        self.play_from(
            sound,
            clock,
            transport::PlaybackSelection::Song,
            Beat::ZERO,
            self.phrase_loop,
        )
    }
    fn play_from(
        &mut self,
        sound: &mut Sound<'_>,
        clock: HostClock,
        selection: transport::PlaybackSelection,
        at: Beat,
        phrase_loop: Option<Occurrence>,
    ) -> Result<(), String> {
        if !self.sync.ready() {
            return Err("Wait for machine preferences to finish loading or saving".into());
        }
        if self.sync.external() {
            return Err("External clock selected; send Start or Continue from that input".into());
        }
        let anchor = playback_timing::launch_frame(clock)?;
        let app = self.app.borrow();
        expression::validate_song(sound, &app, &self.sync.preferences.mpe_outputs)?;
        let snapshot = selection
            .snapshot(&app)
            .map_err(|error| error.to_string())?;
        let mut tempo = TempoMap::new(app.editor.song().tempo);
        let mut playback = match phrase_loop {
            Some(path) => {
                Playback::new_looping(Arc::new(snapshot), path).map_err(|v| v.to_string())?
            }
            None => Playback::new(Arc::new(snapshot)),
        };
        for (position, change) in playback
            .seek_forward_tempos(at)
            .map_err(|error| error.to_string())?
        {
            tempo.insert(position, change);
        }
        let offset = tempo
            .frame_at(at, clock.sample_rate)
            .map_err(|error| error.to_string())?;
        drop(app);
        self.stop(sound)?;
        self.transport_selection = selection;
        self.transport_start = at;
        self.transport_offset_frames = offset;
        self.phrase_loop = phrase_loop;
        self.tempo = tempo;
        self.playback = Some(playback);
        self.anchor = anchor;
        self.last_scheduled_frame = self.anchor;
        self.graph_revision = sound.active_graph_revision().value();
        let mut app = self.app.borrow_mut();
        app.playing = true;
        app.status = "PLAYING".into();
        drop(app);
        if let Err(error) = self.start_clock_output(clock) {
            let _ = self.stop(sound);
            return Err(error);
        }
        self.app.borrow_mut().playback_fault = None;
        Ok(())
    }
    fn advance(&mut self, sound: &mut Sound<'_>, clock: HostClock) -> Result<(), String> {
        let Some(mut playback) = self.playback.take() else {
            return Ok(());
        };
        let result = self.advance_playback(sound, clock, &mut playback);
        if self.app.borrow().playing {
            self.playback = Some(playback);
        }
        result
    }
    fn advance_playback(
        &mut self,
        sound: &mut Sound<'_>,
        clock: HostClock,
        playback: &mut Playback,
    ) -> Result<(), String> {
        if sound.active_graph_revision().value() != self.graph_revision {
            return Err(
                "Sound graph changed; transport stopped safely. Press Play to continue.".into(),
            );
        }
        // The internal producer promised this complete half-open window to
        // audio. Once its horizon is behind the renderer, catching up would
        // collapse past attacks/releases into one callback and exhaust prepared
        // voices. Cancel owned output before emitting that overdue burst.
        if !self.sync.external()
            && self.frame_at_beat(playback.horizon(), clock)? < clock.audio.rendered_frames()
        {
            return Err("Playback missed its audio deadline; transport stopped safely. Press Play to restart.".into());
        }
        {
            let app = self.app.borrow();
            if app.editor.revision() != playback.revision() {
                match self.transport_selection.snapshot(&app) {
                    Ok(snapshot) => match expression::validate_song(
                        sound,
                        &app,
                        &self.sync.preferences.mpe_outputs,
                    ) {
                        Ok(()) => playback.adopt(Arc::new(snapshot)),
                        Err(error) => {
                            drop(app);
                            self.app.borrow_mut().status = format!("EDIT NOT ACTIVE: {error}");
                        }
                    },
                    Err(error) => {
                        drop(app);
                        self.app.borrow_mut().status = format!("EDIT NOT ACTIVE: {error}");
                    }
                }
            }
        }
        let end = if self.sync.external() {
            self.sync
                .timeline
                .latest()
                .ok_or("Clock timeline unavailable")?
                .0
                .max(playback.horizon())
        } else {
            let horizon_frame = self.musical_frame(playback_timing::horizon_frame(clock)?)?;
            self.tempo
                .beat_at_frame(horizon_frame, clock.sample_rate)
                .map_err(|e| e.to_string())?
                .max(playback.horizon())
        };
        let events = playback.window(end).map_err(|v| v.to_string())?;
        // Equal-time tempo winners follow lowest track order, then last slot.
        let mut tempo_track = std::collections::BTreeMap::new();
        for event in &events {
            if let Message::Tempo { tempo } = event.message {
                if self.sync.external() {
                    if !self.sync.tempo_ignored {
                        self.app.borrow_mut().status =
                            "EXTERNAL CLOCK OWNS TEMPO · Song tempo effects ignored".into();
                        self.sync.tempo_ignored = true;
                    }
                    continue;
                }
                let winner = tempo_track.entry(event.at).or_insert(event.owner.track);
                if event.owner.track <= *winner {
                    self.tempo.insert(event.at, tempo);
                    *winner = event.owner.track;
                }
            }
        }
        let prepared = self.expression.prepare(
            sound,
            &self.app.borrow(),
            &events,
            &self.sync.preferences.mpe_outputs,
        )?;
        let mut external_batches = BTreeMap::new();
        let mut external_at = None;
        for (event_index, event) in events.into_iter().enumerate() {
            if let Some(at) = external_at {
                if at != event.at {
                    self.flush_external_expression(at, &mut external_batches, clock)?;
                }
            }
            external_at = Some(event.at);
            if prepared.consumed.contains(&event_index) {
                continue;
            }
            let frame = self.frame_at_beat(event.at, clock)?;
            self.last_scheduled_frame = self.last_scheduled_frame.max(frame);
            let voice = self.voices.voice_for(&event)?;
            if let Target::Channel {
                endpoint: Some(endpoint),
                ..
            } = &event.target
            {
                let owned = matches!(
                    event.message,
                    Message::NoteOn { .. }
                        | Message::NoteOff { .. }
                        | Message::Expression { .. }
                        | Message::MasterExpression { .. }
                        | Message::ExpressionLifetime { .. }
                        | Message::EndExpression
                );
                if prepared.external.contains_key(&event_index)
                    || (owned && self.external_expression.contains(&event.owner))
                {
                    if let Some(action) = self.external_expression.event(
                        &event,
                        voice,
                        prepared.external.get(&event_index),
                    )? {
                        external_batches
                            .entry(endpoint.clone())
                            .or_insert_with(Vec::new)
                            .push(action);
                    }
                    continue;
                }
            }
            if let Some(initial) = prepared.initial.get(&event_index) {
                for patch in internal_targets(sound, &self.app.borrow(), &event.target)? {
                    sound
                        .schedule_initialized_voice(
                            frame,
                            patch,
                            voice.ok_or("Initialized attack has no voice identity")?,
                            *initial,
                        )
                        .map_err(|e| e.to_string())?;
                }
                continue;
            }
            match event.message {
                Message::Send { bus, amount, slot } => {
                    let id = crest_synth::real_time::SendLeaseId::new(self.next_send_lease)
                        .ok_or("Send identity exhausted")?;
                    self.next_send_lease = self
                        .next_send_lease
                        .checked_add(1)
                        .ok_or("Send identity exhausted")?;
                    let bus = crest_synth::mixer::bus_id::BusId::new(
                        u16::try_from(bus).map_err(|_| "Send bus is outside Synth range")?,
                    )
                    .map_err(|v| v.to_string())?;
                    let lease = crest_synth::real_time::SendLease::new(
                        id,
                        bus,
                        f32::from(amount) / 100.0,
                        u64::MAX,
                    )
                    .map_err(|v| v.to_string())?;
                    let patches = internal_targets(sound, &self.app.borrow(), &event.target)?;
                    for patch in &patches {
                        sound
                            .schedule_send_lease(frame, *patch, lease)
                            .map_err(|v| v.to_string())?;
                    }
                    self.send_leases.insert((event.owner, slot), (id, patches));
                }
                Message::EndSend { slot } => {
                    let (id, patches) = self
                        .send_leases
                        .remove(&(event.owner, slot))
                        .ok_or("Send release has no owned lease")?;
                    for patch in patches {
                        sound
                            .schedule_end_send_lease(frame, patch, id)
                            .map_err(|v| v.to_string())?;
                    }
                }
                _ => deliver(
                    sound,
                    &self.app.borrow(),
                    &mut self.outputs,
                    clock,
                    frame,
                    event,
                    voice,
                )?,
            }
        }
        if let Some(at) = external_at {
            self.flush_external_expression(at, &mut external_batches, clock)?;
        }
        self.send_clock_output(end, clock)?;
        let now = self.transport_beat(clock)?;
        let heads = playback.playheads_at(now).map_err(|e| e.to_string())?;
        // Native projections consume only the current audible position. Keeping
        // minutes of past rows makes transactional scheduling copy unused history.
        playback.discard_playheads_before(now);
        let audible_finished = heads.as_ref().is_some_and(Vec::is_empty);
        let mut app = self.app.borrow_mut();
        app.play_beat = Some(now);
        crate::workspace::follow_playback(&mut app, now);
        app.playback_revision = Some(playback.revision());
        app.playback_pending = playback.pending_revision();
        let selected = app.occurrence();
        let head = heads
            .as_ref()
            .and_then(|heads| heads.iter().find(|head| Some(head.occurrence) == selected));
        app.play_occurrence = head.map(|head| head.occurrence);
        app.play_phrase_beat = head.map(|head| head.local_beat);
        app.play_row = head.map(|head| head.row);
        app.play_heads = heads.unwrap_or_default();
        app.playback_scope = match self.transport_selection {
            transport::PlaybackSelection::Song => TransportScope::Song,
            transport::PlaybackSelection::Chain { .. } => TransportScope::Chain,
            transport::PlaybackSelection::Phrase(_) => TransportScope::Phrase,
        };
        app.playback_loop = self.phrase_loop;
        if playback.finished()
            && audible_finished
            && clock.audio.rendered_frames() >= self.last_scheduled_frame
        {
            app.playing = false;
            app.play_row = None;
            app.play_occurrence = None;
            app.play_phrase_beat = None;
            self.playback = None;
            app.status = "STOPPED".into();
            drop(app);
            self.stop(sound)?;
        }
        Ok(())
    }
    fn request(
        &mut self,
        sound: &mut Sound<'_>,
        clock: HostClock,
        request: Request,
    ) -> Result<(), String> {
        if self.close_requested {
            return Err("MIDI cleanup for close is still pending".into());
        }
        if self.dialog.is_some()
            && !matches!(
                &request,
                Request::FileBrowse { .. }
                    | Request::FileChoose { .. }
                    | Request::FileDecision { .. }
                    | Request::FileCancel { .. }
                    | Request::TextEntry { .. }
                    | Request::EndGesture
                    | Request::Stop
            )
        {
            return Err("Finish or cancel the file dialog before changing the Song".into());
        }
        if self.resource_candidate.is_some()
            && !matches!(
                &request,
                Request::RepairResource { .. }
                    | Request::FileBrowse { .. }
                    | Request::FileChoose { .. }
                    | Request::FileDecision { .. }
                    | Request::FileCancel { .. }
                    | Request::EndGesture
                    | Request::TextEntry { .. }
                    | Request::Stop
            )
        {
            return Err("Finish or cancel asset repair before changing the Song".into());
        }
        match request {
            Request::PerformanceInputMode { mode } => self.set_performance_input_mode(sound, mode),
            Request::PerformanceHarmony { edit } => self.edit_performance_harmony(edit),
            Request::PerformanceLaunchGrid { grid } => {
                self.performance_launch_grid = grid;
                Ok(())
            }
            Request::PerformanceClips { endpoint, pads } => {
                self.enter_performance_clips(sound, endpoint, pads)
            }
            Request::PerformancePlay => self.return_performance_to_play(sound),
            Request::PerformancePad { slot } => self.performance_pad(sound, clock, slot),
            Request::PerformanceEdit { slot } => self.open_performance_editor(sound, slot),
            Request::PerformanceNote {
                token,
                note,
                key,
                start,
                duration,
            } => self.edit_performance_note(token, note, key, start, duration),
            Request::PerformanceUndo { token } => self.undo_performance_edit(token),
            Request::PerformanceInput => self.enable_performance_input(sound, clock),
            Request::PerformanceRecord { slot } => self.record_performance(sound, clock, slot),
            Request::PerformanceCaptureRecent { slot } => {
                self.capture_recent_performance(sound, clock, slot)
            }
            Request::PerformanceUndoClipChange => self.undo_performance_clip_change(),
            Request::PerformanceFinish => self.finish_performance_input(sound, false),
            Request::PerformanceCancel => self.finish_performance_input(sound, true),
            Request::PerformanceAudition { slot } => self.audition_performance(sound, clock, slot),
            Request::FileBrowse { token, directory } => self.file_browse(token, directory),
            Request::FileChoose {
                token,
                path,
                overwrite,
            } => self.file_choose(token, path, overwrite),
            Request::FileDecision { token, decision } => {
                self.file_decision(sound, token, &decision)
            }
            Request::FileCancel { token } => self.file_cancel(token),
            request @ (Request::TextEntry { .. } | Request::EndGesture) => self
                .app
                .borrow_mut()
                .apply(request)
                .map_err(|v| v.to_string()),
            Request::Play => {
                if self.playback.is_some() {
                    self.stop(sound)
                } else {
                    self.phrase_loop = None;
                    self.play(sound, clock)
                }
            }
            Request::StartAt { scope, at } => self.start_at(sound, clock, scope, at),
            Request::BrowserFolders { sample, soundfont } => {
                for path in [&sample, &soundfont].into_iter().flatten() {
                    if !std::path::Path::new(path).is_absolute()
                        || !std::path::Path::new(path).is_dir()
                    {
                        return Err(format!("Starting folder is inaccessible: {path}"));
                    }
                }
                let mut preferences = self.sync.preferences.clone();
                preferences.sample_folder = sample;
                preferences.soundfont_folder = soundfont;
                let input_clock = preferences.clock.input.is_some();
                self.configure_preferences(sound, preferences, input_clock)
            }
            Request::Preferences {
                default_tempo,
                monitor,
                input_clock,
                output_clock,
                delay_ms,
                mpe_input,
            } => {
                let mut preferences = self.sync.preferences.clone();
                preferences.default_tempo = default_tempo;
                preferences.monitor = monitor;
                preferences.mpe_input = mpe_input;
                preferences.clock.output = output_clock;
                preferences.clock.delay_ms = delay_ms;
                self.configure_preferences(sound, preferences, input_clock)
            }
            Request::MpeOutputProfile { endpoint, zones } => {
                let mut preferences = self.sync.preferences.clone();
                preferences
                    .mpe_outputs
                    .retain(|profile| profile.endpoint != endpoint);
                if !zones.is_empty() {
                    preferences
                        .mpe_outputs
                        .push(crate::midi_output::MpeOutputProfile { endpoint, zones });
                }
                let input_clock = preferences.clock.input.is_some();
                self.configure_preferences(sound, preferences, input_clock)
            }
            Request::MidiControls { bindings } => {
                let mut preferences = self.sync.preferences.clone();
                preferences.midi_controls = bindings;
                let input_clock = preferences.clock.input.is_some();
                self.configure_preferences(sound, preferences, input_clock)
            }
            Request::RollBindings { bindings } => {
                bindings.validate()?;
                let mut preferences = self.sync.preferences.clone();
                preferences.roll_bindings = bindings;
                let input_clock = preferences.clock.input.is_some();
                self.configure_preferences(sound, preferences, input_clock)
            }
            Request::Recovery { action } => self.recovery_action(sound, &action),
            Request::RepairResource { index, action } => self.repair_resource(index, &action),
            Request::SoundLibrary { action, instrument } => {
                self.sound_library(sound, &action, instrument)
            }
            Request::MidiOutput { action, endpoint } => match action.as_str() {
                "discover" => self.outputs.discover(),
                "connect" => self
                    .outputs
                    .connect(endpoint.as_deref().ok_or("Choose MIDI output")?),
                "disconnect" => {
                    self.stop(sound)?;
                    self.outputs
                        .disconnect(endpoint.as_deref().ok_or("Choose MIDI output")?)
                }
                _ => return Err("Unknown MIDI output action".into()),
            }
            .map_err(|v| v.to_string()),
            Request::SynthAction { action } => {
                let surface = sound.projection().semantic_model().active_surface();
                if crate::workspace::return_from_synth(&mut self.app.borrow_mut(), &action, surface)
                {
                    Ok(())
                } else {
                    sound
                        .dispatch_action(action, crest_synth::control::EventSource::System)
                        .map_err(|e| e.to_string())
                }
            }
            Request::Stop => self.stop(sound),
            Request::Arm {
                mpe,
                instrument,
                endpoint,
                monitor,
                unique,
            } => {
                let channel = Channel::new(1).map_err(|e| e.to_string())?;
                let target = match (instrument, endpoint) {
                    (Some(id), None) => Target::Instrument { id, channel },
                    (None, Some(endpoint)) => Target::Channel {
                        endpoint: Some(endpoint),
                        channel,
                    },
                    _ => return Err("Choose exactly one recording destination".into()),
                };
                self.arm(sound, clock, target, monitor, unique, mpe)
            }
            Request::Capture { action } => {
                if self.performance_live.is_some() {
                    return Err(
                        "Stop LinnStrument performance input before Tracker phrase recording"
                            .into(),
                    );
                }
                self.capture_action(sound, clock, action)
            }
            Request::File { action } => {
                self.close_requested |= self.file(sound, action)?;
                Ok(())
            }
            Request::Example => self.file(sound, FileAction::Example).map(|_| ()),
            request => {
                if self.capture_baseline.is_some()
                    && (self.capture_preparing.is_some() || self.recorder.edit_guard().is_none())
                    && (!matches!(request, Request::Confirm)
                        || self.app.borrow().confirm_edits_song())
                    && !matches!(
                        request,
                        Request::View { .. }
                            | Request::SynthSettings
                            | Request::SynthPage { .. }
                            | Request::PitchDisplay { .. }
                            | Request::Focus { .. }
                            | Request::Move { .. }
                            | Request::Back
                            | Request::PreviewChord { .. }
                            | Request::PreviewEdit { .. }
                            | Request::TextEntry { .. }
                            | Request::ControlsFocus { .. }
                            | Request::ToggleExpanded { .. }
                            | Request::Locate { .. }
                            | Request::PreviewBars { .. }
                            | Request::PreviewStart { .. }
                            | Request::SelectEvents { .. }
                            | Request::SelectRange { .. }
                            | Request::PianoControl {
                                control: crate::app::PianoControl::Grid { .. }
                                    | crate::app::PianoControl::Value { .. }
                                    | crate::app::PianoControl::Cursor { .. }
                                    | crate::app::PianoControl::Navigate { .. },
                            }
                    )
                {
                    return Err(
                        "Commit or discard the retained take before editing the Song".into(),
                    );
                }
                if self.recovery_preparing.is_some()
                    || self.pending_open.is_some()
                    || self.loading.is_some()
                    || self.resource_candidate.is_some()
                    || self.adopting.is_some()
                    || self.library_read.is_some()
                    || self.publishing.is_some()
                {
                    return Err("Document replacement is still being prepared".into());
                }
                if let Request::ChordLibrary { action } = request {
                    return self.chord_library_action(action);
                }
                use crest_synth::shell::hosted_synth::HostedSynthPage;
                let synth_page = match request {
                    Request::SynthSettings
                    | Request::SynthPage {
                        page: SynthPage::Settings,
                    } => Some(HostedSynthPage::MidiSettings),
                    Request::SynthPage {
                        page: SynthPage::Instruments,
                    } => Some(HostedSynthPage::Instruments),
                    Request::SynthPage {
                        page: SynthPage::Mixer,
                    } => Some(HostedSynthPage::Mixer),
                    Request::SynthPage {
                        page: SynthPage::Sends,
                    } => Some(HostedSynthPage::Sends),
                    _ => None,
                };
                let request = if let Some(page) = synth_page {
                    sound
                        .show_page(page, crest_synth::control::EventSource::System)
                        .map_err(|error| error.to_string())?;
                    Request::View {
                        view: View::Instruments,
                    }
                } else {
                    request
                };
                self.app
                    .borrow_mut()
                    .apply(request)
                    .map_err(|v| v.to_string())
            }
        }
    }
    fn repair_resource(&mut self, index: usize, action: &str) -> Result<(), String> {
        if self.repairing_resource.is_some() || self.loading.is_some() {
            return Err("Resource verification is still running".into());
        }
        if action == "cancel" {
            self.resource_candidate = None;
            self.app.borrow_mut().resource_repair = Value::Null;
            self.app.borrow_mut().status = "OPEN CANCELED · Current Song retained".into();
            return Ok(());
        }
        if !matches!(action, "locate" | "replace") {
            return Err("Unknown resource repair action".into());
        }
        let candidate = self
            .resource_candidate
            .as_mut()
            .ok_or("No resource repair is pending")?;
        let resource = candidate
            .document
            .resources
            .get(index)
            .ok_or("Resource selection is stale")?;
        let title = format!(
            "{} {}",
            if action == "replace" {
                "Replace"
            } else {
                "Locate original"
            },
            resource.reference.locator()
        );
        let initial = self.file_initial_path("");
        self.show_file_browser(
            file_runtime::Purpose::Repair {
                index,
                replace: action == "replace",
            },
            title,
            file_dialog::Mode::Open,
            initial,
            &[],
        )
    }
    fn start_resource_repair(
        &mut self,
        index: usize,
        replace: bool,
        path: PathBuf,
    ) -> Result<(), String> {
        let candidate = self
            .resource_candidate
            .as_mut()
            .ok_or("No resource repair is pending")?;
        let document = candidate.document.clone();
        // Failed repair does not change content, but remembering explicit Replace
        // forces Save As even if a subsequent repair completes this candidate.
        candidate.replaced |= replace;
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let _ = send.send(resources::repair::repair(&document, index, &path, replace));
        });
        self.repairing_resource = Some(receive);
        self.app.borrow_mut().status = "VALIDATING SELECTED RESOURCE".into();
        Ok(())
    }
    fn file(&mut self, sound: &mut Sound<'_>, action: FileAction) -> Result<bool, String> {
        self.file_at(sound, action, None, false)
    }
    fn file_at(
        &mut self,
        sound: &mut Sound<'_>,
        action: FileAction,
        selected: Option<PathBuf>,
        approved: bool,
    ) -> Result<bool, String> {
        if self.dialog.is_some() {
            return Err("Finish or cancel the current file dialog".into());
        }
        if self.close_requested {
            return Err("MIDI cleanup for close is still pending".into());
        }
        if self.recovery_preparing.is_some()
            || self.exporting.is_some()
            || self.pending_open.is_some()
            || self.loading.is_some()
            || self.resource_candidate.is_some()
            || self.adopting.is_some()
            || self.library_read.is_some()
            || self.publishing.is_some()
        {
            return Err("Document replacement is still being prepared".into());
        }
        if self.capture_baseline.is_some() && matches!(action, FileAction::ExportSynth) {
            return Err("Finish the retained take before exporting sound".into());
        }
        match action {
            FileAction::Save | FileAction::SaveAs | FileAction::CollectAssets => {
                if self.saving {
                    return Err("Save already in progress".into());
                }
                let collect = matches!(action, FileAction::CollectAssets);
                let path = selected.or_else(|| {
                    if matches!(action, FileAction::Save) {
                        self.app.borrow().path.clone()
                    } else {
                        None
                    }
                });
                let Some(path) = path else {
                    self.stop(sound)?;
                    let (title, mode, name, extensions) = if collect {
                        (
                            "Collect Assets into a new folder",
                            file_dialog::Mode::NewFolder,
                            "Untitled.crest-songpack",
                            vec!["crest-songpack"],
                        )
                    } else {
                        (
                            "Save Song",
                            file_dialog::Mode::Save,
                            "Untitled.crest-song",
                            vec!["crest-song"],
                        )
                    };
                    let initial = if collect {
                        self.file_initial_path(name)
                    } else {
                        self.app
                            .borrow()
                            .path
                            .clone()
                            .unwrap_or_else(|| self.file_initial_path(name))
                    };
                    self.show_file_browser(
                        file_runtime::Purpose::File { action, approved },
                        title.into(),
                        mode,
                        initial,
                        &extensions,
                    )?;
                    return Ok(false);
                };
                let path = if collect {
                    path.join("song.crest-song")
                } else {
                    path
                };
                self.finalize_capture_for_save(sound)?;
                let synth =
                    serde_json::to_value(sound.capture_saved_session().map_err(|v| v.to_string())?)
                        .map_err(|v| v.to_string())?;
                let document = self.capture_document(synth)?;
                let mode = if collect {
                    SaveMode::Collect
                } else if matches!(action, FileAction::Save)
                    && document.resources.iter().any(|item| item.file.is_some())
                {
                    SaveMode::UpdateCollection
                } else {
                    SaveMode::Normal
                };
                self.saver.submit(path, document, mode)?;
                self.saving = true;
                self.app.borrow_mut().status = "SAVING".into();
            }
            FileAction::New
            | FileAction::Open
            | FileAction::Close
            | FileAction::Example
            | FileAction::PerformanceExample
            | FileAction::PerformanceSampleExample
            | FileAction::Corridors
            | FileAction::ImportMidi => {
                if self.saving {
                    return Err(
                        "Wait for the current save to finish before replacing this document".into(),
                    );
                }
                if !approved && !self.confirm_replacement(sound, action)? {
                    return Ok(false);
                }
                self.stop(sound)?;
                if matches!(action, FileAction::Open | FileAction::ImportMidi) && selected.is_none()
                {
                    self.show_file_browser(
                        file_runtime::Purpose::File {
                            action,
                            approved: true,
                        },
                        if matches!(action, FileAction::ImportMidi) {
                            "Import MIDI file"
                        } else {
                            "Open Song"
                        }
                        .into(),
                        file_dialog::Mode::Open,
                        self.file_initial_path(""),
                        if matches!(action, FileAction::ImportMidi) {
                            &["mid", "midi"]
                        } else {
                            &["crest-song"]
                        },
                    )?;
                    return Ok(false);
                }
                self.restore_performance_surface()?;
                self.discard_capture_for_replacement(sound)?;
                if matches!(action, FileAction::ImportMidi) {
                    self.loading = Some((
                        LoadWorker::midi(selected.ok_or("Choose a MIDI file")?),
                        PathBuf::new(),
                    ));
                    self.app.borrow_mut().status = "IMPORTING MIDI".into();
                    return Ok(false);
                }
                if matches!(action, FileAction::Corridors) {
                    self.loading = Some((LoadWorker::corridors(), PathBuf::new()));
                    self.app.borrow_mut().status = "IMPORTING CORRIDORS OF TIME".into();
                    return Ok(false);
                }
                if matches!(action, FileAction::Close) {
                    self.flush_monitor(sound)?;
                    if self.performance_recovery.is_some() {
                        self.clear_recovery()?;
                    }
                    self.close_requested = true;
                    self.app.borrow_mut().status = "CLOSING · Waiting for MIDI cleanup".into();
                    return Ok(false);
                }
                let (document, path) = if matches!(action, FileAction::Example) {
                    (demo::document()?, None)
                } else if matches!(action, FileAction::PerformanceExample) {
                    (performance::example_document()?, None)
                } else if matches!(action, FileAction::PerformanceSampleExample) {
                    (performance::sample_example_document()?, None)
                } else if matches!(action, FileAction::New) {
                    let synth = self
                        .initial_sound
                        .clone()
                        .ok_or("Sound engine is still starting")?;
                    (
                        Document::new(
                            Song {
                                tempo: self.sync.preferences.default_tempo,
                                ..Song::default()
                            },
                            synth,
                            Default::default(),
                        ),
                        None,
                    )
                } else {
                    let path = selected.ok_or("Open Song has no selected file")?;
                    self.loading = Some((LoadWorker::start(path.clone(), false), path));
                    self.app.borrow_mut().status = "READING SONG".into();
                    return Ok(false);
                };
                self.stage_document(sound, document, path)?;
                self.app.borrow_mut().status = "PREPARING SONG".into();
            }
            FileAction::ExportSynth => {
                self.stop(sound)?;
                let Some(path) = selected else {
                    self.show_file_browser(
                        file_runtime::Purpose::File { action, approved },
                        "Export Synth session".into(),
                        file_dialog::Mode::Save,
                        self.file_initial_path("Untitled.json"),
                        &["json"],
                    )?;
                    return Ok(false);
                };
                let saved = sound.capture_saved_session().map_err(|v| v.to_string())?;
                let bytes = saved.to_json().map_err(|v| v.to_string())?.into_bytes();
                let (send, receive) = std::sync::mpsc::sync_channel(1);
                std::thread::spawn(move || {
                    let _ = send.send(atomic_write(&path, &bytes));
                });
                self.exporting = Some(receive);
                self.app.borrow_mut().status = "EXPORTING SOUND".into();
            }
            FileAction::ImportSynth => {
                if !approved && !self.confirm_replacement(sound, action)? {
                    return Ok(false);
                }
                self.stop(sound)?;
                let Some(path) = selected else {
                    self.show_file_browser(
                        file_runtime::Purpose::File {
                            action,
                            approved: true,
                        },
                        "Import Synth session".into(),
                        file_dialog::Mode::Open,
                        self.file_initial_path(""),
                        &["json"],
                    )?;
                    return Ok(false);
                };
                self.discard_capture_for_replacement(sound)?;
                self.loading = Some((LoadWorker::start(path.clone(), true), path));
                self.app.borrow_mut().status = "READING SOUND".into();
            }
        }
        Ok(false)
    }
    fn capture_cutoff(&self) -> Result<Beat, String> {
        if self.sync.external() {
            let now = self
                .last_clock
                .and_then(|clock| self.transport_beat(clock).ok())
                .unwrap_or(self.capture_last);
            let after_input = if self.recorder.held_count() > 0 {
                self.capture_last
                    .checked_add(Beat::new(1, 1_000_000).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?
            } else {
                self.capture_last
            };
            return Ok(now.max(after_input));
        }
        let rate = self.last_clock.map_or(48000, |clock| clock.sample_rate);
        let now = self
            .last_clock
            .map_or(Ok(self.transport_offset_frames), |clock| {
                self.musical_frame(clock.audio.rendered_frames())
            })?;
        // A key pressed exactly at cutoff receives one positive sample of life.
        let after_input = self
            .tempo
            .frame_at(self.capture_last, rate)
            .map_err(|e| e.to_string())?
            .checked_add(u64::from(self.recorder.held_count() > 0))
            .ok_or("Capture cutoff overflow")?;
        self.tempo
            .beat_at_frame(now.max(after_input), rate)
            .map_err(|e| e.to_string())
    }
    fn finalize_capture_for_save(&mut self, sound: &mut Sound<'_>) -> Result<(), String> {
        self.finish_performance_input(sound, false)?;
        if self.capture_baseline.is_none() {
            return Ok(());
        }
        if self.capture_preparing.is_some() {
            return Err("Recording assets are still being verified; retry Save when Armed".into());
        }
        if self.recorder.draft().is_none() {
            return self.discard_capture_for_replacement(sound);
        }
        if !matches!(self.recorder.state(), CaptureState::Idle) {
            let cutoff = self.capture_cutoff()?;
            self.recorder.finish_at(cutoff).map_err(|e| e.to_string())?;
        }
        self.stop(sound)?;
        self.commit_take(sound)
    }
    fn discard_capture_for_replacement(&mut self, sound: &mut Sound<'_>) -> Result<(), String> {
        if self.capture_baseline.is_none() {
            return Ok(());
        }
        self.monitor_cleanup = true;
        self.flush_monitor(sound)?;
        self.recorder.discard();
        self.capture_target = None;
        self.capture_end = None;
        self.app.borrow_mut().capture_dirty = false;
        self.clear_recovery()
    }
    fn refresh_dirty(&self, sound: &Sound<'_>) -> Result<(), String> {
        let current =
            serde_json::to_value(sound.capture_saved_session().map_err(|v| v.to_string())?)
                .map_err(|v| v.to_string())?;
        self.app.borrow_mut().sound_dirty = self.saved_sound.as_ref() != Some(&current);
        self.app.borrow_mut().performance_dirty = self
            .performance_document()
            .map_or(true, |current| current != self.saved_performance);
        Ok(())
    }
    fn recovery_action(&mut self, sound: &mut Sound<'_>, action: &str) -> Result<(), String> {
        self.recovery_action_confirmed(sound, action, false)
    }
    fn recovery_action_confirmed(
        &mut self,
        sound: &mut Sound<'_>,
        action: &str,
        approved: bool,
    ) -> Result<(), String> {
        if self.capture_baseline.is_some()
            || self.performance_live.is_some()
            || self.saving
            || self.recovery_preparing.is_some()
            || self.pending_open.is_some()
            || self.loading.is_some()
            || self.resource_candidate.is_some()
            || self.adopting.is_some()
            || self.library_read.is_some()
            || self.publishing.is_some()
        {
            return Err("Finish the current document or recording operation first".into());
        }
        match action {
            "recover" => {
                if self.recovery_load_error.is_some() {
                    let directory = library::data_directory();
                    let (send, receive) = std::sync::mpsc::sync_channel(1);
                    std::thread::spawn(move || {
                        let _ = send.send(RecoveryWorker::load(&directory));
                    });
                    self.recovery_loading = Some(receive);
                    self.app.borrow_mut().status = "RETRYING RECOVERY READ".into();
                    return Ok(());
                }
                let snapshot = self
                    .recovery_available
                    .clone()
                    .ok_or("No interrupted recording is available")?;
                self.refresh_dirty(sound)?;
                if !approved && self.app.borrow().dirty() {
                    return self.show_recovery_prompt();
                }
                self.stop(sound)?;
                let (send, receive) = std::sync::mpsc::sync_channel(1);
                std::thread::spawn(move || {
                    let _ = send.send(snapshot.resolve_resources());
                });
                self.recovery_preparing = Some(receive);
                self.app.borrow_mut().status = "VERIFYING RECOVERY ASSETS".into();
            }
            "discard" => {
                self.clear_recovery()?;
                self.app.borrow_mut().status = "RECOVERY CHECKPOINT DISCARDED".into();
            }
            _ => return Err("Unknown recording recovery action".into()),
        }
        Ok(())
    }
    fn clear_recovery(&mut self) -> Result<(), String> {
        // A completed commit/discard must release the in-memory capture gate
        // even if disk cleanup fails. An old read can never resurrect it.
        self.recovery_loading = None;
        self.capture_preparing = None;
        self.checkpoint_dirty = false;
        self.capture_baseline = None;
        self.performance_recovery = None;
        self.app.borrow_mut().editor.set_recording_guard(None);
        self.recovery_available = None;
        self.recovery_load_error = None;
        match self.recovery_worker.delete() {
            Ok(generation) => {
                self.checkpoint_generation = generation;
                Ok(())
            }
            Err(error) => {
                self.recovery_load_error = Some(error.clone());
                Err(format!(
                    "Take finalized; recovery checkpoint cleanup failed: {error}"
                ))
            }
        }
    }
    fn poll_recovery(&mut self, clock: HostClock) -> Result<(), String> {
        self.poll_performance_recording()?;
        if let Some(result) = self.capture_preparing.as_ref().and_then(poll_once) {
            self.capture_preparing = None;
            match result {
                Ok(baseline) => {
                    self.resources = baseline.resources.clone();
                    self.capture_baseline = Some(baseline);
                    self.checkpoint_dirty = true;
                    self.app.borrow_mut().status = "ARMED · Assets verified · Record loops this phrase while other tracks play".into();
                }
                Err(error) => {
                    self.recorder.discard();
                    self.capture_target = None;
                    self.capture_baseline = None;
                    self.checkpoint_dirty = false;
                    self.app.borrow_mut().editor.set_recording_guard(None);
                    return Err(format!("Recording setup failed: {error}"));
                }
            }
        }
        if let Some(result) = self.recovery_loading.as_ref().and_then(poll_once) {
            self.recovery_loading = None;
            match result {
                Ok(snapshot) => {
                    self.recovery_available = snapshot;
                    self.recovery_load_error = None;
                }
                Err(error) => {
                    self.recovery_load_error = Some(error.clone());
                    self.app.borrow_mut().status =
                        format!("RECOVERY READ FAILED · {error} · Retry or discard from Settings");
                }
            }
            if self.recovery_available.is_some() {
                self.app.borrow_mut().status =
                    "INTERRUPTED RECORDING AVAILABLE · Recover from Settings".into();
            }
        }
        if let Some(result) = self.recovery_worker.poll() {
            match result {
                Ok(completed) => self.checkpoint_saved = completed.generation,
                Err(error) => {
                    self.recovery_load_error = Some(error.clone());
                    return Err(error);
                }
            }
        }
        self.checkpoint_elapsed += clock.elapsed;
        let performance_recording = self.performance_recovery.is_some()
            && self
                .performance_live
                .as_ref()
                .is_some_and(|live| live.recording() && live.fault.is_none());
        if self.capture_preparing.is_none()
            && (self.checkpoint_dirty || performance_recording)
            && self.checkpoint_elapsed >= std::time::Duration::from_millis(250)
        {
            self.checkpoint_elapsed = std::time::Duration::ZERO;
            if self.performance_recovery.is_some() {
                self.checkpoint_performance()?;
            } else if let Some(baseline) = self.capture_baseline.as_ref() {
                let snapshot = RecoverySnapshot::new(baseline.clone(), self.recorder.checkpoint())?;
                self.checkpoint_generation = self.recovery_worker.submit(snapshot)?;
            }
            self.checkpoint_dirty = false;
        }
        let mut app = self.app.borrow_mut();
        app.recovery = serde_json::json!({
            "available":self.recovery_available.is_some()||self.recovery_load_error.is_some(),
            "error":self.recovery_load_error,
            "pending":self.checkpoint_dirty||self.checkpoint_generation>self.checkpoint_saved,
            "generation":self.checkpoint_saved,
            "checkpointUnixMillis":self.recovery_available.as_ref().and_then(RecoverySnapshot::checkpoint_unix_millis),
            "baselineSong":self.recovery_available.as_ref().map(|snapshot| &snapshot.baseline().song.name),
            "baselineEvents":self.recovery_available.as_ref().map(|snapshot| snapshot.baseline().song.phrases.values().map(|phrase| phrase.events.len()).sum::<usize>()),
            "takeEvents":self.recovery_available.as_ref().map(RecoverySnapshot::take_events),
            "currentSong":app.editor.song().name,
            "currentEvents":app.editor.song().phrases.values().map(|phrase| phrase.events.len()).sum::<usize>(),
        });
        Ok(())
    }
    fn confirm_replacement(
        &mut self,
        sound: &mut Sound<'_>,
        action: FileAction,
    ) -> Result<bool, String> {
        self.refresh_dirty(sound)?;
        if !self.app.borrow().dirty() {
            return Ok(true);
        }
        self.show_replace_prompt(action)?;
        Ok(false)
    }
    fn poll_files(&mut self, sound: &mut Sound<'_>) -> Result<(), String> {
        if let Some(result) = self.recovery_preparing.as_ref().and_then(poll_once) {
            self.recovery_preparing = None;
            let snapshot = result?;
            let document = snapshot.baseline().clone();
            self.stage_document(sound, document, None)?;
            self.recovering = Some(snapshot);
            self.app.borrow_mut().status = "PREPARING RECORDED SONG AND SOUND".into();
        }
        if let Some(result) = self.exporting.as_ref().and_then(poll_once) {
            self.exporting = None;
            result?;
            self.app.borrow_mut().status = "EXPORTED SOUND".into();
        }
        if let Some(result) = self.publishing.as_ref().and_then(|(_, _, r)| poll_once(r)) {
            let (id, revision, _) = self.publishing.take().expect("publication was pending");
            self.pin_sound(id, revision, result?)?;
        }
        if let Some(result) = self
            .library_read
            .as_ref()
            .and_then(|(_, _, r)| poll_once(r))
        {
            let (id, revision, _) = self.library_read.take().expect("library read pending");
            let (pinned, definition) = result?;
            let patch = PatchId::new(
                *self
                    .app
                    .borrow()
                    .mappings
                    .get(&id)
                    .ok_or("Instrument mapping missing")?,
            )
            .map_err(|v| v.to_string())?;
            sound
                .request_install_sound_definition(patch, definition)
                .map_err(|v| v.to_string())?;
            self.adopting = Some((id, revision, pinned));
            self.app.borrow_mut().status = "PREPARING SOUND REVISION".into();
        }
        if self.adopting.is_some() {
            match sound.replacement_status() {
                Ok(true) => {
                    let (id, revision, pinned) = self.adopting.take().expect("adoption pending");
                    self.pin_sound(id, revision, pinned)?;
                    self.app.borrow_mut().sound_dirty = true;
                }
                Ok(false) => {}
                Err(error) => {
                    self.adopting = None;
                    return Err(error.to_string());
                }
            }
        }

        if let Some(result) = self.repairing_resource.as_ref().and_then(poll_once) {
            self.repairing_resource = None;
            let document = result?;
            let candidate = self
                .resource_candidate
                .as_mut()
                .ok_or("Resource candidate was canceled")?;
            candidate.document = document;
            self.app.borrow_mut().resource_repair = candidate.projection();
            // Reuse the same worker and acknowledged graph replacement path.
            self.loading = Some((
                LoadWorker::prepared(candidate.document.clone(), candidate.path.clone()),
                candidate.path.clone(),
            ));
            self.app.borrow_mut().status = "VERIFYING REPAIRED SONG".into();
        }

        if let Some(result) = self.loading.as_ref().and_then(|(worker, _)| worker.poll()) {
            let (_, path) = self.loading.take().expect("load was pending");
            let fresh_import = matches!(&result, Ok(Loaded::Imported(_)));
            let (document, path, importing) = match result? {
                Loaded::Imported(document) => (document, None, false),
                Loaded::Song(document) => (document, Some(path), false),
                Loaded::Resources { document, error } => {
                    let replaced = self.resource_candidate.as_ref().is_some_and(|c| c.replaced);
                    let mut candidate = resources::repair::retain(document, path, error)?;
                    candidate.replaced = replaced;
                    self.app.borrow_mut().resource_repair = candidate.projection();
                    self.app.borrow_mut().status =
                        "ASSETS NEED ATTENTION · Locate, Replace or Cancel".into();
                    self.resource_candidate = Some(candidate);
                    return Ok(());
                }
                Loaded::Synth(synth) => (
                    self.capture_document(synth)?,
                    self.app.borrow().path.clone(),
                    true,
                ),
            };
            let path = if let Some(candidate) = self.resource_candidate.take() {
                self.app.borrow_mut().resource_repair = Value::Null;
                self.replacement_dirty = candidate.replaced;
                if candidate.replaced {
                    None
                } else {
                    path
                }
            } else {
                path
            };
            self.stage_document(sound, document, path)?;
            self.importing = importing;
            self.replacement_dirty |= fresh_import;
        }

        if let Some(result) = self.saver.poll() {
            self.saving = false;
            let next = self.after_save.take();
            let result = result?;
            result.result?;
            self.saved_performance = result.document.performance;
            let recovery_cleanup = if self.performance_recovery.is_some()
                && self.performance_document()? == self.saved_performance
            {
                self.clear_recovery()
            } else {
                Ok(())
            };
            self.app.borrow_mut().performance_dirty =
                self.performance_document()? != self.saved_performance;
            self.saved_sound = Some(result.document.synth);
            self.resources = result.document.resources;
            let mut app = self.app.borrow_mut();
            app.path = Some(result.path);
            app.editor.mark_saved(result.document.song);
            app.status = "SAVED".into();
            drop(app);
            recovery_cleanup?;
            if let Some(next) = next {
                self.close_requested |= self.file(sound, next)?;
            }
        }
        if self.pending_open.is_some() {
            match sound.replacement_status() {
                Ok(true) => {
                    let PendingOpen {
                        document,
                        path,
                        performance,
                    } = self.pending_open.take().expect("pending checked");
                    if self.importing {
                        self.importing = false;
                        self.app.borrow_mut().sound_dirty = true;
                        self.app.borrow_mut().status = "IMPORTED SOUND".into();
                        self.synchronize_catalog(sound)?;
                        return Ok(());
                    }
                    // Graph activation has succeeded. Cleanup failure must not
                    // prevent publication of its matching musical document.
                    let recovery_cleanup =
                        if self.recovering.is_none() && self.performance_recovery.is_some() {
                            self.clear_recovery()
                        } else {
                            Ok(())
                        };
                    self.saved_performance = document.performance;
                    self.performance = performance;
                    self.app.borrow_mut().performance_dirty = false;
                    self.pinned_sounds = document.pinned_sounds;
                    self.resources = document.resources;
                    self.saved_sound = Some(document.synth);
                    let mut app = self.app.borrow_mut();
                    app.replace_song(document.song).map_err(|v| v.to_string())?;
                    app.mappings = document.installed_patches;
                    app.path = path;
                    app.view = View::Song;
                    app.row = 0;
                    app.slot = 0;
                    app.track = 0;
                    app.column = 0;
                    app.chain_row = 0;
                    app.status = "OPENED".into();
                    app.sound_dirty = self.replacement_dirty;
                    if self.replacement_dirty {
                        self.saved_sound = None;
                    }
                    self.replacement_dirty = false;
                    drop(app);
                    if let Some(snapshot) = self.recovering.take() {
                        let baseline = self.capture_document(
                            serde_json::to_value(
                                sound.capture_saved_session().map_err(|e| e.to_string())?,
                            )
                            .map_err(|e| e.to_string())?,
                        )?;
                        if self.restore_performance_recovery(&snapshot, &baseline)? {
                            return Ok(());
                        }
                        self.recorder = snapshot.restore(&baseline, &self.app.borrow().editor)?;
                        if let Some(draft) = self.recorder.draft() {
                            self.phrase_loop = Some(draft.occurrence);
                            let _ = self.app.borrow_mut().apply(Request::Locate {
                                row_at: None,
                                occurrence: draft.occurrence,
                                at: Beat::ZERO,
                                row: 0,
                                view: View::Phrase,
                            });
                        }
                        self.capture_baseline = Some(baseline);
                        self.capture_target = None;
                        self.capture_revision = None;
                        self.capture_last = Beat::ZERO;
                        self.recovery_available = None;
                        self.saved_sound = None;
                        let mut app = self.app.borrow_mut();
                        app.sound_dirty = true;
                        app.status =
                            "RECOVERED TAKE · Stopped. End held notes, then Commit or Discard"
                                .into();
                    } else {
                        self.synchronize_catalog(sound)?;
                    }
                    recovery_cleanup?;
                }
                Ok(false) => {}
                Err(error) => {
                    self.pending_open = None;
                    self.importing = false;
                    self.replacement_dirty = false;
                    self.recovering = None;
                    return Err(error.to_string());
                }
            }
        }
        Ok(())
    }
    fn pin_sound(
        &mut self,
        id: InstrumentId,
        revision: u64,
        pinned: PinnedSound,
    ) -> Result<(), String> {
        let mut app = self.app.borrow_mut();
        let mut instrument = app
            .editor
            .song()
            .instruments
            .get(&id)
            .ok_or("Instrument was removed")?
            .clone();
        instrument.resource = Some(ResourceRevision {
            resource: pinned.resource.clone(),
            revision: pinned.revision.clone(),
        });
        app.editor
            .apply(revision, Command::PutInstrument { id, instrument })
            .map_err(|v| v.to_string())?;
        if !self
            .pinned_sounds
            .iter()
            .any(|p| p.resource == pinned.resource && p.revision == pinned.revision)
        {
            self.pinned_sounds.push(pinned);
        }
        app.status = "SOUND REVISION PINNED · Other Songs remain unchanged".into();
        Ok(())
    }
    fn sound_library(
        &mut self,
        sound: &mut Sound<'_>,
        action: &str,
        id: InstrumentId,
    ) -> Result<(), String> {
        if self.adopting.is_some()
            || self.recovery_preparing.is_some()
            || self.pending_open.is_some()
            || self.loading.is_some()
            || self.resource_candidate.is_some()
            || self.library_read.is_some()
            || self.publishing.is_some()
            || self.capture_baseline.is_some()
        {
            return Err("Finish the pending document/take operation first".into());
        }
        let app = self.app.borrow();
        let patch = PatchId::new(*app.mappings.get(&id).ok_or("Instrument mapping missing")?)
            .map_err(|v| v.to_string())?;
        let revision = app.editor.revision();
        let resource = app
            .editor
            .song()
            .instruments
            .get(&id)
            .and_then(|i| i.resource.as_ref())
            .map(|r| r.resource.clone());
        drop(app);
        match action {
            "publish" => {
                let definition = sound
                    .capture_sound_definition(patch)
                    .map_err(|v| v.to_string())?;
                let (send, receive) = std::sync::mpsc::sync_channel(1);
                std::thread::spawn(move || {
                    let result = resources::freeze_definition(definition)
                        .and_then(|definition| {
                            serde_json::to_value(definition).map_err(|e| e.to_string())
                        })
                        .and_then(|definition| {
                            library::publish(
                                &library::data_directory().join("sounds"),
                                resource.as_deref(),
                                definition,
                            )
                        });
                    let _ = send.send(result);
                });
                self.publishing = Some((id, revision, receive));
                self.app.borrow_mut().status = "PUBLISHING SOUND REVISION".into();
            }
            "adopt" => {
                self.stop(sound)?;
                self.show_file_browser(
                    file_runtime::Purpose::Adopt {
                        instrument: id,
                        revision,
                    },
                    "Adopt sound revision".into(),
                    file_dialog::Mode::Open,
                    library::data_directory().join("sounds"),
                    &["crest-sound"],
                )?;
            }
            _ => return Err("Unknown sound library action".into()),
        }
        Ok(())
    }
    fn start_sound_adoption(
        &mut self,
        id: InstrumentId,
        revision: u64,
        path: PathBuf,
    ) -> Result<(), String> {
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        let resources = self.resources.clone();
        std::thread::spawn(move || {
            let result = library::read(&path).and_then(|pinned| {
                let definition = crest_synth::control::saved_session::SoundDefinition::from_json(
                    &pinned.definition.to_string(),
                )
                .map_err(|e| e.to_string())?;
                Ok((
                    pinned,
                    resources::resolve_definition(definition, &resources)?,
                ))
            });
            let _ = send.send(result);
        });
        self.library_read = Some((id, revision, receive));
        self.app.borrow_mut().status = "READING SOUND REVISION".into();
        Ok(())
    }
    fn arm(
        &mut self,
        sound: &mut Sound<'_>,
        clock: HostClock,
        target: Target,
        monitor: bool,
        unique: bool,
        mpe: bool,
    ) -> Result<(), String> {
        if !self.sync.ready() {
            return Err("Wait for machine preferences to finish loading or saving".into());
        }
        if self.capture_baseline.is_some() || self.recorder.draft().is_some() {
            return Err("Commit or discard the retained take first".into());
        }
        if self.performance_recovery.is_some() {
            return Err("Save the expressive performance before starting a tracker take".into());
        }
        if self.recovery_available.is_some()
            || self.recovery_load_error.is_some()
            || self.recovery_loading.is_some()
        {
            return Err("Review the interrupted recording in Settings first".into());
        }
        if self.recovery_preparing.is_some()
            || self.pending_open.is_some()
            || self.loading.is_some()
            || self.resource_candidate.is_some()
            || self.adopting.is_some()
            || self.library_read.is_some()
            || self.publishing.is_some()
        {
            return Err("Finish the current document operation first".into());
        }
        if clock.audio_clock.is_none() {
            return Err("Audio clock is not ready".into());
        }
        let revision = sound
            .active_midi_input_identity()
            .ok_or("Connect MIDI input in Instruments / Settings first")?
            .revision()
            .value();
        self.stop(sound)?;
        self.recorder.discard();
        self.synchronize_catalog(sound)?;
        let mut app = self.app.borrow_mut();
        match &target {
            Target::Instrument { id, .. } if !app.mappings.contains_key(id) => {
                return Err("Recording instrument is unresolved".into())
            }
            Target::Channel {
                endpoint: Some(endpoint),
                ..
            } => {
                if !self.outputs.snapshot().connected.contains(endpoint) {
                    return Err("Connect the exact MIDI destination before arming".into());
                }
                if !mpe {
                    return Err(
                        "External recording monitoring requires an explicit MPE profile".into(),
                    );
                }
            }
            Target::Channel { endpoint: None, .. } => {
                return Err("Choose an explicit recording destination".into())
            }
            _ => {}
        }
        let mpe_profile = if mpe {
            let profile = self
                .sync
                .preferences
                .mpe_input
                .clone()
                .ok_or("Configure an MPE source profile in Settings first")?;
            let identity = sound
                .active_midi_input_identity()
                .ok_or("MIDI input disconnected")?;
            if profile.source() != identity.identity().to_string() {
                return Err(
                    "MPE profile belongs to another MIDI input; select its exact source".into(),
                );
            }
            match &target {
                Target::Instrument { id, .. } => {
                    let patch = PatchId::new(app.mappings[id]).map_err(|e| e.to_string())?;
                    sound
                        .owned_expression_profile(patch)
                        .map_err(|e| e.to_string())?;
                }
                Target::Channel {
                    endpoint: Some(endpoint),
                    ..
                } => {
                    self.sync
                        .preferences
                        .mpe_outputs
                        .iter()
                        .find(|profile| &profile.endpoint == endpoint)
                        .ok_or("Configure this exact MPE output in MIDI Outputs first")?
                        .validate()?;
                }
                _ => return Err("MPE destination must be explicit".into()),
            }
            Some(profile)
        } else {
            None
        };
        // Capture can fail while Synth is replacing a graph. Resolve that before
        // Make Unique or installing a recorder, so failed setup leaves no live take.
        let synth_baseline =
            serde_json::to_value(sound.capture_saved_session().map_err(|v| v.to_string())?)
                .map_err(|v| v.to_string())?;
        if unique {
            let occurrence = app.occurrence().ok_or("Choose a phrase occurrence first")?;
            app.command(Command::MakeUnique { occurrence })
                .map_err(|v| v.to_string())?;
        }
        let occurrence = app.occurrence().ok_or("Choose a phrase occurrence first")?;
        if let Some(profile) = mpe_profile {
            self.recorder
                .arm_mpe(&app.editor, occurrence, profile)
                .map_err(|e| e.to_string())?;
        } else {
            self.recorder
                .arm(&app.editor, occurrence)
                .map_err(|e| e.to_string())?;
        }
        self.phrase_loop = Some(occurrence);
        self.capture_target = Some(target);
        self.capture_revision = Some(revision);
        self.capture_overflow = sound.midi_activity().map_or(0, |v| v.overflow_epoch());
        self.capture_monitor = monitor;
        self.capture_end = None;
        self.capture_last = Beat::ZERO;
        drop(app);
        let mut baseline = self.capture_document(synth_baseline)?;
        self.capture_baseline = Some(baseline.clone());
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let result = resources::prepare_save(&mut baseline, None).map(|_| baseline);
            let _ = send.send(result);
        });
        self.capture_preparing = Some(receive);
        self.checkpoint_dirty = false;
        self.app.borrow_mut().status = "PREPARING RECORDING · Verifying asset content".into();
        Ok(())
    }
    fn next_capture_start(&self, after: Beat) -> Result<Beat, String> {
        self.recorder
            .next_start_boundary(after)
            .map_err(|error| error.to_string())
    }
    fn capture_action(
        &mut self,
        sound: &mut Sound<'_>,
        clock: HostClock,
        action: CaptureAction,
    ) -> Result<(), String> {
        match action {
            CaptureAction::Record => {
                if self.capture_preparing.is_some() {
                    return Err("Recording assets are still being verified".into());
                }
                if !matches!(self.recorder.state(), CaptureState::Armed) {
                    return Err("Arm a phrase first".into());
                }
                if self.recorder.mpe_profile().is_some() && self.recorder.held_count() > 0 {
                    return Err("Release all MPE keys and pedals before Record".into());
                }
                // play() cleans an old transport; retain the explicit capture setup across it.
                let recorder = self.recorder.clone();
                let boundary = if self.sync.external() {
                    self.next_capture_start(
                        self.playback.as_ref().map_or(Beat::ZERO, Playback::horizon),
                    )?
                } else {
                    self.play(sound, clock)?;
                    self.flush_monitor(sound)?;
                    Beat::ZERO
                };
                self.recorder = recorder;
                self.recorder
                    .request_record(boundary)
                    .map_err(|v| v.to_string())?;
                self.app.borrow_mut().status = "WAITING FOR RECORD START".into();
            }
            CaptureAction::End => {
                if matches!(self.recorder.state(), CaptureState::Interrupted { .. }) {
                    let cutoff = self.capture_cutoff()?;
                    self.recorder.finish_at(cutoff).map_err(|v| v.to_string())?;
                    self.capture_end = None;
                    self.app.borrow_mut().status =
                        "RECOVERED TAKE READY · Commit or discard".into();
                } else {
                    let now = if self.playback.is_some() {
                        self.transport_beat(clock)?
                    } else {
                        self.capture_last
                    };
                    self.capture_end = Some(
                        self.recorder
                            .next_loop_boundary(now)
                            .map_err(|v| v.to_string())?,
                    );
                    self.app.borrow_mut().status = "ENDING TAKE AT LOOP BOUNDARY".into();
                }
            }
            CaptureAction::Commit => {
                self.commit_take(sound)?;
            }
            CaptureAction::SaveAsNewPhrase => {
                self.stop(sound)?;
                self.monitor_cleanup = true;
                self.flush_monitor(sound)?;
                let mut app = self.app.borrow_mut();
                let track = &app.editor.song().tracks[app.track];
                let (id, slot) = (
                    track.id,
                    track
                        .slots
                        .iter()
                        .position(Option::is_none)
                        .unwrap_or(track.slots.len()),
                );
                let occurrence = self
                    .recorder
                    .save_as_new_phrase(&mut app.editor, id, slot)
                    .map_err(|e| e.to_string())?;
                app.capture_dirty = false;
                app.apply(Request::Locate {
                    row_at: None,
                    occurrence,
                    at: Beat::ZERO,
                    row: 0,
                    view: View::Phrase,
                })
                .map_err(|e| e.to_string())?;
                app.status =
                    "TAKE RECOVERED INTO NEW PHRASE · Original preserved · One Undo".into();
                drop(app);
                self.capture_target = None;
                self.capture_end = None;
                self.clear_recovery()?;
            }
            CaptureAction::Discard => {
                self.monitor_cleanup = true;
                self.flush_monitor(sound)?;
                self.recorder.discard();
                self.app.borrow_mut().capture_dirty = false;
                self.capture_target = None;
                self.capture_end = None;
                self.clear_recovery()?;
                self.app.borrow_mut().status = "TAKE DISCARDED".into();
            }
        }
        self.checkpoint_dirty = self.capture_baseline.is_some();
        Ok(())
    }
    fn commit_take(&mut self, sound: &mut Sound<'_>) -> Result<(), String> {
        self.monitor_cleanup = true;
        self.flush_monitor(sound)?;
        self.publish_take()
    }
    fn publish_take(&mut self) -> Result<(), String> {
        // Validate the complete take and playback snapshot before consuming its
        // recovery draft or mutating the live editor.
        let mut editor = self.app.borrow().editor.clone();
        editor.set_recording_guard(None);
        let pending = self
            .recorder
            .prepare_commit(&editor)
            .map_err(|v| v.to_string())?;
        editor
            .apply(pending.expected_revision, pending.command)
            .map_err(|v| v.to_string())?;
        let snapshot =
            Snapshot::compile(editor.song(), editor.revision(), PlaybackBudget::default())
                .map_err(|v| v.to_string())?;
        self.app.borrow_mut().editor = editor;
        self.app.borrow_mut().capture_dirty = false;
        self.recorder.discard();
        if let Some(playback) = &mut self.playback {
            playback.adopt_at_phrase_boundary(Arc::new(snapshot));
        }
        self.capture_target = None;
        self.capture_end = None;
        self.app.borrow_mut().status = "TAKE COMMITTED · Replays at next unscheduled phrase boundary · One Undo restores previous phrase".into();
        self.clear_recovery()
    }
    fn queue_mpe_monitor(&mut self) -> Result<(), String> {
        let owned = self.recorder.take_mpe_monitor_events();
        if self.capture_monitor && !self.monitor_cleanup {
            if self.monitor_owned_pending.len().saturating_add(owned.len()) > 1024 {
                return Err("Owned monitor queue full; take retained".into());
            }
            self.monitor_owned_pending.extend(owned);
        }
        Ok(())
    }
    fn flush_monitor(&mut self, sound: &mut Sound<'_>) -> Result<(), String> {
        self.queue_mpe_monitor()?;
        if let Some(Target::Channel {
            endpoint: Some(endpoint),
            ..
        }) = self.capture_target.clone()
        {
            return self.flush_external_monitor(&endpoint);
        }
        if let Some(Target::Instrument { id, .. }) = self.capture_target.clone() {
            let patch = PatchId::new(
                *self
                    .app
                    .borrow()
                    .mappings
                    .get(&id)
                    .ok_or("Monitor mapping missing")?,
            )
            .map_err(|v| v.to_string())?;
            if self.monitor_cleanup {
                self.monitor_pending.clear();
                self.monitor_owned_pending.clear();
                self.monitor_owned.clear();
                self.recorder.take_mpe_monitor_events();
                sound.cancel_monitor(patch).map_err(|v| v.to_string())?;
                self.monitor_cleanup = false;
            } else {
                for message in std::mem::take(&mut self.monitor_pending) {
                    sound
                        .send_monitor_midi(patch, message)
                        .map_err(|v| v.to_string())?;
                }
                for event in std::mem::take(&mut self.monitor_owned_pending) {
                    use crest_synth::synth::MidiExpressionScope;
                    use crest_tracker::mpe::MpeEvent;
                    match event {
                        MpeEvent::Attack {
                            id,
                            pitch,
                            velocity,
                            provenance,
                            member,
                            master,
                            ..
                        } => {
                            let mut state = expression::VoiceState::new(pitch, provenance.member());
                            for control in member {
                                state.apply(control, false)?;
                            }
                            for control in master {
                                state.apply(control, true)?;
                            }
                            state.validate(sound, patch)?;
                            let voice = MidiVoiceId::new(id).ok_or("Invalid monitor identity")?;
                            sound
                                .send_initialized_monitor_voice(
                                    patch,
                                    voice,
                                    state.initial(velocity)?,
                                )
                                .map_err(|e| e.to_string())?;
                            self.monitor_owned.insert(id, state);
                        }
                        MpeEvent::Release { id, velocity, .. } => {
                            let state = self
                                .monitor_owned
                                .get(&id)
                                .ok_or("MPE monitor release has no attack")?;
                            sound
                                .send_owned_monitor_midi(
                                    patch,
                                    MidiVoiceId::new(id).ok_or("Invalid monitor identity")?,
                                    MidiExpressionScope::Member,
                                    expression::midi(
                                        state.channel,
                                        MidiMessageKind::NoteOff,
                                        state.pitch,
                                        velocity,
                                    )?,
                                )
                                .map_err(|e| e.to_string())?;
                        }
                        MpeEvent::Expression {
                            id,
                            master,
                            control,
                            ..
                        } => {
                            let state = self
                                .monitor_owned
                                .get_mut(&id)
                                .ok_or("MPE monitor expression has no attack")?;
                            let mut candidate = state.clone();
                            candidate.apply(control, master)?;
                            candidate.validate(sound, patch)?;
                            sound
                                .send_owned_monitor_midi(
                                    patch,
                                    MidiVoiceId::new(id).ok_or("Invalid monitor identity")?,
                                    if master {
                                        MidiExpressionScope::Master
                                    } else {
                                        MidiExpressionScope::Member
                                    },
                                    expression::control_midi(state.channel, control)?,
                                )
                                .map_err(|e| e.to_string())?;
                            *state = candidate;
                        }
                        MpeEvent::End { id, .. } => {
                            self.monitor_owned.remove(&id);
                        }
                    }
                }
            }
        }
        Ok(())
    }
    fn advance_capture(&mut self, sound: &mut Sound<'_>, clock: HostClock) -> Result<(), String> {
        let previous = self.recorder.state().clone();
        if self.capture_target.is_some()
            && !matches!(
                self.recorder.state(),
                CaptureState::Idle | CaptureState::Interrupted { .. }
            )
        {
            if sound
                .midi_activity()
                .is_some_and(|v| v.overflow_epoch() != self.capture_overflow)
            {
                self.interrupt_capture("MIDI input overflow; take retained")?;
            }
            if sound
                .active_midi_input_identity()
                .map(|v| v.revision().value())
                != self.capture_revision
            {
                self.interrupt_capture("MIDI input disconnected or changed")?;
            }
            if self.playback.is_some() {
                let now = self.transport_beat(clock)?;
                if self.sync.external() || clock.audio.rendered_frames() >= self.anchor {
                    self.recorder.advance(now);
                }
                if let Some(end) = self.capture_end.filter(|end| now >= *end) {
                    self.recorder.finish(end).map_err(|v| v.to_string())?;
                    self.capture_end = None;
                    self.commit_take(sound)?;
                }
            }
        }
        self.flush_monitor(sound)?;
        if self.recorder.state() != &previous && self.capture_baseline.is_some() {
            self.checkpoint_dirty = true;
        }
        self.project_capture(
            self.recording_destinations(sound),
            sound
                .midi_activity()
                .map(|activity| activity.accepted_count()),
        );
        Ok(())
    }
    fn recording_destinations(&self, sound: &Sound<'_>) -> Vec<RecordingDestination> {
        self.app
            .borrow()
            .mappings
            .iter()
            .map(|(&instrument, &patch)| RecordingDestination {
                instrument,
                mpe_error: PatchId::new(patch)
                    .map_err(|error| error.to_string())
                    .and_then(|patch| {
                        sound
                            .owned_expression_profile(patch)
                            .map(|_| ())
                            .map_err(|error| error.to_string())
                    })
                    .err(),
            })
            .collect()
    }
    fn project_capture(
        &self,
        destinations: Vec<RecordingDestination>,
        input_activity: Option<u64>,
    ) {
        let armed = self.recorder.armed_phrase().map(
            |(occurrence, phrase)| serde_json::json!({"occurrence":occurrence,"phrase":phrase}),
        );
        let capture = serde_json::json!({"state":self.recorder.state(),"armed":armed,"draft":self.recorder.draft(),"held":self.recorder.held_count(),"monitor":self.capture_monitor,"mpe":self.recorder.mpe_profile(),"target":self.capture_target,"ending":self.capture_end,"destinations":destinations,"inputActivity":input_activity});
        let mut app = self.app.borrow_mut();
        // Packets can start or interrupt recording before advance_capture runs.
        // Compare the last published state so those transitions also reach chrome.
        if app.capture["state"] != capture["state"] {
            match self.recorder.state() {
                CaptureState::Recording { .. } => app.status = "RECORDING · Raw timing".into(),
                CaptureState::Interrupted { reason } => {
                    app.status = format!("TAKE INTERRUPTED · {reason} · Commit or discard");
                }
                _ => {}
            }
        }
        app.capture_dirty = self.recorder.draft().is_some();
        app.capture = capture;
    }
    /// The editor guard already admitted only independent phrase edits. Keep the
    /// recorder and recovery document on that same revision before another input.
    fn adopt_capture_editor_revision(&mut self, previous_revision: u64) -> Result<(), String> {
        let app = self.app.borrow();
        if previous_revision == app.editor.revision() || self.capture_baseline.is_none() {
            return Ok(());
        }
        self.recorder
            .rebase_other_phrase_edits(&app.editor)
            .map_err(|error| error.to_string())?;
        self.capture_baseline
            .as_mut()
            .expect("baseline checked")
            .song = app.editor.song().clone();
        self.checkpoint_dirty = true;
        Ok(())
    }
    fn synchronize_catalog(&mut self, sound: &mut Sound<'_>) -> Result<(), String> {
        if self.capture_baseline.is_some() {
            return Ok(());
        }
        let mut app = self.app.borrow_mut();
        let mut song = app.editor.song().clone();
        let mut changed = false;
        for patch in sound.installed_patches() {
            if !app.mappings.values().any(|id| *id == patch.id().value()) {
                let id = InstrumentId::new(song.next_identity().map_err(|v| v.to_string())?)
                    .map_err(|v| v.to_string())?;
                song.instruments.insert(
                    id,
                    InstrumentUse {
                        name: patch.name().into(),
                        resource: None,
                    },
                );
                app.mappings.insert(id, patch.id().value());
                changed = true;
            }
        }
        if changed {
            if song.chains.is_empty() && !app.editor.dirty() {
                app.replace_song(song).map_err(|v| v.to_string())?;
            } else {
                let commands = song
                    .instruments
                    .into_iter()
                    .map(
                        |(id, instrument)| crest_tracker::editing::Command::PutInstrument {
                            id,
                            instrument,
                        },
                    )
                    .collect();
                app.command(crest_tracker::editing::Command::Batch { commands })
                    .map_err(|v| v.to_string())?;
            }
        }
        Ok(())
    }
}
impl Runtime {
    fn capture_midi_input(
        &mut self,
        event: crest_synth::control::PhysicalMidiEvent,
        clock: HostClock,
    ) -> bool {
        self.last_clock = Some(clock);
        let Some(target) = self.capture_target.clone() else {
            return false;
        };
        if !matches!(
            self.recorder.state(),
            CaptureState::Armed | CaptureState::Waiting { .. } | CaptureState::Recording { .. }
        ) {
            return false;
        }
        let result = (|| -> Result<(), String> {
            if Some(event.revision().value()) != self.capture_revision {
                return Err("MIDI input changed; take interrupted".into());
            }
            let mpe = self.recorder.mpe_profile().is_some();
            if self.capture_monitor && !mpe {
                if self.monitor_pending.len() >= 1024 {
                    return Err("Monitor queue full; take interrupted".into());
                }
                self.monitor_pending.push(event.message());
            }
            let armed = matches!(self.recorder.state(), CaptureState::Armed);
            if armed && !mpe {
                return Ok(());
            }
            let mut pre_roll = armed;
            let at = if armed {
                Beat::ZERO
            } else if self.sync.external() {
                let timestamp = event
                    .timestamp_micros()
                    .saturating_sub(self.sync.delay_micros());
                if self
                    .sync
                    .timeline
                    .earliest()
                    .is_none_or(|(_, first)| timestamp < first)
                {
                    if !mpe {
                        return Ok(());
                    }
                    pre_roll = true;
                    self.capture_last
                } else {
                    self.sync.capture_beat(event.timestamp_micros())?
                }
            } else {
                let frame = clock
                    .audio_clock
                    .and_then(|c| c.frame_at_midi_timestamp(event.timestamp_micros()))
                    .ok_or("Input/audio clock correlation unavailable")?;
                if frame < self.anchor {
                    if !mpe {
                        return Ok(());
                    }
                    pre_roll = true;
                    Beat::ZERO
                } else {
                    self.tempo
                        .beat_at_frame(self.musical_frame(frame)?, clock.sample_rate)
                        .map_err(|e| e.to_string())?
                }
            };
            if self.capture_end.is_some_and(|end| at >= end) {
                return Ok(());
            }
            let midi = event.message();
            let message = match midi.kind() {
                MidiMessageKind::NoteOn => CaptureMessage::NoteOn {
                    pitch: midi.data1(),
                    velocity: midi.data2(),
                },
                MidiMessageKind::NoteOff => CaptureMessage::NoteOff {
                    pitch: midi.data1(),
                    velocity: midi.data2(),
                },
                MidiMessageKind::PitchBend => CaptureMessage::Control(Control::Bend {
                    value: u16::from(midi.data1()) | u16::from(midi.data2()) << 7,
                }),
                MidiMessageKind::ChannelPressure => CaptureMessage::Control(Control::Pressure {
                    value: midi.data1(),
                }),
                MidiMessageKind::PolyPressure => CaptureMessage::Control(Control::PolyPressure {
                    pitch: midi.data1(),
                    value: midi.data2(),
                }),
                MidiMessageKind::ControlChange => CaptureMessage::Control(Control::Cc {
                    number: midi.data1(),
                    value: midi.data2(),
                }),
                MidiMessageKind::ProgramChange => CaptureMessage::Control(Control::Program {
                    value: midi.data1(),
                }),
                MidiMessageKind::AllNotesOff => {
                    return Err("MIDI source requested recovery; take retained".into())
                }
            };
            let channel = Channel::new(midi.channel().value() + 1).map_err(|v| v.to_string())?;
            let input = CaptureInput {
                at,
                source_channel: channel,
                target: match target {
                    Target::Instrument { id, .. } => Target::Instrument { id, channel },
                    Target::Channel { endpoint, .. } => Target::Channel { endpoint, channel },
                },
                message,
            };
            if pre_roll && mpe {
                self.recorder.prime_mpe(input).map_err(|e| e.to_string())?;
            } else {
                self.recorder.input(input).map_err(|e| e.to_string())?;
            }
            self.queue_mpe_monitor()?;
            self.capture_last = at;
            self.app.borrow_mut().capture_dirty = self.recorder.draft().is_some();
            self.checkpoint_dirty = true;
            Ok(())
        })();
        if let Err(error) = result {
            let interruption = self.interrupt_capture(&error);
            self.monitor_cleanup = true;
            self.checkpoint_dirty = true;
            self.app.borrow_mut().status = match interruption {
                Ok(()) => error,
                Err(cleanup) => format!("{error}; capture finalization failed: {cleanup}"),
            };
        }
        true
    }
}
impl ApplicationHost<LockFreeControlHandle> for Runtime {
    fn tick(&mut self, sound: &mut Sound<'_>, clock: HostClock) {
        self.last_clock = Some(clock);
        self.sticks
            .advance(&mut self.app.borrow_mut(), clock.elapsed);
        let result = (|| -> Result<(), String> {
            self.initialize(sound)?;
            self.poll_file_dialog(sound)?;
            self.poll_files(sound)?;
            self.advance_clock(sound, clock)?;
            let failures = (
                clock.audio.routing_failures(),
                clock.audio.voice_limit_refusals(),
            );
            if failures != self.audio_error_counts {
                let routing = failures.0.saturating_sub(self.audio_error_counts.0);
                let voices = failures.1.saturating_sub(self.audio_error_counts.1);
                self.audio_error_counts = failures;
                return Err(format!("Sound rejected {routing} note/control messages; voice budget refused {voices} attacks; transport stopped"));
            }
            let outputs = self.outputs.snapshot();
            self.app.borrow_mut().outputs =
                serde_json::to_value(&outputs).map_err(|v| v.to_string())?;
            if outputs.error_count != self.output_error_count {
                self.output_error_count = outputs.error_count;
                self.close_requested = false;
                self.restore_performance_surface()?;
                if let Some(error) = outputs.last_error {
                    return Err(format!("MIDI OUTPUT: {}", error.message));
                }
            }
            let requests = std::mem::take(&mut self.app.borrow_mut().requests);
            for request in requests {
                let previous_revision = self.app.borrow().editor.revision();
                self.app
                    .borrow_mut()
                    .editor
                    .set_recording_guard(self.recorder.edit_guard());
                let requested = self.request(sound, clock, request);
                let captured = self.adopt_capture_editor_revision(previous_revision);
                if let Err(error) = requested.and(captured) {
                    self.app.borrow_mut().status = error;
                }
            }
            self.project_file_dialog();
            self.poll_chord_library();
            self.advance_capture(sound, clock)?;
            self.advance(sound, clock)?;
            self.advance_performance(sound, clock)?;
            self.advance_performance_surface(sound)?;
            self.poll_recovery(clock)?;
            self.elapsed_since_capture += clock.elapsed;
            if self.elapsed_since_capture >= std::time::Duration::from_millis(300) {
                self.elapsed_since_capture = std::time::Duration::ZERO;
                self.synchronize_catalog(sound)?;
                if let Ok(saved) = sound.capture_saved_session() {
                    let current = serde_json::to_value(saved).map_err(|v| v.to_string())?;
                    self.app.borrow_mut().sound_dirty = self.saved_sound.as_ref() != Some(&current);
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            let was_playing = self.app.borrow().playing;
            let _ = self.stop(sound);
            let mut app = self.app.borrow_mut();
            if was_playing {
                app.playback_fault = Some(error.clone());
            }
            app.status = error;
        }
        self.project_performance();
    }
    fn midi_transport(
        &mut self,
        event: crest_synth::control::PhysicalMidiTransportEvent,
        _clock: HostClock,
    ) {
        self.sync
            .receive(crest_synth::control::PhysicalMidiPacket::Transport(event));
    }
    fn midi_input(
        &mut self,
        event: crest_synth::control::PhysicalMidiEvent,
        clock: HostClock,
    ) -> bool {
        if self.performance_surface_input(event) {
            return true;
        }
        if self.performance_midi_input(event, clock) {
            return true;
        }
        if self.control_midi_input(event) {
            return true;
        }
        if self.sync.external()
            && self.capture_target.is_some()
            && matches!(
                self.recorder.state(),
                CaptureState::Armed | CaptureState::Waiting { .. } | CaptureState::Recording { .. }
            )
        {
            self.sync
                .receive(crest_synth::control::PhysicalMidiPacket::Performance(event));
            return true;
        }
        self.capture_midi_input(event, clock)
    }
    fn document(&self) -> SessionDocumentProjection {
        let app = self.app.borrow();
        SessionDocumentProjection::new(
            &app.editor.song().name,
            app.dirty(),
            if self.saving
                || self.exporting.is_some()
                || self.recovery_preparing.is_some()
                || self.pending_open.is_some()
                || self.loading.is_some()
                || self.resource_candidate.is_some()
                || self.adopting.is_some()
                || self.library_read.is_some()
                || self.publishing.is_some()
            {
                SessionDocumentMarker::Busy
            } else {
                SessionDocumentMarker::Ready
            },
            None,
            &app.status,
            None,
        )
    }
    fn session_command(&mut self, sound: &mut Sound<'_>, command: SessionCommand) -> bool {
        let action = match command {
            SessionCommand::New => FileAction::New,
            SessionCommand::Open => FileAction::Open,
            SessionCommand::Save => FileAction::Save,
            SessionCommand::SaveAs => FileAction::SaveAs,
            SessionCommand::Close => FileAction::Close,
        };
        match self.file(sound, action) {
            Ok(close) => close,
            Err(error) => {
                self.app.borrow_mut().status = error;
                false
            }
        }
    }
    fn controller_input(
        &mut self,
        input: crest_synth::shell::hosted_application::HostControllerInput,
        bindings: &crest_synth::control::ControllerBindings,
    ) -> bool {
        self.sticks.event(
            &mut self.app.borrow_mut(),
            input,
            bindings,
            &self.sync.preferences.roll_bindings,
        )
    }
    fn action(&mut self, action: SemanticAction) -> bool {
        crate::workspace::action(&mut self.app.borrow_mut(), action)
    }
    fn action_handler(&self) -> Option<Rc<dyn Fn(SemanticAction) -> bool>> {
        let app = Rc::clone(&self.app);
        Some(Rc::new(move |action| {
            crate::workspace::action(&mut app.borrow_mut(), action)
        }))
    }
    fn take_close_request(&mut self) -> bool {
        if !self.close_requested {
            return false;
        }
        let snapshot = self.outputs.snapshot();
        if snapshot.error_count != self.output_error_count || !snapshot.running {
            self.close_requested = false;
            self.app.borrow_mut().status = snapshot.last_error.map_or_else(
                || "MIDI output worker stopped before close cleanup".into(),
                |error| format!("CLOSE CANCELED · {}", error.message),
            );
            return false;
        }
        if snapshot.settled_epoch < snapshot.requested_epoch
            || snapshot.settled_surface_generation < snapshot.requested_surface_generation
        {
            return false;
        }
        if let Some(surface) = &self.performance_surface {
            if let Some(error) = &surface.fault {
                self.close_requested = false;
                self.app.borrow_mut().status = format!("CLOSE CANCELED · {error}");
            }
            return false;
        }
        std::mem::take(&mut self.close_requested)
    }
    fn owns_input(&self) -> bool {
        let app = self.app.borrow();
        crate::workspace::owns_input(&app) || self.dialog.is_some()
    }
}
fn deliver(
    sound: &mut Sound<'_>,
    app: &App,
    outputs: &mut MidiOutputService,
    clock: HostClock,
    frame: u64,
    event: TimedEvent,
    voice: Option<MidiVoiceId>,
) -> Result<(), String> {
    if matches!(
        event.message,
        Message::Tempo { .. } | Message::ExpressionLifetime { .. } | Message::EndExpression
    ) {
        return Ok(());
    }
    let scope = if matches!(event.message, Message::MasterExpression { .. }) {
        crest_synth::synth::MidiExpressionScope::Master
    } else {
        crest_synth::synth::MidiExpressionScope::Member
    };
    let (kind, data1, data2) = match event.message {
        Message::NoteOn { pitch, velocity } => (MidiMessageKind::NoteOn, pitch, velocity),
        Message::NoteOff { pitch, velocity } => (MidiMessageKind::NoteOff, pitch, velocity),
        Message::Control { control }
        | Message::Expression { control }
        | Message::MasterExpression { control } => match control {
            Control::Bend { value } => (
                MidiMessageKind::PitchBend,
                (value & 127) as u8,
                (value >> 7) as u8,
            ),
            Control::Pressure { value } => (MidiMessageKind::ChannelPressure, value, 0),
            Control::Cc { number, value } => (MidiMessageKind::ControlChange, number, value),
            Control::Program { value } => (MidiMessageKind::ProgramChange, value, 0),
            Control::PolyPressure { pitch, value } => (MidiMessageKind::PolyPressure, pitch, value),
        },
        Message::Send { .. } | Message::EndSend { .. } => {
            return Err("Send event must use the effective-control path".into())
        }
        Message::Tempo { .. } | Message::ExpressionLifetime { .. } | Message::EndExpression => {
            return Ok(())
        }
    };
    let channel = MidiChannel::new(event.target.channel().get() - 1).map_err(|v| v.to_string())?;
    let message = MidiMessage::try_new(channel, kind, data1, data2).map_err(|v| v.to_string())?;
    if let Target::Channel {
        endpoint: Some(endpoint),
        ..
    } = &event.target
    {
        if scope == crest_synth::synth::MidiExpressionScope::Master {
            return Err("External MPE requires a declared destination zone allocator".into());
        }
        let status = match kind {
            MidiMessageKind::NoteOff => 0x80,
            MidiMessageKind::NoteOn => 0x90,
            MidiMessageKind::PolyPressure => 0xa0,
            MidiMessageKind::ControlChange | MidiMessageKind::AllNotesOff => 0xb0,
            MidiMessageKind::ProgramChange => 0xc0,
            MidiMessageKind::ChannelPressure => 0xd0,
            MidiMessageKind::PitchBend => 0xe0,
        } | channel.value();
        let packet = [status, data1, data2];
        let length = if matches!(
            kind,
            MidiMessageKind::ProgramChange | MidiMessageKind::ChannelPressure
        ) {
            2
        } else {
            3
        };
        let deadline = deadline_for_frame(clock, frame)?;
        let result = match voice {
            Some(voice) => outputs.enqueue_voice(
                endpoint,
                &packet[..length],
                deadline,
                outputs.epoch(),
                voice.get(),
            ),
            None => outputs.enqueue(endpoint, &packet[..length], deadline, outputs.epoch()),
        };
        return result.map_err(|v| v.to_string());
    }
    let patches = internal_targets(sound, app, &event.target)?;
    for patch in patches {
        sound
            .schedule_scoped_voice_midi(frame, patch, message, voice, scope)
            .map_err(|v| v.to_string())?;
    }
    Ok(())
}
fn internal_targets(sound: &Sound<'_>, app: &App, target: &Target) -> Result<Vec<PatchId>, String> {
    let channel = MidiChannel::new(target.channel().get() - 1).map_err(|v| v.to_string())?;
    let patches = match target {
        Target::Instrument { id, .. } => vec![PatchId::new(
            *app.mappings
                .get(id)
                .ok_or("Instrument mapping is unresolved")?,
        )
        .map_err(|v| v.to_string())?],
        Target::Channel { endpoint: None, .. } => sound
            .installed_patches()
            .iter()
            .filter(|p| p.channel() == channel)
            .map(|p| p.id())
            .collect(),
        Target::Channel {
            endpoint: Some(_), ..
        } => return Err("Temporary sends require an internal Synth destination".into()),
    };
    if patches.is_empty() {
        return Err("No installed instrument subscribes to this MIDI channel".into());
    }
    Ok(patches)
}
#[cfg(target_os = "macos")]
fn host_micros() -> Result<u64, String> {
    #[repr(C)]
    struct Timebase {
        numer: u32,
        denom: u32,
    }
    unsafe extern "C" {
        fn mach_absolute_time() -> u64;
        fn mach_timebase_info(info: *mut Timebase) -> i32;
    }
    let mut base = Timebase { numer: 0, denom: 0 };
    // These Mach calls read a monotonic clock and its conversion, without product mutation.
    unsafe {
        if mach_timebase_info(&mut base) != 0 || base.denom == 0 {
            return Err("Host clock unavailable".into());
        }
        u64::try_from(
            u128::from(mach_absolute_time()) * u128::from(base.numer)
                / u128::from(base.denom)
                / 1000,
        )
        .map_err(|_| "Host clock overflow".into())
    }
}
#[cfg(not(target_os = "macos"))]
fn host_micros() -> Result<u64, String> {
    Err("External output clock correlation is not implemented on this platform".into())
}
/// Uses the backend audio timeline, including output latency. Shell receipt
/// time and wall-clock reads must not move an audible playhead or capture cutoff.
fn audio_timestamp_at_frame(clock: HostClock, frame: u64) -> Result<u64, String> {
    let audio = clock.audio_clock.ok_or("Audio timing is not ready")?;
    if audio.domain() != crest_synth::shell::audio_output::AudioClockDomain::CoreAudioHost {
        return Err("External clock requires shared input/audio timing".into());
    }
    let target = i128::from(audio.playback_micros())
        + (i128::from(frame) - i128::from(audio.frame_start())) * 1_000_000
            / i128::from(audio.sample_rate());
    u64::try_from(target).map_err(|_| "Audio timestamp overflow".into())
}
fn deadline_for_frame(clock: HostClock, frame: u64) -> Result<std::time::Instant, String> {
    struct OutputClockAnchor {
        instant: std::time::Instant,
        host_micros: u64,
    }
    // Both clocks are monotonic host clocks. Correlate their epochs once:
    // sampling/rounding again per packet gives identical musical timestamps
    // different deadlines and can reverse their required wire order.
    static ORIGIN: std::sync::OnceLock<Result<OutputClockAnchor, String>> =
        std::sync::OnceLock::new();
    let target = audio_timestamp_at_frame(clock, frame)?;
    let origin = ORIGIN
        .get_or_init(|| {
            let instant = std::time::Instant::now();
            Ok(OutputClockAnchor {
                instant,
                host_micros: host_micros()?,
            })
        })
        .as_ref()
        .map_err(Clone::clone)?;
    let deadline = if target >= origin.host_micros {
        origin.instant.checked_add(std::time::Duration::from_micros(
            target - origin.host_micros,
        ))
    } else {
        origin.instant.checked_sub(std::time::Duration::from_micros(
            origin.host_micros - target,
        ))
    };
    deadline.ok_or("Output deadline overflow".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crest_synth::control::{MidiConnectionRevision, PhysicalMidiEvent};
    use crest_synth::shell::audio_output::{AudioClockDomain, AudioFrameClock};

    #[test]
    fn voice_ledger_scopes_expression_but_never_raw_channel_controls() {
        let owner = Owner {
            track: TrackId::new(1).unwrap(),
            chain_use: UseId::new(2).unwrap(),
            phrase_use: UseId::new(3).unwrap(),
            cycle: 0,
            event: EventId::new(4).unwrap(),
            voice: 0,
        };
        let mut event = TimedEvent {
            at: Beat::ZERO,
            target: Target::Channel {
                channel: Channel::FIRST,
                endpoint: None,
            },
            owner,
            message: Message::NoteOn {
                pitch: 60,
                velocity: 100,
            },
        };
        let mut ledger = ScheduledVoices::default();
        let voice = ledger.voice_for(&event).unwrap().unwrap();
        event.message = Message::Control {
            control: Control::Bend { value: 9000 },
        };
        assert_eq!(ledger.voice_for(&event).unwrap(), None);
        event.message = Message::Expression {
            control: Control::Pressure { value: 70 },
        };
        assert_eq!(ledger.voice_for(&event).unwrap(), Some(voice));
        event.at = Beat::ONE;
        event.message = Message::NoteOff {
            pitch: 60,
            velocity: 43,
        };
        assert_eq!(ledger.voice_for(&event).unwrap(), Some(voice));
        event.message = Message::Expression {
            control: Control::Pressure { value: 0 },
        };
        assert_eq!(ledger.voice_for(&event).unwrap(), Some(voice));
        event.at = Beat::integer(2);
        assert!(ledger.voice_for(&event).is_err());
    }

    pub(super) fn capture_fixture() -> Runtime {
        let mut app = App::new();
        let instrument = InstrumentId::new(17).unwrap();
        app.command(Command::PutInstrument {
            id: instrument,
            instrument: InstrumentUse {
                name: "test".into(),
                resource: None,
            },
        })
        .unwrap();
        app.command(Command::AddPhrase {
            track: app.editor.song().tracks[0].id,
            slot: 0,
        })
        .unwrap();
        app.mappings.insert(instrument, 1);
        let app = Rc::new(RefCell::new(app));
        let mut runtime = Runtime::with_data_directory(
            app.clone(),
            std::env::temp_dir().join(format!("crest-host-test-{}", uuid::Uuid::new_v4())),
        )
        .unwrap();
        runtime
            .recorder
            .arm(&app.borrow().editor, app.borrow().occurrence().unwrap())
            .unwrap();
        runtime.recorder.request_record(Beat::ZERO).unwrap();
        runtime.capture_target = Some(Target::Instrument {
            id: instrument,
            channel: Channel::new(1).unwrap(),
        });
        runtime.capture_revision = Some(1);
        runtime.monitor_cleanup = false;
        runtime
    }
    #[test]
    fn external_record_start_keeps_armed_length_after_navigation() {
        let runtime = capture_fixture();
        let length = runtime
            .app
            .borrow()
            .selected_phrase()
            .unwrap()
            .1
            .length()
            .unwrap();
        let mut app = runtime.app.borrow_mut();
        app.track = 1;
        app.view = View::Song;
        assert!(app.selected_phrase().is_none());
        drop(app);
        assert_eq!(runtime.next_capture_start(Beat::ZERO).unwrap(), Beat::ZERO);
        assert_eq!(runtime.next_capture_start(Beat::ONE).unwrap(), length);
        assert_eq!(runtime.next_capture_start(length).unwrap(), length);
    }

    #[test]
    fn source_interruption_closes_held_note_at_audio_cutoff_and_keeps_commit_available() {
        use crest_synth::real_time::AudioObservationSnapshot;
        for (frames, duration) in [(48_000, Beat::ONE), (24_000, Beat::new(1, 24_000).unwrap())] {
            let mut runtime = capture_fixture();
            runtime.capture_monitor = true;
            runtime
                .capture_midi_input(packet(MidiMessageKind::NoteOn, 60, 101, 1_510_000), clock());
            assert_eq!(runtime.capture_last, Beat::ONE);
            assert_eq!(runtime.recorder.held_count(), 1);
            let mut cutoff = clock();
            cutoff.audio =
                AudioObservationSnapshot::from_mix(1, 1, frames, 0, 0, 0, Default::default());
            runtime.last_clock = Some(cutoff);
            runtime.capture_end = Some(Beat::integer(8));
            runtime
                .interrupt_capture("MIDI input disconnected or changed")
                .unwrap();
            assert_eq!(runtime.recorder.held_count(), 0);
            assert!(runtime.monitor_cleanup);
            assert!(runtime.checkpoint_dirty);
            assert_eq!(runtime.capture_end, None);
            let draft = runtime.recorder.draft().unwrap();
            let note = draft
                .events
                .iter()
                .find_map(|event| event.note.as_ref())
                .unwrap();
            assert_eq!(note.duration, duration);
            assert_eq!(note.velocity, 101);
            runtime
                .recorder
                .prepare_commit(&runtime.app.borrow().editor)
                .unwrap();
            runtime.project_capture(Vec::new(), None);
            assert_eq!(runtime.app.borrow().capture["held"], 0);
            assert!(runtime
                .app
                .borrow()
                .status
                .contains("MIDI input disconnected or changed"));
        }
    }

    #[test]
    fn capture_projection_keeps_armed_source_after_navigation_until_discard() {
        let mut runtime = capture_fixture();
        let (occurrence, phrase) = runtime.recorder.armed_phrase().unwrap();
        runtime.app.borrow_mut().track = 1;
        runtime.project_capture(Vec::new(), None);
        let expected = serde_json::json!({"occurrence":occurrence,"phrase":phrase});
        assert_eq!(runtime.app.borrow().capture["armed"], expected);
        runtime.recorder.advance(Beat::ZERO);
        runtime.project_capture(Vec::new(), None);
        assert_eq!(runtime.app.borrow().capture["armed"], expected);
        runtime
            .recorder
            .interrupt(Beat::ONE, "Disconnected")
            .unwrap();
        runtime.project_capture(Vec::new(), None);
        assert_eq!(runtime.app.borrow().capture["armed"], expected);
        runtime.recorder.discard();
        runtime.project_capture(Vec::new(), None);
        assert!(runtime.app.borrow().capture["armed"].is_null());
    }

    #[test]
    fn capture_status_publishes_packet_transitions_without_overwriting_later_messages() {
        let mut runtime = capture_fixture();
        runtime.app.borrow_mut().status = "WAITING FOR RECORD START".into();
        runtime.project_capture(Vec::new(), None);
        assert_eq!(runtime.app.borrow().status, "WAITING FOR RECORD START");
        assert!(runtime.midi_input(packet(MidiMessageKind::NoteOn, 60, 100, 1_510_000), clock()));
        runtime.project_capture(Vec::new(), None);
        assert_eq!(runtime.app.borrow().status, "RECORDING · Raw timing");
        runtime.app.borrow_mut().status = "ENDING TAKE AT LOOP BOUNDARY".into();
        runtime.project_capture(Vec::new(), None);
        assert_eq!(runtime.app.borrow().status, "ENDING TAKE AT LOOP BOUNDARY");
        runtime
            .recorder
            .interrupt(Beat::integer(2), "MIDI input disconnected or changed")
            .unwrap();
        runtime.project_capture(Vec::new(), None);
        assert_eq!(
            runtime.app.borrow().status,
            "TAKE INTERRUPTED · MIDI input disconnected or changed · Commit or discard"
        );
        assert!(runtime.app.borrow().capture_dirty);
        runtime.recorder.discard();
        runtime.app.borrow_mut().status = "TAKE DISCARDED".into();
        runtime.project_capture(Vec::new(), None);
        assert_eq!(runtime.app.borrow().status, "TAKE DISCARDED");
        assert!(!runtime.app.borrow().capture_dirty);
    }

    #[test]
    fn close_waits_for_output_cancellation_acknowledgment() {
        let mut runtime = capture_fixture();
        runtime.outputs.cancel().unwrap();
        runtime.close_requested = true;
        let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let snapshot = runtime.outputs.snapshot();
            if runtime.take_close_request() {
                let settled = runtime.outputs.snapshot();
                assert!(settled.settled_epoch >= settled.requested_epoch);
                break;
            }
            assert!(snapshot.settled_epoch < snapshot.requested_epoch);
            assert!(std::time::Instant::now() < until);
            std::thread::yield_now();
        }
        assert!(!runtime.take_close_request());
    }
    #[test]
    fn discarded_recovery_cannot_be_resurrected_by_an_old_reader() {
        let mut runtime = capture_fixture();
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        runtime.recovery_loading = Some(receive);
        runtime.capture_baseline = Some(Document::new(
            runtime.app.borrow().editor.song().clone(),
            serde_json::json!({}),
            Default::default(),
        ));
        runtime.checkpoint_dirty = true;
        runtime.clear_recovery().unwrap();
        assert!(send.send(Err("old read failed".into())).is_err());
        assert!(runtime.recovery_loading.is_none());
        assert!(runtime.capture_baseline.is_none());
        assert!(!runtime.checkpoint_dirty);
        assert!(runtime.recovery_load_error.is_none());
    }
    #[test]
    fn failed_capture_asset_verification_releases_editor_guard_immediately() {
        let mut runtime = capture_fixture();
        runtime
            .app
            .borrow_mut()
            .editor
            .set_recording_guard(runtime.recorder.edit_guard());
        runtime.capture_baseline = Some(Document::new(
            runtime.app.borrow().editor.song().clone(),
            serde_json::json!({}),
            Default::default(),
        ));
        let before = runtime.app.borrow().editor.song().clone();
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        runtime.capture_preparing = Some(receive);
        send.send(Err("required sample is unavailable".into()))
            .unwrap();
        assert!(runtime
            .poll_recovery(clock())
            .unwrap_err()
            .contains("required sample"));
        assert_eq!(runtime.recorder.state(), &CaptureState::Idle);
        assert!(runtime.capture_baseline.is_none());
        assert!(runtime.capture_preparing.is_none());
        assert!(runtime.capture_target.is_none());
        let mut app = runtime.app.borrow_mut();
        assert!(!app.editor.has_recording_guard());
        assert_eq!(app.editor.song(), &before);
        app.command(Command::RenameSong {
            name: "Still editable".into(),
        })
        .unwrap();
        app.editor.undo().unwrap();
        assert_eq!(app.editor.song(), &before);
    }

    pub(super) fn clock() -> HostClock {
        HostClock {
            elapsed: std::time::Duration::from_millis(10),
            control_micros: 9000000,
            sample_rate: 48000,
            audio: Default::default(),
            audio_clock: AudioFrameClock::new(
                0,
                256,
                1_000_000,
                1_010_000,
                48000,
                AudioClockDomain::CoreAudioHost,
            ),
        }
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn identical_audio_positions_have_identical_midi_output_deadlines() {
        let now = host_micros().unwrap();
        let mut clock = clock();
        clock.audio_clock = AudioFrameClock::new(
            0,
            256,
            now,
            now + 10_000,
            48_000,
            AudioClockDomain::CoreAudioHost,
        );
        let deadline = deadline_for_frame(clock, 4800).unwrap();
        for _ in 0..100 {
            assert_eq!(deadline_for_frame(clock, 4800).unwrap(), deadline);
        }
        let next = deadline_for_frame(clock, 9600).unwrap();
        assert_eq!(
            next.duration_since(deadline),
            std::time::Duration::from_millis(100)
        );
    }
    fn packet(kind: MidiMessageKind, a: u8, b: u8, timestamp: u64) -> PhysicalMidiEvent {
        PhysicalMidiEvent::new(
            MidiConnectionRevision::new(1).unwrap(),
            timestamp,
            MidiMessage::try_new(MidiChannel::new(4).unwrap(), kind, a, b).unwrap(),
        )
    }
    #[test]
    fn declared_mpe_capture_keeps_owned_monitor_events_and_sustained_raw_curves() {
        use crest_tracker::mpe::{BendRange, MpeEvent, MpeProfile, MpeZone};
        let mut runtime = capture_fixture();
        runtime.recorder.discard();
        let app = runtime.app.borrow();
        runtime
            .recorder
            .arm_mpe(
                &app.editor,
                app.occurrence().unwrap(),
                MpeProfile::new(
                    "fixture".into(),
                    MpeZone::lower(15).unwrap(),
                    BendRange::new(48, 0).unwrap(),
                    BendRange::new(2, 0).unwrap(),
                )
                .unwrap(),
            )
            .unwrap();
        drop(app);
        // Armed control primes the initial attack, with no fabricated pressure.
        assert!(runtime.midi_input(
            packet(MidiMessageKind::PitchBend, 17, 65, 1_010_000),
            clock()
        ));
        assert!(runtime.recorder.draft().is_none());
        runtime.recorder.request_record(Beat::ZERO).unwrap();
        assert!(runtime.midi_input(packet(MidiMessageKind::NoteOn, 60, 99, 1_510_000), clock()));
        let master = |value, timestamp| {
            PhysicalMidiEvent::new(
                MidiConnectionRevision::new(1).unwrap(),
                timestamp,
                MidiMessage::try_new(
                    MidiChannel::new(0).unwrap(),
                    MidiMessageKind::ControlChange,
                    64,
                    value,
                )
                .unwrap(),
            )
        };
        assert!(runtime.midi_input(master(127, 1_610_000), clock()));
        assert!(runtime.midi_input(packet(MidiMessageKind::NoteOff, 60, 41, 1_710_000), clock()));
        assert!(runtime.midi_input(
            packet(MidiMessageKind::ControlChange, 74, 87, 1_810_000),
            clock()
        ));
        assert!(runtime.midi_input(master(0, 1_910_000), clock()));
        assert!(runtime.monitor_pending.is_empty());
        assert!(runtime.monitor_owned_pending.iter().any(|event|matches!(event,MpeEvent::Attack{member,..} if member.contains(&Control::Bend{value:8337}))));
        let draft = runtime.recorder.draft().unwrap();
        let note = draft.events[0].note.as_ref().unwrap();
        assert_eq!(note.duration, Beat::new(2, 5).unwrap());
        assert_eq!(note.expression_duration(), Beat::new(4, 5).unwrap());
        assert_eq!(note.release_velocity, 41);
        assert!(note
            .expression
            .iter()
            .any(|point| point.offset == Beat::new(3, 5).unwrap()
                && point.control
                    == Control::Cc {
                        number: 74,
                        value: 87
                    }));
        assert_eq!(note.mpe.as_ref().unwrap().member().get(), 5);
        assert!(note.master_expression.iter().any(|point| point.control
            == Control::Cc {
                number: 64,
                value: 127
            }));
    }

    #[test]
    fn physical_capture_uses_device_timestamp_and_monitor_once() {
        let mut runtime = capture_fixture();
        assert!(runtime.midi_input(packet(MidiMessageKind::NoteOn, 61, 99, 1_510_000), clock()));
        assert!(runtime.midi_input(
            packet(MidiMessageKind::ControlChange, 74, 87, 1_610_000),
            clock()
        ));
        assert!(runtime.midi_input(packet(MidiMessageKind::NoteOff, 61, 43, 1_760_000), clock()));
        assert_eq!(runtime.monitor_pending.len(), 3);
        runtime.recorder.finish(Beat::integer(4)).unwrap();
        let draft = runtime.recorder.draft().unwrap();
        let note = draft.events.iter().find(|e| e.note.is_some()).unwrap();
        assert_eq!(note.at, Beat::ONE);
        assert_eq!(note.target.channel().get(), 5);
        assert_eq!(
            note.note.as_ref().unwrap().duration,
            Beat::new(1, 2).unwrap()
        );
        assert_eq!(note.note.as_ref().unwrap().release_velocity, 43);
        assert_eq!(
            draft
                .events
                .iter()
                .find(|e| !e.controls.is_empty())
                .unwrap()
                .controls,
            vec![Control::Cc {
                number: 74,
                value: 87
            }]
        );
        let previous = runtime.app.borrow().editor.song().clone();
        runtime
            .recorder
            .commit(&mut runtime.app.borrow_mut().editor)
            .unwrap();
        runtime.app.borrow_mut().editor.undo().unwrap();
        assert_eq!(runtime.app.borrow().editor.song(), &previous);
    }
    #[test]
    fn missing_clock_never_falls_back_to_ui_receipt_time() {
        let mut runtime = capture_fixture();
        let mut clock = clock();
        clock.audio_clock = None;
        assert!(runtime.midi_input(packet(MidiMessageKind::NoteOn, 60, 100, 1_510_000), clock));
        assert!(matches!(
            runtime.recorder.state(),
            CaptureState::Interrupted { .. }
        ));
        assert!(runtime.recorder.draft().is_none());
        assert!(runtime.monitor_cleanup);
    }
}
