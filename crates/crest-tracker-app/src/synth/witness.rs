//! Integrated witnesses exercise the production host, workers, queues and DSP.
//! They replace the window and audio device only; no hardware evidence is implied.
mod audio;
mod controller;
mod corridors;
mod live;
mod recording_edits;
mod replacement;
mod scenarios;
use super::*;
use crest_synth::adapter::{
    atomic_audio_observation::AtomicAudioObservation,
    fixed_midi_event_source::FixedMidiEventSource,
    lock_free_audio_boundary::LockFreeAudioBoundary,
    lock_free_structural_graph_boundary::LockFreeStructuralGraphBoundary,
    production_effects::{production_effect_preparers, production_effect_providers},
    production_instruments::{production_instrument_preparers, production_instrument_providers},
};
use crest_synth::control::DefaultSessionBlueprint;
use crest_synth::mixer::{global_parameters::GlobalParameters, mixer_state::MixerState};
use crest_synth::real_time::{
    AudioObservationSnapshot, GraphHandoffStatus, GraphRevision, ParameterSnapshot,
};
use crest_synth::shell::app_window::*;
use crest_synth::shell::standalone_application::StandaloneApplication;
use crest_synth::synth::CapabilityId;
use serde::Serialize;
use std::time::{Duration, Instant};

const FRAMES: usize = 256;
type Step = Box<dyn FnMut(&mut Runtime, AudioObservationSnapshot, &mut Evidence) -> bool>;

#[derive(Default, Serialize)]
struct Evidence {
    #[serde(skip)]
    control_stall_blocks: u64,
    callback_ns: Vec<u64>,
    control_ns: Vec<u64>,
    allocations: u64,
    deallocations: u64,
    peak_voices: u32,
    sustained_64_blocks: u64,
    audible_tracks: [bool; 16],
    final_voices: u32,
    routing_failures: u64,
    voice_refusals: u64,
    nonfinite_samples: u64,
    scheduled_late_events: u64,
    scheduled_max_lateness_frames: u64,
    scheduled_queue_high_water: u64,
    live_edits: u64,
    monitor_cancellations: u64,
    dense_attacks: u64,
    measured: bool,
    complete: bool,
}
impl Evidence {
    fn observe(&mut self, audio: AudioObservationSnapshot, nanos: u64, memory: (u64, u64)) {
        self.peak_voices = self.peak_voices.max(audio.active_notes());
        self.final_voices = audio.active_notes();
        self.routing_failures = audio.routing_failures();
        self.voice_refusals = audio.voice_limit_refusals();
        self.nonfinite_samples = audio.non_finite_samples();
        self.scheduled_late_events = audio.scheduled_late_events();
        self.scheduled_max_lateness_frames = audio.scheduled_max_lateness_frames();
        self.scheduled_queue_high_water = audio.scheduled_queue_high_water();
        if !self.measured {
            return;
        }
        self.callback_ns.push(nanos);
        self.allocations += memory.0;
        self.deallocations += memory.1;
        if audio.active_notes() == 64 {
            self.sustained_64_blocks += 1;
        }
        for (index, track) in audio.tracks().into_iter().enumerate() {
            self.audible_tracks[index] |= track.rms() > 0.000001;
        }
    }
    fn check_cleanup(&self) {
        assert!(self.complete, "scenario did not complete");
        assert_eq!(self.final_voices, 0, "Stop must release every owned attack");
        assert_eq!(
            (
                self.routing_failures,
                self.voice_refusals,
                self.nonfinite_samples
            ),
            (0, 0, 0)
        );
        assert_eq!((self.allocations, self.deallocations), (0, 0));
        assert_eq!(
            self.scheduled_late_events, 0,
            "admitted schedule must arrive on time"
        );
    }
}

struct Host(Rc<RefCell<Runtime>>);
impl ApplicationHost<LockFreeControlHandle> for Host {
    fn tick(&mut self, sound: &mut Sound<'_>, clock: HostClock) {
        self.0.borrow_mut().tick(sound, clock);
    }
    fn document(&self) -> SessionDocumentProjection {
        self.0.borrow().document()
    }
    fn session_command(&mut self, sound: &mut Sound<'_>, command: SessionCommand) -> bool {
        self.0.borrow_mut().session_command(sound, command)
    }
    fn action_handler(&self) -> Option<Rc<dyn Fn(SemanticAction) -> bool>> {
        self.0.borrow().action_handler()
    }
    fn owns_input(&self) -> bool {
        self.0.borrow().owns_input()
    }
}

