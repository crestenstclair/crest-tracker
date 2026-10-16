//! Host clock ownership. Exact input pulse segments drive external playback;
//! internal playback alone can generate clock output. No automatic echo.
use super::*;
use crate::midi_clock::{
    ClockAction, ClockMessage, ClockOutput, ClockSource, ClockStopReason, ClockTransport,
    PulseTimeline,
};
use crate::preferences::MachinePreferences;
use crest_synth::adapter::browser_preferences::{self, BrowserPreferences};
use std::collections::VecDeque;

const INPUT_CAPACITY: usize = 1024;

struct ClockOutputRun {
    pulses: ClockOutput,
    // Every message at the start frame must have exactly the same deadline.
    // Re-correlating Instant and CoreAudio separately can reorder SPP/Continue
    // and the first Tick by a few microseconds in the output priority queue.
    start_deadline: std::time::Instant,
}

pub(super) struct ClockRuntime {
    pub preferences: MachinePreferences,
    directory: PathBuf,
    shared_directory: Option<PathBuf>,
    operation: Option<std::sync::mpsc::Receiver<Result<MachinePreferences, String>>>,
    transport: ClockTransport,
    source: Option<ClockSource>,
    overflow_epoch: u64,
    pub timeline: PulseTimeline,
    pending: VecDeque<crest_synth::control::PhysicalMidiPacket>,
    error: Option<String>,
    output: Option<ClockOutputRun>,
    pub tempo_ignored: bool,
    awaiting_start: Option<Beat>,
}
impl ClockRuntime {
    pub fn new(directory: PathBuf, shared_directory: Option<PathBuf>) -> Result<Self, String> {
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        let root = directory.clone();
        let shared = shared_directory.clone();
        std::thread::spawn(move || {
            let _ = send.send(load_preferences(&root, shared.as_deref()));
        });
        Ok(Self {
            preferences: MachinePreferences::default(),
            directory,
            shared_directory,
            operation: Some(receive),
            transport: ClockTransport::default(),
            source: Some(ClockSource::Internal),
            overflow_epoch: 0,
            timeline: PulseTimeline::new(4096).map_err(|e| e.to_string())?,
            pending: VecDeque::new(),
            error: None,
            output: None,
            tempo_ignored: false,
            awaiting_start: None,
        })
    }
    pub fn external(&self) -> bool {
        self.preferences.clock.input.is_some()
    }
    pub fn ready(&self) -> bool {
        self.operation.is_none()
    }
    /// The owner must finalize capture before dropping its clock history.
    pub fn stop(&mut self) {
        self.transport.stop_local();
        self.pending.clear();
        self.timeline.clear();
        self.awaiting_start = None;
        self.output = None;
        self.tempo_ignored = false;
    }
    pub fn receive(&mut self, packet: crest_synth::control::PhysicalMidiPacket) {
        if !self.external() || !self.ready() || self.error.is_some() {
            return;
        }
        if self.pending.len() == INPUT_CAPACITY {
            self.pending.clear();
            self.error = Some("Clock input queue full; transport stopped; take retained".into());
        } else {
            self.pending.push_back(packet);
        }
    }
    /// Preserve only packets from the newly selected session. Old queued Start
    /// and Tick packets must never restart playback after disconnect/reconnect.
    fn bind_source(&mut self, source: Option<ClockSource>) -> Result<(), String> {
        self.transport.stop_local();
        match &source {
            Some(source) => {
                self.transport
                    .set_source(source.clone())
                    .map_err(|e| e.to_string())?;
            }
            None => {
                if let ClockSource::Input { endpoint, revision } = self.transport.source().clone() {
                    self.transport.disconnect(&endpoint, revision);
                }
            }
        }
        self.pending.retain(|packet| matches!(&source, Some(ClockSource::Input { revision, .. }) if packet.revision().value() == *revision));
        self.source = source;
        self.overflow_epoch = 0;
        self.timeline.clear();
        self.awaiting_start = None;
        self.output = None;
        self.tempo_ignored = false;
        Ok(())
    }
    fn observe_overflow(&mut self, epoch: u64) -> Result<(), String> {
        if epoch != self.overflow_epoch {
            self.overflow_epoch = epoch;
            if epoch != 0 {
                return Err(
                    "MIDI input overflow; external transport stopped; take retained".into(),
                );
            }
        }
        Ok(())
    }
    fn accept_transport(
        &mut self,
        event: crest_synth::control::PhysicalMidiTransportEvent,
    ) -> Result<Option<ClockAction>, String> {
        let Some(endpoint) = &self.preferences.clock.input else {
            return Ok(None);
        };
        let message = ClockMessage::parse(event.raw_bytes()).map_err(|e| e.to_string())?;
        self.transport
            .receive(
                endpoint,
                event.revision().value(),
                event.timestamp_micros(),
                message,
            )
            .map_err(|e| e.to_string())
    }
    pub fn delay_micros(&self) -> u64 {
        u64::from(self.preferences.clock.delay_ms) * 1000
    }
    pub fn capture_beat(&self, timestamp: u64) -> Result<Beat, String> {
        self.timeline
            .beat_at(timestamp.saturating_sub(self.delay_micros()))
            .map_err(|e| e.to_string())
    }
}

