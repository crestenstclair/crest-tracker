//! One event script for the visible demo and the production-DSP witness.
//! Actions enter the same workspace boundary as the UI. Observations only verify
//! results; this module has no controller/device API and never edits Song state.
mod features;
mod mixer;
mod piano;
mod song;
mod stress;
mod transport;
use super::*;
use crest_synth::control::{Direction, InteractionMode};
use crest_synth::real_time::AudioObservationSnapshot;
use crest_synth::shell::webview::host_presentation::WorkspaceEvent;
use serde::Serialize;
use std::time::{Duration, Instant};

#[derive(Default, Serialize)]
pub(super) struct Report {
    scene: &'static str,
    checkpoints: Vec<&'static str>,
    events: Vec<String>,
    notes: usize,
    instruments: usize,
    audible_tracks: [bool; 16],
    peak_track_rms: [f32; 16],
    peak_output_rms: f32,
    output_peak: f32,
    clipped_samples: u64,
    peak_voices: u32,
    final_voices: u32,
    last_beat: Option<Beat>,
    routing_failures: u64,
    voice_refusals: u64,
    nonfinite_samples: u64,
    late_events: u64,
    playback_recoveries: u64,
    complete: bool,
    error: Option<String>,
    cursor_positions_checked: usize,
    shared_phrase_offsets: bool,
    independent_song_cells: bool,
    mixer: Vec<mixer::Observation>,
    transport: Vec<transport::Observation>,
}

impl Report {
    pub(super) fn record_failure(&mut self, error: String) {
        self.complete = false;
        self.error = Some(error);
    }
}

