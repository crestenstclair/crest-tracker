//! One event script for the visible demo and the production-DSP witness.
//! Actions enter the same workspace boundary as the UI. Observations only verify
//! results; this module has no controller/device API and never edits Song state.
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
    complete: bool,
    error: Option<String>,
}

enum Event {
    Action(SemanticAction),
    Activate(&'static str),
    Decision(&'static str),
    Wait,
}
enum Check {
    Ready,
    Held(bool),
    Chain,
    Phrase,
    View(View),
    Note(bool),
    FileMenu,
    Confirm,
    Retained,
    Imported,
    Playing,
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
    pub report: Report,
}

impl Scene {
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
            report: Report {
                scene: "corridors-of-time",
                ..Report::default()
            },
        }
    }

    pub fn advance(&mut self, runtime: &Runtime, clock: HostClock) -> Result<bool, String> {
        self.observe(clock.audio);
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
        let step = &self.steps[self.index];
        let limit = if matches!(step.check, Check::Finished) {
            330
        } else {
            45
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
        if self.full_playback {
            if let Some(beat) = runtime.app.borrow().play_beat {
                self.report.last_beat = Some(self.report.last_beat.unwrap_or(Beat::ZERO).max(beat));
            }
        }
        // A short control-clock dwell makes the same script legible in the real
        // window. Worker/graph/audio completion is always a state checkpoint.
        if self.elapsed < Duration::from_millis(160)
            || !self.check(runtime, &step.check, clock.audio)?
        {
            return Ok(false);
        }
        eprintln!("DEMO PASS · {}", step.label);
        if matches!(step.check, Check::Imported) {
            let app = runtime.app.borrow();
            self.report.notes = app
                .editor
                .song()
                .phrases
                .values()
                .flat_map(|phrase| &phrase.events)
                .filter(|event| event.note.is_some())
                .count();
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

    fn check(
        &self,
        runtime: &Runtime,
        check: &Check,
        audio: AudioObservationSnapshot,
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
            Check::Ready => runtime.initial_sound.is_some() && runtime.sync.ready(),
            Check::Held(held) => app.edit_held == *held,
            Check::Chain => app.view == View::Song && app.selected_chain().is_some(),
            Check::Phrase => app.view == View::Chain && app.selected_phrase().is_some(),
            Check::View(view) => {
                app.view == *view && crate::workspace::project(&app).workspace.is_some()
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
            Check::Playing | Check::FullPlayback => app.playing && audio.active_notes() > 0,
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
        Event::Wait => {}
        Event::Action(action) => {
            log.push(format!("{action:?}"));
            if !crate::workspace::action(&mut app, action.clone()) {
                return Err("Scene action was not handled by Tracker".into());
            }
        }
        Event::Activate(id) => {
            log.push(format!("Activate({id})"));
            crate::workspace::event(&mut app, WorkspaceEvent::Activate { id: (*id).into() });
        }
        Event::Decision(choice) => {
            let dialog = &app.file_dialog;
            if dialog["mode"] != "confirm" {
                return Err("Scene expected replacement confirmation".into());
            }
            let id = format!("dialog:{}:{}:{choice}", dialog["token"], dialog["version"]);
            log.push(format!("Activate({id})"));
            crate::workspace::event(&mut app, WorkspaceEvent::Activate { id });
        }
    }
    Ok(())
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
        match self.scene.advance(&self.runtime, clock) {
            Ok(done) => self.done = done,
            Err(error) => {
                self.scene.report.error = Some(error);
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
    let root = std::env::temp_dir().join(format!("crest-corridors-demo-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root)?;
    let result = run_in(&root);
    // Scene scratch preferences/recovery never share the user's document storage.
    let _ = std::fs::remove_dir_all(root);
    result
}

fn run_in(root: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
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
    let window =
        TauriWebviewWindow::new("Crest Tracker · Corridors of Time demo").with_host(presentation);
    let result = Rc::new(RefCell::new(None));
    let host = Host {
        runtime,
        scene: Scene::new(),
        result: result.clone(),
        done: false,
        caption,
    };
    production_application(window, ApplicationConfig::default())?
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