// MIDI Song Position Pointer represents exact sixteenth-note boundaries only.
fn start_messages(at: Beat) -> Result<Vec<ClockMessage>, String> {
    if at.is_zero() {
        return Ok(vec![ClockMessage::Start]);
    }
    let units = at.checked_scale(4, 1).map_err(|error| error.to_string())?;
    if units.denominator() != 1 || units.numerator() > 16_383 {
        return Err(
            "MIDI clock seek requires a sixteenth-note boundary within Song Position Pointer range"
                .into(),
        );
    }
    Ok(vec![
        ClockMessage::SongPosition(units.numerator() as u16),
        ClockMessage::Continue,
    ])
}

impl Runtime {
    pub(super) fn configure_preferences(
        &mut self,
        sound: &mut Sound<'_>,
        mut preferences: MachinePreferences,
        input_clock: bool,
    ) -> Result<(), String> {
        if self.sync.operation.is_some() {
            return Err("Preferences are still being read or saved".into());
        }
        preferences.clock.input = if input_clock {
            Some(
                sound
                    .active_midi_input_identity()
                    .ok_or("Connect the intended clock input in Synth Devices first")?
                    .identity()
                    .to_string(),
            )
        } else {
            None
        };
        preferences.validate()?;
        if let Some(endpoint) = &preferences.clock.output {
            if !self.outputs.snapshot().connected.contains(endpoint) {
                return Err("Connect the exact clock output first".into());
            }
        }
        self.stop(sound)?;
        let directory = self.sync.directory.clone();
        let shared = self.sync.shared_directory.clone();
        let folders_changed = preferences.sample_folder != self.sync.preferences.sample_folder
            || preferences.soundfont_folder != self.sync.preferences.soundfont_folder;
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let result = (|| {
                if let Some(shared) = shared {
                    if folders_changed {
                        browser_preferences::save(
                            &shared,
                            &browser_preferences(&preferences),
                            true,
                        )?;
                    } else {
                        let mut local = preferences.clone();
                        local.sample_folder = None;
                        local.soundfont_folder = None;
                        crate::preferences::save(&directory, &local)?;
                    }
                } else {
                    crate::preferences::save(&directory, &preferences)?;
                }
                Ok(preferences)
            })();
            let _ = send.send(result);
        });
        self.sync.operation = Some(receive);
        self.app.borrow_mut().status = "SAVING MACHINE PREFERENCES".into();
        Ok(())
    }

    pub(super) fn advance_clock(
        &mut self,
        sound: &mut Sound<'_>,
        clock: HostClock,
    ) -> Result<(), String> {
        if let Some(result) = self.sync.operation.as_ref().and_then(poll_once) {
            self.sync.operation = None;
            let preferences = result?;
            // Capture cutoff still belongs to the old authority and latency.
            self.stop(sound)?;
            for (kind, folder) in browser_preferences(&preferences).folders()? {
                sound
                    .set_browser_start(kind, folder)
                    .map_err(|error| error.to_string())?;
            }
            self.sync.preferences = preferences;
            self.app.borrow_mut().status = "MACHINE PREFERENCES LOADED".into();
        }
        let active = sound.active_midi_input_identity();
        self.midi_controls.observe_source(
            active.map(|active| (active.identity().to_string(), active.revision().value())),
            sound
                .midi_activity()
                .map_or(0, |activity| activity.overflow_epoch()),
        );
        let source = match (&self.sync.preferences.clock.input, active) {
            (Some(selected), Some(active)) if selected == &active.identity().to_string() => {
                Some(ClockSource::Input {
                    endpoint: selected.clone(),
                    revision: active.revision().value(),
                })
            }
            (None, _) => Some(ClockSource::Internal),
            _ => None,
        };
        if source != self.sync.source {
            self.stop_outputs(
                sound,
                "MIDI clock source changed or disconnected; take retained",
            )?;
            self.sync.bind_source(source)?;
            self.app.borrow_mut().status =
                "CLOCK SOURCE CHANGED · Fresh Start/Continue required".into();
        }
        if let Some(error) = self.sync.error.take() {
            return Err(error);
        }
        if self.sync.external() && self.sync.source.is_some() {
            self.sync.observe_overflow(
                sound
                    .midi_activity()
                    .map_or(0, |activity| activity.overflow_epoch()),
            )?;
        }
        // Source selection happens before this drain; transport and performance
        // packets then retain the native ingress order across Start/Stop/SPP.
        let packets = std::mem::take(&mut self.sync.pending);
        for packet in packets {
            match packet {
                crest_synth::control::PhysicalMidiPacket::Transport(event) => {
                    if let Some(action) = self.sync.accept_transport(event)? {
                        self.clock_action(sound, clock, action)?;
                    }
                }
                crest_synth::control::PhysicalMidiPacket::Performance(event) => {
                    self.capture_midi_input(event, clock);
                }
            }
        }
        if self.sync.external() {
            if let Some(action) = self.sync.transport.poll_loss(host_micros()?) {
                self.clock_action(sound, clock, action)?;
            }
        }
        self.project_clock(sound);
        Ok(())
    }

    fn project_clock(&mut self, sound: &Sound<'_>) {
        let active = sound.active_midi_input_identity();
        self.app.borrow_mut().preferences = serde_json::json!({
            "default_bpm":self.sync.preferences.default_tempo.bpm(),
            "sample_folder":self.sync.preferences.sample_folder,"soundfont_folder":self.sync.preferences.soundfont_folder,
            "monitor":self.sync.preferences.monitor,"clock":self.sync.preferences.clock,
            "mpe_input":self.sync.preferences.mpe_input,
            "mpe_outputs":self.sync.preferences.mpe_outputs,
            "midi_controls":self.sync.preferences.midi_controls,
            "roll_bindings":self.sync.preferences.roll_bindings,
            "roll_buttons":crate::preferences::RollBindings::button_options(),
            "active_input":active.map(|identity|serde_json::json!({
                "id":identity.identity().to_string(),
                "name":sound.active_midi_input_descriptor().map(|device|device.display_name())
            })),
            "running":self.sync.transport.running(),"ready":self.sync.ready(),
            "song_tempo_ignored":self.sync.tempo_ignored,
            "status": if self.sync.external() { if self.sync.transport.running() { "EXTERNAL RUNNING" } else { "EXTERNAL WAITING FOR START" } } else { "INTERNAL" },
        });
    }

    fn clock_action(
        &mut self,
        sound: &mut Sound<'_>,
        clock: HostClock,
        action: ClockAction,
    ) -> Result<(), String> {
        match action {
            ClockAction::Begin { position, .. } => {
                let setup = matches!(
                    self.recorder.state(),
                    CaptureState::Armed | CaptureState::Waiting { .. }
                )
                .then(|| self.recorder.clone());
                if setup.as_ref().is_some_and(|recorder| {
                    recorder.mpe_profile().is_some() && recorder.held_count() > 0
                }) {
                    return Err(
                        "Release MPE keys and pedals before external Start or Continue".into(),
                    );
                }
                self.stop_outputs(sound, "External MIDI clock restarted; take retained")?;
                self.flush_monitor(sound)?;
                if let Some(recorder) = setup {
                    self.recorder = recorder;
                }
                if matches!(self.recorder.state(), CaptureState::Waiting { .. }) {
                    self.recorder
                        .reschedule_waiting(self.next_capture_start(position)?)
                        .map_err(|e| e.to_string())?;
                }
                self.sync.timeline.clear();
                self.sync.tempo_ignored = false;
                self.sync.awaiting_start = Some(position);
                self.app.borrow_mut().status = "EXTERNAL CLOCK · Waiting for pulse interval".into();
            }
            ClockAction::Tick {
                position: Some(position),
                timestamp_micros,
                ..
            } => {
                self.sync
                    .timeline
                    .push(position, timestamp_micros)
                    .map_err(|e| e.to_string())?;
                if let Some(start) = self.sync.awaiting_start {
                    if position > start {
                        super::expression::validate_song(
                            sound,
                            &self.app.borrow(),
                            &self.sync.preferences.mpe_outputs,
                        )?;
                        self.prepare_external_playback(
                            start,
                            clock,
                            sound.active_graph_revision().value(),
                        )?;
                    }
                }
            }
            ClockAction::Stop { reason } => {
                let reason = clock_stop_reason(reason);
                self.stop_with_reason(sound, reason)?;
                self.app.borrow_mut().status =
                    format!("EXTERNAL CLOCK STOPPED · {reason} · Fresh Start/Continue required");
            }
            _ => {}
        }
        Ok(())
    }

    fn prepare_external_playback(
        &mut self,
        start: Beat,
        clock: HostClock,
        graph_revision: u64,
    ) -> Result<(), String> {
        let app = self.app.borrow();
        let snapshot = Arc::new(
            Snapshot::compile(
                app.editor.song(),
                app.editor.revision(),
                PlaybackBudget::default(),
            )
            .map_err(|e| e.to_string())?,
        );
        let mut playback = match self.phrase_loop {
            Some(path) => Playback::new_looping(snapshot, path).map_err(|e| e.to_string())?,
            None => Playback::new(snapshot),
        };
        playback.seek_forward(start).map_err(|e| e.to_string())?;
        self.transport_selection = transport::PlaybackSelection::Song;
        self.transport_start = start;
        self.transport_offset_frames = 0;
        self.tempo = TempoMap::new(app.editor.song().tempo);
        drop(app);
        self.playback = Some(playback);
        self.last_scheduled_frame = clock.audio.rendered_frames();
        self.graph_revision = graph_revision;
        self.sync.awaiting_start = None;
        self.app.borrow_mut().playing = true;
        self.app.borrow_mut().status = "PLAYING · EXTERNAL CLOCK".into();
        self.app.borrow_mut().playback_fault = None;
        Ok(())
    }

    pub(super) fn frame_at_beat(&self, at: Beat, clock: HostClock) -> Result<u64, String> {
        if self.sync.external() {
            let timestamp = self
                .sync
                .timeline
                .timestamp_at(at)
                .map_err(|e| e.to_string())?
                .checked_add(self.sync.delay_micros())
                .ok_or("Clock timestamp overflow")?;
            let frame = clock
                .audio_clock
                .and_then(|clock| clock.frame_at_midi_timestamp(timestamp))
                .ok_or("External clock requires shared input/audio timing")?;
            if frame < clock.audio.rendered_frames() {
                return Err(
                    "External clock arrived too late; increase clock latency in Settings".into(),
                );
            }
            Ok(frame)
        } else {
            self.anchor
                .checked_add(
                    self.tempo
                        .frame_at(at, clock.sample_rate)
                        .map_err(|e| e.to_string())?
                        .checked_sub(self.transport_offset_frames)
                        .ok_or("Event precedes transport start")?,
                )
                .ok_or("Audio frame overflow".into())
        }
    }

    pub(super) fn transport_beat(&self, clock: HostClock) -> Result<Beat, String> {
        if !self.sync.external() {
            return self
                .tempo
                .beat_at_frame(
                    self.musical_frame(clock.audio.rendered_frames())?,
                    clock.sample_rate,
                )
                .map_err(|e| e.to_string());
        }
        let (first, first_time) = self
            .sync
            .timeline
            .earliest()
            .ok_or("External clock has no pulse interval")?;
        let (last, last_time) = self
            .sync
            .timeline
            .latest()
            .ok_or("External clock has no pulse interval")?;
        let timestamp = audio_timestamp_at_frame(clock, clock.audio.rendered_frames())?
            .saturating_sub(self.sync.delay_micros());
        if timestamp <= first_time {
            return Ok(first);
        }
        if timestamp >= last_time {
            return Ok(last);
        }
        self.sync
            .timeline
            .beat_at(timestamp)
            .map_err(|e| e.to_string())
    }

    pub(super) fn start_clock_output(&mut self, clock: HostClock) -> Result<(), String> {
        if self.sync.external() {
            return Err("Clock echo is disabled".into());
        }
        if let Some(endpoint) = &self.sync.preferences.clock.output {
            let deadline = deadline_for_frame(clock, self.anchor)?;
            for message in start_messages(self.transport_start)? {
                self.outputs
                    .send_transport(
                        endpoint,
                        &message.bytes().map_err(|e| e.to_string())?,
                        deadline,
                        self.outputs.epoch(),
                    )
                    .map_err(|e| e.to_string())?;
            }
            self.sync.output = Some(ClockOutputRun {
                pulses: ClockOutput::new(self.transport_start, 4096).map_err(|e| e.to_string())?,
                start_deadline: deadline,
            });
        }
        Ok(())
    }
    pub(super) fn send_clock_output(&mut self, end: Beat, clock: HostClock) -> Result<(), String> {
        let Some(output) = &mut self.sync.output else {
            return Ok(());
        };
        let endpoint = self
            .sync
            .preferences
            .clock
            .output
            .as_deref()
            .ok_or("Clock output destination missing")?;
        let start_deadline = output.start_deadline;
        let pulses = output
            .pulses
            .window(end, &self.tempo, clock.sample_rate)
            .map_err(|e| e.to_string())?;
        for pulse in pulses {
            // Seeking rebases notes and clock to the same audio anchor. The
            // pulse's TempoMap frame alone still includes time before the seek.
            let frame = self.frame_at_beat(pulse.at, clock)?;
            let deadline = if frame == self.anchor {
                start_deadline
            } else {
                // Later pulses follow current backend clock observations, so
                // device drift does not accumulate against a fixed wall clock.
                deadline_for_frame(clock, frame)?
            };
            self.outputs
                .send_transport(
                    endpoint,
                    &ClockMessage::Tick.bytes().map_err(|e| e.to_string())?,
                    deadline,
                    self.outputs.epoch(),
                )
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

fn clock_stop_reason(reason: ClockStopReason) -> &'static str {
    match reason {
        ClockStopReason::LocalStop => "Local Stop",
        ClockStopReason::TransportStop => "MIDI Stop received",
        ClockStopReason::SourceChanged => "MIDI clock source changed",
        ClockStopReason::Disconnected => "MIDI clock input disconnected",
        ClockStopReason::ClockLost => "MIDI clock pulses lost",
        ClockStopReason::InvalidTimestamp => "Invalid MIDI clock timestamp",
        ClockStopReason::PositionWhileRunning => "Song Position Pointer received while running",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::midi_clock::ClockStopReason;
    use crest_synth::control::{
        MidiConnectionRevision, PhysicalMidiIngress, PhysicalMidiIngressOutcome, PhysicalMidiPacket,
    };
    use crest_synth::shell::audio_output::{AudioClockDomain, AudioFrameClock};

    fn beat(n: u64, d: u64) -> Beat {
        Beat::new(n, d).unwrap()
    }
    fn packet(revision: u64, timestamp: u64, raw: &[u8]) -> PhysicalMidiPacket {
        let (mut ingress, mut consumer) =
            PhysicalMidiIngress::bounded(MidiConnectionRevision::new(revision).unwrap());
        consumer.enable();
        assert_eq!(
            ingress.receive_raw(timestamp, raw),
            PhysicalMidiIngressOutcome::Accepted
        );
        consumer.try_pop_packet().unwrap()
    }
    fn source(revision: u64) -> Option<ClockSource> {
        Some(ClockSource::Input {
            endpoint: "clock-source".into(),
            revision,
        })
    }
    fn fixture() -> Runtime {
        let mut runtime = super::super::tests::capture_fixture();
        runtime.sync.operation = None;
        runtime.sync.preferences.clock.input = Some("clock-source".into());
        runtime.sync.bind_source(source(1)).unwrap();
        runtime
    }
    fn accept(
        runtime: &mut Runtime,
        revision: u64,
        timestamp: u64,
        raw: &[u8],
    ) -> Option<ClockAction> {
        let PhysicalMidiPacket::Transport(event) = packet(revision, timestamp, raw) else {
            panic!("expected transport")
        };
        runtime.sync.accept_transport(event).unwrap()
    }
    fn anchors(runtime: &mut Runtime) {
        runtime.sync.timeline.push(Beat::ZERO, 1_000_000).unwrap();
        runtime.sync.timeline.push(beat(1, 24), 1_024_000).unwrap();
        runtime.sync.timeline.push(beat(1, 12), 1_044_000).unwrap();
    }
    fn clock_at(playback_micros: u64) -> HostClock {
        let mut clock = super::super::tests::clock();
        clock.audio_clock = AudioFrameClock::new(
            0,
            playback_micros - 10_000,
            playback_micros,
            48_000,
            AudioClockDomain::CoreAudioHost,
        );
        clock
    }

    #[test]
    fn native_ingress_order_survives_queue_and_reconnect_discards_old_commands() {
        let mut runtime = fixture();
        for event in [
            packet(1, 1_000_000, &[0xfa]),
            packet(1, 1_001_000, &[0xf8]),
            packet(2, 1_002_000, &[0xf2, 5, 0]),
            packet(2, 1_003_000, &[0xfb]),
            packet(2, 1_004_000, &[0xf8]),
        ] {
            runtime.sync.receive(event);
        }
        runtime.sync.bind_source(source(2)).unwrap();
        let pending = std::mem::take(&mut runtime.sync.pending);
        assert_eq!(pending.len(), 3);
        let actions: Vec<_> = pending
            .into_iter()
            .map(|packet| {
                let PhysicalMidiPacket::Transport(event) = packet else {
                    unreachable!()
                };
                runtime.sync.accept_transport(event).unwrap().unwrap()
            })
            .collect();
        assert_eq!(
            actions[0],
            ClockAction::Position {
                position: beat(5, 4)
            }
        );
        assert_eq!(
            actions[1],
            ClockAction::Begin {
                position: beat(5, 4),
                timestamp_micros: 1_003_000
            }
        );
        assert!(
            matches!(actions[2], ClockAction::Tick { position: Some(position), .. } if position == beat(5, 4))
        );
        runtime.sync.bind_source(None).unwrap();
        runtime.sync.bind_source(source(3)).unwrap();
        assert!(!runtime.sync.transport.running());
        assert_eq!(accept(&mut runtime, 2, 2_000_000, &[0xfa]), None);
        assert!(matches!(
            accept(&mut runtime, 3, 2_000_001, &[0xf8]),
            Some(ClockAction::Tick { position: None, .. })
        ));
    }

    #[test]
    fn pending_preferences_and_capacity_failures_never_publish_partial_clock_input() {
        let mut runtime = fixture();
        let (_send, receive) = std::sync::mpsc::sync_channel(1);
        runtime.sync.operation = Some(receive);
        runtime.sync.receive(packet(1, 1_000_000, &[0xfa]));
        assert!(!runtime.sync.ready());
        assert!(runtime.sync.pending.is_empty());
        runtime.sync.operation = None;
        for index in 0..=INPUT_CAPACITY {
            runtime
                .sync
                .receive(packet(1, 1_000_000 + index as u64, &[0xf8]));
        }
        assert!(runtime.sync.pending.is_empty());
        assert!(runtime.sync.error.take().unwrap().contains("queue full"));
        runtime.sync.stop();
        assert!(runtime
            .sync
            .observe_overflow(1)
            .unwrap_err()
            .contains("take retained"));
        // Acknowledged overflow does not permanently disable the selected input.
        runtime.sync.observe_overflow(1).unwrap();
        assert!(matches!(
            accept(&mut runtime, 1, 2_000_000, &[0xfa]),
            Some(ClockAction::Begin { .. })
        ));
    }

    #[test]
    fn local_stop_and_clock_loss_preserve_position_without_tick_resumption() {
        let mut runtime = fixture();
        accept(&mut runtime, 1, 1_000_000, &[0xfa]);
        accept(&mut runtime, 1, 1_000_000, &[0xf8]);
        accept(&mut runtime, 1, 1_020_000, &[0xf8]);
        runtime.sync.stop();
        assert!(matches!(
            accept(&mut runtime, 1, 1_040_000, &[0xf8]),
            Some(ClockAction::Tick { position: None, .. })
        ));
        assert_eq!(
            accept(&mut runtime, 1, 1_050_000, &[0xfb]),
            Some(ClockAction::Begin {
                position: beat(1, 12),
                timestamp_micros: 1_050_000
            })
        );
        assert_eq!(
            runtime.sync.transport.poll_loss(1_550_000),
            Some(ClockAction::Stop {
                reason: ClockStopReason::ClockLost
            })
        );
        assert!(!runtime.sync.transport.running());
    }

    #[test]
    fn external_schedule_and_playhead_use_exact_pulses_and_audio_time() {
        let mut runtime = fixture();
        anchors(&mut runtime);
        let clock = clock_at(1_010_000);
        assert_eq!(runtime.frame_at_beat(beat(1, 48), clock).unwrap(), 4896);
        assert_eq!(runtime.frame_at_beat(beat(1, 16), clock).unwrap(), 5952);
        assert_eq!(runtime.sync.capture_beat(1_112_000).unwrap(), beat(1, 48));
        assert_eq!(
            runtime.transport_beat(clock_at(1_112_000)).unwrap(),
            beat(1, 48)
        );
        let mut delayed_ui = clock_at(1_112_000);
        delayed_ui.control_micros = u64::MAX;
        delayed_ui.elapsed = std::time::Duration::from_secs(300);
        assert_eq!(runtime.transport_beat(delayed_ui).unwrap(), beat(1, 48));
        assert!(runtime.frame_at_beat(Beat::ONE, clock).is_err());
        let mut missing = clock;
        missing.audio_clock = None;
        assert!(runtime.frame_at_beat(Beat::ZERO, missing).is_err());
        assert!(runtime.transport_beat(missing).is_err());
    }

    #[test]
    fn host_capture_queues_in_ingress_order_and_stop_retains_exact_take() {
        let mut runtime = fixture();
        anchors(&mut runtime);
        let clock = clock_at(1_136_000);
        let PhysicalMidiPacket::Performance(attack) = packet(1, 1_112_000, &[0x94, 61, 99]) else {
            unreachable!()
        };
        assert!(runtime.midi_input(attack, clock));
        runtime.midi_transport(
            match packet(1, 1_136_000, &[0xfc]) {
                PhysicalMidiPacket::Transport(event) => event,
                _ => unreachable!(),
            },
            clock,
        );
        let mut queued = std::mem::take(&mut runtime.sync.pending);
        assert!(runtime.monitor_pending.is_empty());
        let Some(PhysicalMidiPacket::Performance(event)) = queued.pop_front() else {
            panic!("lost channel/transport order")
        };
        runtime.capture_midi_input(event, clock);
        assert_eq!(runtime.monitor_pending.len(), 1);
        assert_eq!(runtime.recorder.held_count(), 1);
        let Some(PhysicalMidiPacket::Transport(stop)) = queued.pop_front() else {
            panic!("lost stop")
        };
        assert!(matches!(
            runtime.sync.accept_transport(stop).unwrap(),
            Some(ClockAction::Stop { .. })
        ));
        runtime
            .interrupt_capture("Transport stopped; take retained")
            .unwrap();
        runtime.sync.stop();
        assert!(runtime.sync.timeline.latest().is_none());
        assert!(matches!(
            runtime.recorder.state(),
            CaptureState::Interrupted { .. }
        ));
        let note = runtime
            .recorder
            .draft()
            .unwrap()
            .events
            .iter()
            .find(|event| event.note.is_some())
            .unwrap();
        assert_eq!(note.at, beat(1, 48));
        assert_eq!(note.note.as_ref().unwrap().duration, beat(11, 240));
        assert!(runtime.checkpoint_dirty);
    }

    #[test]
    fn external_spp_seek_keeps_only_future_owned_attacks_with_exact_frames() {
        let mut runtime = fixture();
        let phrase = runtime.app.borrow().selected_phrase().unwrap().0;
        for (identity, at, pitch) in [(901, Beat::ZERO, 60), (902, beat(13, 48), 64)] {
            runtime
                .app
                .borrow_mut()
                .command(Command::PutEvent {
                    phrase,
                    event: Event::note(
                        EventId::new(identity).unwrap(),
                        at,
                        Target::Instrument {
                            id: InstrumentId::new(17).unwrap(),
                            channel: Channel::FIRST,
                        },
                        Note::new(pitch, 100, beat(1, 96)).unwrap(),
                    ),
                })
                .unwrap();
        }
        assert!(
            matches!(accept(&mut runtime, 1, 999_000, &[0xf2, 1, 0]), Some(ClockAction::Position { position }) if position == beat(1, 4))
        );
        let Some(ClockAction::Begin { position, .. }) = accept(&mut runtime, 1, 999_001, &[0xfb])
        else {
            panic!("continue missing")
        };
        for timestamp in [1_000_000, 1_024_000] {
            let Some(ClockAction::Tick {
                position: Some(at), ..
            }) = accept(&mut runtime, 1, timestamp, &[0xf8])
            else {
                panic!("pulse missing")
            };
            runtime.sync.timeline.push(at, timestamp).unwrap();
        }
        let clock = clock_at(1_010_000);
        runtime
            .prepare_external_playback(position, clock, 19)
            .unwrap();
        assert_eq!(runtime.playback.as_ref().unwrap().horizon(), beat(1, 4));
        let events = runtime
            .playback
            .as_mut()
            .unwrap()
            .window(beat(7, 24))
            .unwrap();
        assert_eq!(events.len(), 2);
        assert!(events
            .iter()
            .all(|event| event.owner.event == EventId::new(902).unwrap()));
        assert!(matches!(
            events[0].message,
            Message::NoteOn { pitch: 64, .. }
        ));
        assert_eq!(runtime.frame_at_beat(events[0].at, clock).unwrap(), 4896);
        let voice = runtime.voices.voice_for(&events[0]).unwrap();
        assert_eq!(runtime.voices.voice_for(&events[1]).unwrap(), voice);
        assert_eq!(runtime.graph_revision, 19);
    }

    #[test]
    fn internal_clock_output_has_no_automatic_external_echo_or_missing_destination_fallback() {
        let mut runtime = fixture();
        runtime.sync.preferences.clock.output = Some("unconnected-clock-output".into());
        assert_eq!(
            runtime.start_clock_output(clock_at(1_010_000)).unwrap_err(),
            "Clock echo is disabled"
        );
        assert!(runtime.sync.output.is_none());
        runtime.sync.preferences.clock.input = None;
        assert!(runtime.start_clock_output(clock_at(1_010_000)).is_err());
        assert!(runtime.sync.output.is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "requires CoreMIDI; creates an ephemeral virtual clock destination"]
    fn coremidi_clock_after_seek_shares_note_deadlines_and_cancels_future_pulses() {
        use crest_tracker::time::Tempo;
        use std::time::Duration;
        let mut runtime = fixture();
        runtime.sync.preferences.clock.input = None;
        let (_input, receiver, endpoint) = virtual_clock_destination(&mut runtime);
        runtime.sync.preferences.clock.output = Some(endpoint);
        runtime.transport_start = Beat::integer(16);
        runtime.tempo = TempoMap::new(Tempo::new(120_000).unwrap());
        runtime.transport_offset_frames =
            runtime.tempo.frame_at(Beat::integer(16), 48_000).unwrap();
        runtime.anchor = 4800;
        let now = host_micros().unwrap();
        let mut clock = super::super::tests::clock();
        clock.audio_clock =
            AudioFrameClock::new(0, now, now, 48_000, AudioClockDomain::CoreAudioHost);
        runtime.start_clock_output(clock).unwrap();
        runtime.send_clock_output(beat(129, 8), clock).unwrap();
        assert_eq!(
            runtime.frame_at_beat(Beat::integer(16), clock).unwrap(),
            4800
        );
        for expected in [
            vec![0xf2, 64, 0],
            vec![0xfb],
            vec![0xf8],
            vec![0xf8],
            vec![0xf8],
        ] {
            assert_eq!(
                receiver.recv_timeout(Duration::from_millis(500)).unwrap(),
                expected
            );
        }
        // Queue later pulses, then cancel before their deadlines. Exactly one
        // Stop may escape; an already-queued old-epoch pulse must not follow it.
        runtime.send_clock_output(Beat::integer(17), clock).unwrap();
        runtime.outputs.cancel().unwrap();
        assert_eq!(
            receiver.recv_timeout(Duration::from_millis(500)).unwrap(),
            [0xfc]
        );
        assert!(receiver.recv_timeout(Duration::from_millis(550)).is_err());
        assert_eq!(runtime.outputs.snapshot().error_count, 0);
        runtime.outputs.shutdown().unwrap();
    }

    #[cfg(target_os = "macos")]
    fn virtual_clock_destination(
        runtime: &mut Runtime,
    ) -> (
        midir::MidiInputConnection<()>,
        std::sync::mpsc::Receiver<Vec<u8>>,
        String,
    ) {
        use midir::os::unix::VirtualInput;
        use std::time::{Duration, Instant};
        let name = format!("Crest Tracker Clock Witness {}", uuid::Uuid::new_v4());
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut input = midir::MidiInput::new(&name).unwrap();
        input.ignore(midir::Ignore::None);
        let connection = input
            .create_virtual(
                &name,
                move |_, bytes, _| {
                    let _ = sender.send(bytes.to_vec());
                },
                (),
            )
            .unwrap();
        runtime.outputs.discover().unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let endpoint = loop {
            if let Some(endpoint) = runtime
                .outputs
                .snapshot()
                .endpoints
                .into_iter()
                .find(|e| e.name == name)
            {
                break endpoint.id;
            }
            assert!(
                Instant::now() < deadline,
                "virtual clock destination not discovered"
            );
            std::thread::sleep(Duration::from_millis(2));
        };
        runtime.outputs.connect(&endpoint).unwrap();
        while !runtime.outputs.snapshot().connected.contains(&endpoint) {
            assert!(
                Instant::now() < deadline,
                "virtual clock destination not connected"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        (connection, receiver, endpoint)
    }
}

fn browser_preferences(preferences: &MachinePreferences) -> BrowserPreferences {
    BrowserPreferences {
        sample_folder: preferences.sample_folder.clone(),
        soundfont_folder: preferences.soundfont_folder.clone(),
    }
}
fn load_preferences(
    directory: &std::path::Path,
    shared: Option<&std::path::Path>,
) -> Result<MachinePreferences, String> {
    let mut preferences = crate::preferences::load(directory)?;
    if let Some(shared) = shared {
        let folders = match browser_preferences::load(shared)? {
            Some(value) => value,
            None => {
                let legacy = browser_preferences(&preferences);
                if legacy != BrowserPreferences::default() {
                    browser_preferences::save(shared, &legacy, false)?;
                    browser_preferences::load(shared)?
                        .ok_or("Shared browser preferences disappeared during import")?
                } else {
                    legacy
                }
            }
        };
        preferences.sample_folder = folders.sample_folder;
        preferences.soundfont_folder = folders.soundfont_folder;
    }
    Ok(preferences)
}

#[cfg(test)]
mod browser_migration_tests {
    use super::*;
    #[test]
    fn legacy_roots_import_once_and_existing_shared_settings_win() {
        let root =
            std::env::temp_dir().join(format!("crest-browser-migration-{}", std::process::id()));
        let local = root.join("tracker");
        let shared = root.join("synth");
        let preferences = MachinePreferences {
            sample_folder: Some(root.display().to_string()),
            ..MachinePreferences::default()
        };
        crate::preferences::save(&local, &preferences).unwrap();
        let original = std::fs::read(local.join("preferences.json")).unwrap();
        assert_eq!(
            load_preferences(&local, Some(&shared)).unwrap(),
            preferences
        );
        assert_eq!(
            browser_preferences::load(&shared).unwrap(),
            Some(browser_preferences(&preferences))
        );
        browser_preferences::save(&shared, &BrowserPreferences::default(), true).unwrap();
        assert_eq!(
            load_preferences(&local, Some(&shared))
                .unwrap()
                .sample_folder,
            None
        );
        assert_eq!(
            std::fs::read(local.join("preferences.json")).unwrap(),
            original
        );
        std::fs::write(shared.join("browser-starts.json"), b"invalid").unwrap();
        assert!(load_preferences(&local, Some(&shared)).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