enum Event {
    CreateCursorDemo,
    FileName,
    AcceptSharedEdit,
    FilePath(std::path::PathBuf),
    Action(SemanticAction),
    Repeat(SemanticAction, usize),
    Activate(&'static str),
    Focus(&'static str),
    Submit(&'static str, &'static str),
    Decision(&'static str),
    Wait,
}
enum Check {
    Transport(transport::Checkpoint),
    Stress(stress::Checkpoint),
    Piano(piano::Checkpoint),
    Feature(features::Checkpoint),
    Song(song::Checkpoint),
    Mixer(mixer::Checkpoint),
    CursorsReady,
    PaddingPlayback { slot: usize, sounding: bool },
    CursorProgress(u64),
    Ready,
    Held(bool),
    Chain,
    Phrase,
    View(View),
    Panel(&'static str),
    Synth,
    Note(bool),
    FileMenu,
    Confirm,
    Retained,
    Imported,
    Playing,
    PeakVoices(u32),
    ReleasedBefore(Beat),
    FullPlayback,
    Stopped,
    Progress(u64),
    Finished,
}
struct Step {
    label: &'static str,
    event: Event,
    check: Check,
}

pub(super) struct Scene {
    steps: Vec<Step>,
    index: usize,
    issued: bool,
    elapsed: Duration,
    started: Instant,
    full_playback: bool,
    playback_timeout: u64,
    pub report: Report,
}

impl Scene {
    pub fn named(name: &str) -> Result<Self, String> {
        match name {
            "corridors" => Ok(Self::new()),
            "piano" => Ok(Self::piano()),
            "cursors" => Ok(Self::cursors()),
            "transport" => Ok(transport::scene()),
            _ => features::scene(name)
                .ok_or_else(|| format!("Unknown demo {name}. Use --list-demos")),
        }
    }

    fn script(name: &'static str, steps: Vec<(&'static str, Event, Check)>) -> Self {
        Self {
            steps: steps
                .into_iter()
                .map(|(label, event, check)| Step {
                    label,
                    event,
                    check,
                })
                .collect(),
            index: 0,
            issued: false,
            elapsed: Duration::ZERO,
            started: Instant::now(),
            full_playback: false,
            playback_timeout: 45,
            report: Report {
                scene: name,
                ..Report::default()
            },
        }
    }

    pub fn piano() -> Self {
        piano::scene()
    }

    pub fn cursors() -> Self {
        use Check::*;
        use Event::*;
        let mut scene = Self::new();
        scene.report.scene = "independent-track-cursors";
        scene.steps = vec![
            ("Application ready", Wait, Ready),
            (
                "Create ten short chains and one long chain",
                CreateCursorDemo,
                CursorsReady,
            ),
            (
                "Start independent tracks",
                Action(SemanticAction::PreviewStart),
                FullPlayback,
            ),
            (
                "Shared phrase plays at different local positions",
                Wait,
                CursorProgress(15),
            ),
            (
                "Track 0 slot 05 while Track 1 stays in slot 00",
                Wait,
                CursorProgress(55),
            ),
            (
                "Both tracks loop independently at 100 rows",
                Wait,
                CursorProgress(105),
            ),
            (
                "Stop and release every voice",
                Action(SemanticAction::PreviewStart),
                Stopped,
            ),
        ]
        .into_iter()
        .map(|(label, event, check)| Step {
            label,
            event,
            check,
        })
        .collect();
        scene
    }

    pub fn new() -> Self {
        use Check::*;
        use Direction::*;
        use Event::{Action, Activate, Decision, Wait};
        use SemanticAction::{Adjust, NavigatePage, PreviewStart, PreviewStop, SetInteractionMode};
        let down = || Action(SetInteractionMode(InteractionMode::Adjust));
        let up = || Action(SetInteractionMode(InteractionMode::Navigate));
        let steps = vec![
            ("Application ready", Wait, Ready),
            ("Hold Edit in Song", down(), Held(true)),
            ("Edit + Right creates chain", Action(Adjust(Right)), Chain),
            ("Release Edit in Song", up(), Held(false)),
            (
                "Shift + Right enters Chain",
                Action(NavigatePage(Right)),
                View(crate::app::View::Chain),
            ),
            ("Hold Edit in Chain", down(), Held(true)),
            ("Edit + Up creates phrase", Action(Adjust(Up)), Phrase),
            ("Release Edit in Chain", up(), Held(false)),
            (
                "Shift + Right enters Phrase",
                Action(NavigatePage(Right)),
                View(crate::app::View::Phrase),
            ),
            ("Hold Edit in Phrase", down(), Held(true)),
            ("Edit + Up creates note", Action(Adjust(Up)), Note(false)),
            ("Held Edit changes pitch", Action(Adjust(Up)), Note(true)),
            ("Release Edit in Phrase", up(), Held(false)),
            (
                "Shift + Left returns to Chain",
                Action(NavigatePage(Left)),
                View(crate::app::View::Chain),
            ),
            (
                "Shift + Left returns to Song",
                Action(NavigatePage(Left)),
                View(crate::app::View::Song),
            ),
            ("Open menu for Song File", Activate("menu"), Panel("files")),
            ("Open Song File", Activate("files"), FileMenu),
            (
                "Import protects unsaved edits",
                Activate("corridors"),
                Confirm,
            ),
            ("Cancel retains edited Song", Decision("cancel"), Retained),
            ("Request Corridors import", Activate("corridors"), Confirm),
            (
                "Discard only scene scratch Song",
                Decision("discard"),
                Imported,
            ),
            ("Start plays imported Song", Action(PreviewStart), Playing),
            (
                "Start release keeps playing",
                Action(PreviewStop),
                Progress(4),
            ),
            ("Start toggles Stop", Action(PreviewStart), Stopped),
            (
                "Restart full arrangement",
                Action(PreviewStart),
                FullPlayback,
            ),
            ("Song advances through first four bars", Wait, Progress(16)),
            (
                "Navigate Chain while playing",
                Action(NavigatePage(Right)),
                View(crate::app::View::Chain),
            ),
            (
                "Navigate imported Piano Roll",
                Action(NavigatePage(Right)),
                View(crate::app::View::Phrase),
            ),
            ("Playback survives Phrase navigation", Wait, Progress(24)),
            (
                "Return through Chain",
                Action(NavigatePage(Left)),
                View(crate::app::View::Chain),
            ),
            (
                "Return to Song during playback",
                Action(NavigatePage(Left)),
                View(crate::app::View::Song),
            ),
            (
                "Global Settings from Song",
                Action(SemanticAction::OpenMidiSettings),
                Panel("key_scale"),
            ),
            (
                "Close Global Settings",
                Activate("back"),
                View(crate::app::View::Song),
            ),
            (
                "Open main menu",
                Action(SemanticAction::Return),
                Panel("instruments"),
            ),
            (
                "Original Synth workspace remains available",
                Activate("instruments"),
                Synth,
            ),
            (
                "Shift + Down opens menu from Synth Overview",
                Action(NavigatePage(Down)),
                Panel("files"),
            ),
            (
                "Return to Song from main menu",
                Activate("home"),
                View(crate::app::View::Song),
            ),
            ("Complete arrangement and release voices", Wait, Finished),
        ]
        .into_iter()
        .map(|(label, event, check)| Step {
            label,
            event,
            check,
        })
        .collect();
        Self {
            steps,
            index: 0,
            issued: false,
            elapsed: Duration::ZERO,
            started: Instant::now(),
            full_playback: false,
            playback_timeout: 45,
            report: Report {
                scene: "corridors-of-time",
                ..Report::default()
            },
        }
    }

    pub fn advance(
        &mut self,
        runtime: &Runtime,
        clock: HostClock,
        projection: Option<&crest_synth::control::GraphicalShellProjection>,
    ) -> Result<bool, String> {
        let _timing = performance::span(Stage::SceneCheck);
        self.observe(clock.audio);
        self.report.playback_recoveries = runtime.playback_recoveries;
        if self.report.complete {
            return Ok(true);
        }
        if let Some(error) = &runtime.app.borrow().playback_fault {
            return Err(error.clone());
        }
        if (
            self.report.routing_failures,
            self.report.voice_refusals,
            self.report.nonfinite_samples,
            self.report.late_events,
        ) != (0, 0, 0, 0)
        {
            return Err("Audio reported routing, voice, nonfinite, or scheduling failures".into());
        }
        if self.full_playback && self.report.scene == "independent-track-cursors" {
            self.check_cursors(&runtime.app.borrow())?;
        }
        let step = &self.steps[self.index];
        let limit = match &step.check {
            Check::Finished => 330,
            Check::FullPlayback => self.playback_timeout,
            Check::Song(song::Checkpoint::Finished {
                timeout_seconds, ..
            }) => *timeout_seconds,
            _ => 45,
        };
        if self.started.elapsed() > Duration::from_secs(limit) {
            return Err(format!(
                "Timed out at '{}': {} · {}",
                step.label,
                runtime.app.borrow().status,
                serde_json::to_string(&self.report).map_err(|e| e.to_string())?
            ));
        }
        if !self.issued {
            if matches!(step.check, Check::FullPlayback) {
                self.full_playback = true;
                self.report.audible_tracks = [false; 16];
                self.report.peak_track_rms = [0.0; 16];
                self.report.peak_output_rms = 0.0;
                self.report.output_peak = 0.0;
                self.report.last_beat = None;
            }
            dispatch(runtime, &step.event, &mut self.report.events)?;
            self.issued = true;
            self.elapsed = Duration::ZERO;
            return Ok(false);
        }
        self.elapsed += clock.elapsed;
        if self.report.scene.starts_with("stress-") && !runtime.app.borrow().requests.is_empty() {
            return Ok(false);
        }
        if self.full_playback {
            if let Some(beat) = runtime.app.borrow().play_beat {
                self.report.last_beat = Some(self.report.last_beat.unwrap_or(Beat::ZERO).max(beat));
            }
        }
        // A short control-clock dwell makes the same script legible in the real
        // window. Worker/graph/audio completion is always a state checkpoint.
        let dwell = match &step.check {
            _ if self.report.scene.starts_with("stress-") => Duration::from_millis(45),
            Check::Mixer(check) => check.dwell(),
            _ => Duration::from_millis(
                if matches!(
                    self.report.scene,
                    "corridors-of-time" | "independent-track-cursors"
                ) {
                    160
                } else {
                    400
                },
            ),
        };
        if self.elapsed < dwell || !self.check(runtime, &step.check, clock.audio, projection)? {
            return Ok(false);
        }
        // Keep checkpoint evidence in memory. Printing here can block the UI
        // on stderr/filesystem backpressure and invalidate the audio stress test.
        if let Check::Transport(_) = step.check {
            self.report.transport.push(transport::observe(runtime));
        }
        if let Check::Mixer(_) = step.check {
            self.report.mixer.push(mixer::observe(
                runtime,
                projection.ok_or("Missing Mixer projection")?,
                clock.audio,
            )?);
        }
        if matches!(
            step.check,
            Check::Imported | Check::CursorsReady | Check::FullPlayback
        ) {
            let app = runtime.app.borrow();
            // The finite imported-song scene already counted all placements,
            // including shared phrases and repeats. Live authoring scenes count
            // their stored notes instead; some of those scenes loop forever.
            if !matches!(self.report.scene, "imported-song" | "song-mixer") {
                self.report.notes = app
                    .editor
                    .song()
                    .phrases
                    .values()
                    .flat_map(|phrase| &phrase.events)
                    .filter(|event| event.note.is_some())
                    .count();
            }
            self.report.instruments = app.mappings.len();
        }
        self.report.checkpoints.push(step.label);
        self.index += 1;
        self.issued = false;
        self.started = Instant::now();
        self.report.complete = self.index == self.steps.len();
        Ok(self.report.complete)
    }

    fn observe(&mut self, audio: AudioObservationSnapshot) {
        self.report.peak_voices = self.report.peak_voices.max(audio.active_notes());
        self.report.final_voices = audio.active_notes();
        self.report.routing_failures = audio.routing_failures();
        self.report.voice_refusals = audio.voice_limit_refusals();
        self.report.nonfinite_samples = audio.non_finite_samples();
        self.report.late_events = audio.scheduled_late_events();
        self.report.clipped_samples = audio.clipped_samples();
        if self.full_playback {
            self.report.peak_output_rms = self.report.peak_output_rms.max(audio.output_rms());
            self.report.output_peak = self
                .report
                .output_peak
                .max(audio.left_peak())
                .max(audio.right_peak());
            for (index, track) in audio.tracks().into_iter().enumerate() {
                self.report.peak_track_rms[index] =
                    self.report.peak_track_rms[index].max(track.rms());
                self.report.audible_tracks[index] |= track.rms() > 0.000001;
            }
        }
    }

    fn check_cursors(&mut self, app: &App) -> Result<(), String> {
        if !app.playing || app.play_heads.is_empty() {
            return Ok(());
        }
        let song = app.editor.song();
        let step = app
            .play_beat
            .ok_or("missing audible time")?
            .units(Beat::new(1, 4).unwrap())
            .map_err(|e| e.to_string())?;
        let row = step % 100;
        if app.play_heads.len() != 2 {
            return Err("expected two independent playheads".into());
        }
        for (track_index, slot_index, phrase_index, local_row) in [
            (0, row as usize / 10, 0, row % 10),
            (
                1,
                0,
                if row < 5 {
                    0
                } else if row < 95 {
                    1
                } else {
                    2
                },
                if row < 5 {
                    row
                } else if row < 95 {
                    (row - 5) % 10
                } else {
                    row - 95
                },
            ),
        ] {
            let track = &song.tracks[track_index];
            let usage = track.slots[slot_index]
                .as_ref()
                .ok_or("missing demo slot")?;
            let phrase_use = &song.chains[&usage.chain.pattern().unwrap()].phrases[phrase_index];
            let head = app
                .play_heads
                .iter()
                .find(|head| head.occurrence.track == track.id)
                .ok_or("missing track cursor")?;
            if head.occurrence.chain_use != usage.id
                || head.occurrence.phrase_use != phrase_use.id
                || u64::from(head.row) != local_row
            {
                return Err(format!(
                    "Track {track_index} cursor mismatch at step {step}: {head:?}"
                ));
            }
        }
        self.report.cursor_positions_checked += 1;
        self.report.shared_phrase_offsets |= app.play_heads[0].phrase == app.play_heads[1].phrase
            && app.play_heads[0].row != app.play_heads[1].row;
        if app.view == View::Song {
            let projection = crate::workspace::project(app)
                .workspace
                .ok_or("missing Song workspace")?;
            let cells: Vec<_> = projection
                .grid
                .iter()
                .flat_map(|row| &row.cells)
                .filter(|cell| cell.playing)
                .map(|cell| cell.id.clone())
                .collect();
            let mut expected = vec![format!("cell:{}:0", row / 10), "cell:0:1".into()];
            expected.sort();
            let mut actual = cells;
            actual.sort();
            if actual != expected || projection.grid.iter().any(|row| row.playing) {
                return Err(format!(
                    "Song playhead cells disagree at step {step}: {actual:?}"
                ));
            }
            self.report.independent_song_cells |= row >= 10;
        }
        Ok(())
    }

    fn check(
        &self,
        runtime: &Runtime,
        check: &Check,
        audio: AudioObservationSnapshot,
        projection: Option<&crest_synth::control::GraphicalShellProjection>,
    ) -> Result<bool, String> {
        let app = runtime.app.borrow();
        let song = app.editor.song();
        let note_count = || {
            song.phrases
                .values()
                .flat_map(|phrase| &phrase.events)
                .filter(|event| event.note.is_some())
                .count()
        };
        Ok(match check {
            Check::Transport(check) => transport::check(runtime, &self.report, check, audio)?,
            Check::Stress(check) => stress::check(&app, check)?,
            Check::Piano(check) => piano::check(&app, check),
            Check::Feature(check) => features::check(&app, check),
            Check::Song(check) => song::check(runtime, &self.report, check, audio)?,
            Check::Mixer(check) => mixer::check(runtime, &self.report, check, audio, projection)?,
            Check::PaddingPlayback { slot, sounding } => {
                let track = &song.tracks[0];
                track
                    .slots
                    .get(*slot)
                    .and_then(Option::as_ref)
                    .is_some_and(|usage| {
                        app.playing
                            && app
                                .song_play_heads
                                .iter()
                                .any(|head| head.track == track.id && head.chain_use == usage.id)
                            && (audio.active_notes() > 0) == *sounding
                    })
            }
            Check::CursorsReady => {
                song.name == "Independent track cursors"
                    && song.tracks[0].slots.len() == 10
                    && song.tracks[1].slots.len() == 1
            }
            Check::CursorProgress(step) => {
                app.playing
                    && app
                        .play_beat
                        .is_some_and(|at| at >= Beat::new(*step, 4).unwrap())
                    && self.report.shared_phrase_offsets
                    && self.report.independent_song_cells
            }
            Check::Panel(id) => crate::workspace::project(&app).workspace.is_some_and(|v| {
                v.rows
                    .iter()
                    .chain(&v.tabs)
                    .chain(&v.inspector)
                    .any(|r| r.id == *id)
            }),
            Check::Synth => {
                app.view == View::Instruments && crate::workspace::project(&app).synth_visible()
            }
            Check::Ready => runtime.initial_sound.is_some() && runtime.sync.ready(),
            Check::Held(held) => app.edit_held == *held,
            Check::Chain => app.view == View::Song && app.selected_chain().is_some(),
            Check::Phrase => app.view == View::Chain && app.selected_phrase().is_some(),
            Check::View(view) => {
                app.view == *view
                    && crate::workspace::project(&app)
                        .workspace
                        .is_some_and(|view| !view.grid.is_empty())
            }
            Check::Note(changed) => app.selected_phrase().is_some_and(|(_, phrase)| {
                phrase.events.len() == 1
                    && phrase.events[0]
                        .note
                        .as_ref()
                        .is_some_and(|note| !changed || note.pitch > 60)
            }),
            Check::FileMenu => crate::workspace::project(&app)
                .workspace
                .is_some_and(|view| view.rows.iter().any(|row| row.id == "corridors")),
            Check::Confirm => app.file_dialog["mode"] == "confirm",
            Check::Retained => {
                app.file_dialog.is_null()
                    && song.name != crate::corridors::NAME
                    && note_count() == 1
                    && app.dirty()
            }
            Check::Imported => {
                if runtime.loading.is_some() || runtime.pending_open.is_some() {
                    return Ok(false);
                }
                if song.name != crate::corridors::NAME {
                    return Err(format!("Import failed: {}", app.status));
                }
                if song.instruments.len() != 21
                    || app.mappings.len() != 21
                    || note_count() != 24218
                    || !app.dirty()
                    || app.path.is_some()
                {
                    return Err(format!(
                        "Import checkpoint mismatch: {} notes, {} instruments, dirty {}",
                        note_count(),
                        song.instruments.len(),
                        app.dirty()
                    ));
                }
                true
            }
            Check::Playing => app.playing && audio.active_notes() > 0,
            Check::PeakVoices(count) => self.report.peak_voices >= *count,
            Check::ReleasedBefore(end) => {
                app.playing
                    && audio.active_notes() == 0
                    && app.play_beat.is_some_and(|at| at >= Beat::ONE && at < *end)
            }
            Check::FullPlayback => {
                app.playing && self.report.audible_tracks.iter().any(|audible| *audible)
            }
            Check::Stopped => !app.playing && audio.active_notes() == 0,
            Check::Progress(beat) => {
                app.playing && app.play_beat.is_some_and(|at| at >= Beat::integer(*beat))
            }
            Check::Finished => {
                if app.playing || audio.active_notes() != 0 {
                    return Ok(false);
                }
                if self
                    .report
                    .last_beat
                    .is_none_or(|at| at < Beat::integer(448))
                    || !self.report.audible_tracks.iter().all(|audible| *audible)
                    // Track meters are pre-master. Require useful final stereo
                    // output as well, so an attenuated/muted master cannot pass.
                    || self.report.peak_output_rms < 0.03
                    || self.report.output_peak > 1.0
                    || self.report.clipped_samples != 0
                {
                    return Err(format!(
                        "Incomplete playback evidence: {}",
                        serde_json::to_string(&self.report).map_err(|e| e.to_string())?
                    ));
                }
                true
            }
        })
    }
}

fn dispatch(runtime: &Runtime, event: &Event, log: &mut Vec<String>) -> Result<(), String> {
    let mut app = runtime.app.borrow_mut();
    match event {
        Event::CreateCursorDemo => {
            let instrument = *app
                .mappings
                .keys()
                .next()
                .ok_or("demo instrument is not ready")?;
            let command = crate::app::cursor_demo::arrangement(
                app.editor.song(),
                Target::instrument(instrument),
            )
            .map_err(|e| e.to_string())?;
            let revision = app.editor.revision();
            log.push("Edit(independent-track arrangement)".into());
            app.requests.push(Request::Edit {
                revision,
                command: Box::new(command),
            });
        }
        Event::AcceptSharedEdit => {
            if let Some(token) = app.shared_edit_projection()["token"].as_u64() {
                let id = format!("shared:{token}:shared");
                require_visible_action(&app, &id)?;
                log.push(format!("Activate({id})"));
                crate::workspace::input::enqueue(
                    &mut app,
                    crate::workspace::input::Input::Event(WorkspaceEvent::Activate { id }),
                );
            }
        }
        Event::FileName => {
            let dialog = &app.file_dialog;
            if !matches!(dialog["mode"].as_str(), Some("open" | "save")) {
                return Err("Scene expected a file browser".into());
            }
            let id = format!("dialog:{}:{}:filename", dialog["token"], dialog["version"]);
            require_visible_action(&app, &id)?;
            log.push(format!("Activate({id})"));
            crate::workspace::input::enqueue(
                &mut app,
                crate::workspace::input::Input::Event(WorkspaceEvent::Activate { id }),
            );
        }
        Event::FilePath(path) => {
            let token = app.file_dialog["token"]
                .as_u64()
                .ok_or("Scene expected a file dialog token")?;
            let id = format!("filename:{token}");
            let value = path.to_str().ok_or("Demo path must be UTF-8")?.to_owned();
            log.push(format!("Submit({id}, {value})"));
            crate::workspace::input::enqueue(
                &mut app,
                crate::workspace::input::Input::Event(WorkspaceEvent::Submit { id, value }),
            );
        }
        Event::Wait => {}
        Event::Action(action) => {
            dispatch_action(&mut app, action, log);
        }
        Event::Repeat(action, count) => {
            for _ in 0..*count {
                dispatch_action(&mut app, action, log);
            }
        }
        Event::Activate(id) => {
            require_visible_action(&app, id)?;
            log.push(format!("Activate({id})"));
            crate::workspace::input::enqueue(
                &mut app,
                crate::workspace::input::Input::Event(WorkspaceEvent::Activate {
                    id: (*id).into(),
                }),
            );
        }
        Event::Focus(id) => {
            require_visible_action(&app, id)?;
            log.push(format!("Focus({id})"));
            crate::workspace::input::enqueue(
                &mut app,
                crate::workspace::input::Input::Event(WorkspaceEvent::Focus { id: (*id).into() }),
            );
        }
        Event::Submit(id, value) => {
            log.push(format!("Submit({id}, {value})"));
            crate::workspace::input::enqueue(
                &mut app,
                crate::workspace::input::Input::Event(WorkspaceEvent::Submit {
                    id: (*id).into(),
                    value: (*value).into(),
                }),
            );
        }
        Event::Decision(choice) => {
            let dialog = &app.file_dialog;
            if dialog["mode"] != "confirm" {
                return Err("Scene expected replacement confirmation".into());
            }
            let id = format!("dialog:{}:{}:{choice}", dialog["token"], dialog["version"]);
            log.push(format!("Activate({id})"));
            crate::workspace::input::enqueue(
                &mut app,
                crate::workspace::input::Input::Event(WorkspaceEvent::Activate { id }),
            );
        }
    }
    Ok(())
}

fn dispatch_action(app: &mut App, action: &SemanticAction, log: &mut Vec<String>) {
    log.push(format!("{action:?}"));
    if !crate::workspace::input::action(app, action.clone()) {
        app.requests.push(Request::SynthAction {
            action: action.clone(),
        });
    }
}

fn require_visible_action(app: &App, id: &str) -> Result<(), String> {
    if matches!(id, "menu" | "home" | "instruments" | "transport") {
        return Ok(());
    }
    let view = crate::workspace::project(app)
        .workspace
        .ok_or("Scene expected a Tracker workspace")?;
    if view
        .rows
        .iter()
        .chain(&view.inspector)
        .chain(&view.tabs)
        .any(|row| row.id == id && row.enabled)
    {
        Ok(())
    } else {
        Err(format!(
            "Scene action {id} is unavailable in {}",
            view.context
        ))
    }
}

struct Host {
    runtime: Runtime,
    scene: Scene,
    result: Rc<RefCell<Option<Report>>>,
    done: bool,
    caption: Rc<RefCell<String>>,
}
impl ApplicationHost<LockFreeControlHandle> for Host {
    fn tick(&mut self, sound: &mut Sound<'_>, clock: HostClock) {
        self.runtime.tick(sound, clock);
        if self.done {
            return;
        }
        match self
            .scene
            .advance(&self.runtime, clock, Some(&sound.projection()))
        {
            Ok(done) => self.done = done,
            Err(error) => {
                self.scene.report.record_failure(error);
                let _ = self.runtime.stop(sound);
                self.done = true;
            }
        }
        let label = self
            .scene
            .steps
            .get(self.scene.index)
            .map_or("Complete", |step| step.label);
        let bar = self
            .runtime
            .app
            .borrow()
            .play_beat
            .and_then(|beat| beat.units(Beat::integer(4)).ok());
        *self.caption.borrow_mut() = match bar {
            Some(bar) => format!("{label} · BAR {}", bar + 1),
            None => label.into(),
        };
        if self.done {
            *self.result.borrow_mut() = Some(std::mem::take(&mut self.scene.report));
        }
    }
    fn document(&self) -> SessionDocumentProjection {
        self.runtime.document()
    }
    fn session_command(&mut self, _sound: &mut Sound<'_>, command: SessionCommand) -> bool {
        matches!(command, SessionCommand::Close)
    }
    fn action(&mut self, _action: SemanticAction) -> bool {
        true
    }
    fn owns_input(&self) -> bool {
        true
    }
    fn take_close_request(&mut self) -> bool {
        self.done
    }
}

pub(super) fn run() -> Result<(), Box<dyn std::error::Error>> {
    run_scene(Scene::new())
}

pub(super) fn run_piano() -> Result<(), Box<dyn std::error::Error>> {
    run_scene(Scene::piano())
}

pub(super) fn run_cursors() -> Result<(), Box<dyn std::error::Error>> {
    run_scene(Scene::cursors())
}

pub(super) fn run_scene(scene: Scene) -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::temp_dir().join(format!("crest-event-demo-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root)?;
    let result = run_in(&root, scene);
    // Scene scratch preferences/recovery never share the user's document storage.
    let _ = std::fs::remove_dir_all(root);
    result
}

fn run_in(root: &std::path::Path, scene: Scene) -> Result<(), Box<dyn std::error::Error>> {
    let app = Rc::new(RefCell::new(App::new()));
    let runtime = Runtime::with_data_directory(app.clone(), root.to_owned())?;
    let projection = app.clone();
    let caption = Rc::new(RefCell::new(String::from("Application ready")));
    let projected_caption = caption.clone();
    let presentation = HostPresentation {
        projection: Rc::new(move || {
            let mut view = crate::workspace::project(&projection.borrow());
            if let Some(workspace) = &mut view.workspace {
                workspace.status = format!(
                    "DEMO · {} · {}",
                    projected_caption.borrow(),
                    workspace.status
                );
            }
            view
        }),
        event: Rc::new(|_| {}),
    };
    let window = TauriWebviewWindow::new(format!("Crest Tracker · {} demo", scene.report.scene))
        .with_host(presentation);
    let result = Rc::new(RefCell::new(None));
    let host = Host {
        runtime,
        scene,
        result: result.clone(),
        done: false,
        caption,
    };
    production_application(window, ApplicationConfig::default())?
        .with_default_session_blueprint(crest_synth::control::DefaultSessionBlueprint::new(
            crest_synth::synth::CapabilityId::new(DEFAULT_INSTRUMENT)?,
        ))
        .without_system_input_devices()
        .run_with_host(host)?;
    let report = result
        .borrow_mut()
        .take()
        .ok_or("Demo closed before completion")?;
    serde_json::to_writer_pretty(std::io::stdout().lock(), &report)?;
    println!();
    if !report.complete {
        return Err(report
            .error
            .unwrap_or_else(|| "Demo incomplete".into())
            .into());
    }
    Ok(())
}