struct Window {
    runtime: Rc<RefCell<Runtime>>,
    output: audio::Output,
    step: RefCell<Step>,
    evidence: Rc<RefCell<Evidence>>,
    wall_limit: Duration,
}
impl AppWindow for Window {
    fn run(
        &self,
        _input: AppInputCallback,
        projection: ProjectionCallback,
        observation: AudioObservationCallback,
        _midi: MidiActivityObservationCallback,
        _session: SessionCommandCallback,
        _document: SessionDocumentProjectionCallback,
        mut tick: TickCallback,
        _paint: FrameObservationCallback,
    ) -> Result<(), WindowError> {
        let start = Instant::now();
        let mut buffer = [0.0_f32; FRAMES * 2];
        let elapsed = Duration::from_secs_f64(FRAMES as f64 / 48_000.0);
        loop {
            assert!(
                start.elapsed() < self.wall_limit,
                "witness timed out: {}",
                self.runtime.borrow().app.borrow().status
            );
            let (nanos, memory) = self.output.render(&mut buffer);
            assert!(buffer.iter().all(|sample| sample.is_finite()));
            let audio = observation();
            self.evidence.borrow_mut().observe(audio, nanos, memory);
            if self.evidence.borrow().control_stall_blocks > 0 {
                self.evidence.borrow_mut().control_stall_blocks -= 1;
                continue;
            }
            let control_start = Instant::now();
            assert!(tick(elapsed), "production control loop failed");
            if audio.rendered_blocks().is_multiple_of(4) {
                let _ = projection();
            }
            if self.evidence.borrow().measured {
                self.evidence
                    .borrow_mut()
                    .control_ns
                    .push(control_start.elapsed().as_nanos() as u64);
            }
            let complete = (self.step.borrow_mut())(
                &mut self.runtime.borrow_mut(),
                audio,
                &mut self.evidence.borrow_mut(),
            );
            if complete {
                self.evidence.borrow_mut().complete = true;
                break;
            }
            std::thread::yield_now();
        }
        Ok(())
    }
}

fn run(root: &std::path::Path, step: Step, wall_limit: Duration) -> Evidence {
    run_with_instrument(
        root,
        step,
        wall_limit,
        crest_synth::adapter::braids_capability::BRAIDS_CAPABILITY_ID,
    )
}

fn run_with_instrument(
    root: &std::path::Path,
    step: Step,
    wall_limit: Duration,
    capability: &str,
) -> Evidence {
    run_with_devices(root, step, wall_limit, capability, false)
}

fn run_with_devices(
    root: &std::path::Path,
    step: Step,
    wall_limit: Duration,
    capability: &str,
    system_devices: bool,
) -> Evidence {
    let app = Rc::new(RefCell::new(App::new()));
    let runtime = Rc::new(RefCell::new(
        Runtime::with_data_directory(app, root.join("machine")).unwrap(),
    ));
    let output = audio::Output::default();
    let evidence = Rc::new(RefCell::new(Evidence::default()));
    let window = Window {
        runtime: runtime.clone(),
        output: output.clone(),
        step: RefCell::new(step),
        evidence: evidence.clone(),
        wall_limit,
    };
    let config = ApplicationConfig::new(48_000.0, FRAMES, GlobalParameters::new(-12.0).unwrap());
    let parameters =
        ParameterSnapshot::new(0, config.global_parameters(), MixerState::default(), &[]).unwrap();
    let boundary = LockFreeAudioBoundary::new(4096, parameters);
    let structural = LockFreeStructuralGraphBoundary::new(
        1,
        1,
        GraphHandoffStatus::with_active(GraphRevision::INITIAL),
    )
    .unwrap();
    let application = StandaloneApplication::new_with_effects(
        boundary,
        production_instrument_providers().unwrap(),
        production_instrument_preparers().unwrap(),
        production_effect_providers().unwrap(),
        production_effect_preparers().unwrap(),
        structural,
        AtomicAudioObservation::default(),
        FixedMidiEventSource::new(),
        window,
        output.clone(),
        config,
    )
    .unwrap()
    .with_default_session_blueprint(DefaultSessionBlueprint::new(
        CapabilityId::new(capability).unwrap(),
    ));
    let application = if system_devices {
        application.with_system_input_devices()
    } else {
        application
    };
    application.run_with_host(Host(runtime)).unwrap();
    // Retire the callback and its graph outside the measured region.
    output.callback.borrow_mut().take();
    Rc::try_unwrap(evidence).ok().unwrap().into_inner()
}
