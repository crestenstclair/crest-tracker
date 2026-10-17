//! Device control state belongs to the host; musical launch uses the same
//! commands as the workspace. Display intent is never a simulated LED frame.
use super::*;
use crate::midi_output::{MidiOutputError, SurfaceLease};
use crest_linnstrument::{MidiParser, PadEvent};
use crest_performance_core::SlotState;
use std::time::{Duration, Instant};

const MODE_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Default)]
struct Nrpn {
    msb: Option<u8>,
    lsb: Option<u8>,
    data: Option<u8>,
}
impl Nrpn {
    fn accept(&mut self, number: u8, value: u8) -> Option<bool> {
        match number {
            99 => {
                self.msb = Some(value);
                self.data = None;
            }
            98 => {
                self.lsb = Some(value);
                self.data = None;
            }
            100 | 101 => *self = Self::default(),
            6 => self.data = Some(value),
            38 if self.msb == Some(1)
                && self.lsb == Some(117)
                && self.data == Some(0)
                && value <= 1 =>
            {
                self.data = None;
                return Some(value == 1);
            }
            _ => {}
        }
        None
    }
}

pub(in crate::synth) struct Surface {
    pub endpoint: String,
    lease: SurfaceLease,
    geometry: Geometry,
    revision: u64,
    overflow: u64,
    parser: MidiParser,
    nrpn: [Nrpn; 16],
    held: [[bool; 8]; 26],
    pub ready: bool,
    reported_mode: Option<bool>,
    confirmed: bool,
    restoring: Option<u64>,
    deadline: Instant,
    frame: Option<[[u8; 8]; 26]>,
    pub fault: Option<String>,
}
impl Surface {
    fn new(
        endpoint: String,
        lease: SurfaceLease,
        geometry: Geometry,
        revision: u64,
        overflow: u64,
    ) -> Self {
        Self {
            endpoint,
            lease,
            geometry,
            revision,
            overflow,
            parser: MidiParser::default(),
            nrpn: [Nrpn::default(); 16],
            held: [[false; 8]; 26],
            ready: false,
            reported_mode: None,
            confirmed: false,
            restoring: None,
            deadline: Instant::now() + MODE_TIMEOUT,
            frame: None,
            fault: None,
        }
    }

    pub fn pads(&self) -> u16 {
        match self.geometry {
            Geometry::Pads128 => 128,
            Geometry::Pads200 => 200,
        }
    }

    pub fn status(&self) -> &'static str {
        if self.fault.is_some() {
            "INTERRUPTED · Return to Play to retry"
        } else if self.restoring.is_some() {
            "RESTORING PLAY"
        } else if self.ready {
            "CLIPS · Bottom row launches/stops"
        } else {
            "WAITING FOR FIRMWARE"
        }
    }

    fn accept(&mut self, bytes: &[u8]) -> Result<Option<usize>, String> {
        let mut slot = None;
        for byte in bytes {
            let Some(message) = self.parser.feed(*byte) else {
                continue;
            };
            let [status, number, value] = message.bytes();
            if status & 0xf0 == 0xb0 {
                if let Some(enabled) = self.nrpn[usize::from(status & 15)].accept(number, value) {
                    self.reported_mode = Some(enabled);
                    self.confirmed |= enabled;
                    self.ready = enabled && self.restoring.is_none() && self.fault.is_none();
                    if enabled == self.restoring.is_none() {
                        self.deadline = Instant::now() + MODE_TIMEOUT;
                    }
                }
                continue;
            }
            if !self.ready {
                continue;
            }
            if let Some(pad) =
                crest_linnstrument::user_pad(message, self.geometry).map_err(|e| e.to_string())?
            {
                if self.rising(pad) && pad.row == 0 && pad.column > 0 {
                    slot = Some(usize::from(pad.column - 1));
                }
            }
        }
        Ok(slot)
    }

    fn rising(&mut self, pad: PadEvent) -> bool {
        let held = &mut self.held[usize::from(pad.column)][usize::from(pad.row)];
        let rising = pad.pressed && !*held;
        *held = pad.pressed;
        rising
    }

    fn restore(&mut self, outputs: &MidiOutputService) -> Result<(), String> {
        self.restoring = Some(outputs.release_surface().map_err(|e| e.to_string())?);
        self.ready = false;
        self.reported_mode = None;
        self.held.fill([false; 8]);
        self.parser = MidiParser::default();
        self.nrpn.fill(Nrpn::default());
        self.deadline = Instant::now() + MODE_TIMEOUT;
        self.fault = None;
        Ok(())
    }

    fn display(
        &mut self,
        outputs: &MidiOutputService,
        slots: &[SlotState],
        playing: Option<(usize, SlotState)>,
    ) -> Result<(), String> {
        if self.restoring.is_some() || self.fault.is_some() {
            return Ok(());
        }
        let frame = clip_frame(self.geometry, slots, playing);
        if self.frame == Some(frame) {
            return Ok(());
        }
        match outputs.display_surface(&self.lease, self.geometry, frame) {
            Ok(()) => self.frame = Some(frame),
            Err(MidiOutputError::ConnectionBusy) => {} // Retry complete intent next control tick.
            Err(error) => return Err(error.to_string()),
        }
        Ok(())
    }
}

fn clip_frame(
    geometry: Geometry,
    slots: &[SlotState],
    playback: Option<(usize, SlotState)>,
) -> [[u8; 8]; 26] {
    let mut frame = [[0; 8]; 26];
    for (index, state) in slots
        .iter()
        .take(usize::from(geometry.columns() - 1))
        .enumerate()
    {
        frame[index + 1][0] = crest_linnstrument::slot_color(
            playback
                .filter(|(slot, _)| *slot == index)
                .map_or(*state, |(_, state)| state),
        );
    }
    frame
}

impl Runtime {
    pub(in crate::synth) fn enter_performance_clips(
        &mut self,
        sound: &mut Sound<'_>,
        endpoint: String,
        pads: u16,
    ) -> Result<(), String> {
        if self.performance_surface.is_some() {
            return Err("Return to Play before changing the controller".into());
        }
        if self.capture_baseline.is_some()
            || self.pending_open.is_some()
            || self.loading.is_some()
            || self.sync.external()
        {
            return Err(
                "Finish the document/take operation and select the local clock first".into(),
            );
        }
        let geometry = match pads {
            200 => Geometry::Pads200,
            128 => Geometry::Pads128,
            _ => return Err("Choose 128 or 200 pads".into()),
        };
        let revision = sound
            .active_midi_input_identity()
            .ok_or("Select the LinnStrument MIDI input in Settings first")?
            .revision()
            .value();
        let overflow = sound
            .midi_activity()
            .map_or(0, |input| input.overflow_epoch());
        let lease = self
            .outputs
            .acquire_surface(&endpoint)
            .map_err(|e| e.to_string())?;
        self.finish_performance_input(sound, false)?;
        let slots = self
            .performance
            .as_ref()
            .ok_or("Open a performance session first")?
            .snapshot()
            .slots;
        sound.release_default_input().map_err(|e| e.to_string())?;
        sound.cancel_monitor_schedule().map_err(|e| e.to_string())?;
        let mut surface = Surface::new(endpoint, lease, geometry, revision, overflow);
        surface.display(
            &self.outputs,
            &slots,
            self.performance_playback.as_ref().map(Playback::slot_state),
        )?;
        self.performance_surface = Some(surface);
        self.app.borrow_mut().status =
            "CLIPS · Waiting for firmware mode acknowledgement · Release held pads".into();
        Ok(())
    }

    pub(in crate::synth) fn restore_performance_surface(&mut self) -> Result<(), String> {
        if let Some(surface) = &mut self.performance_surface {
            surface.restore(&self.outputs)?;
        }
        Ok(())
    }

    pub(in crate::synth) fn performance_surface_input(
        &mut self,
        event: crest_synth::control::PhysicalMidiEvent,
    ) -> bool {
        let Some(surface) = &mut self.performance_surface else {
            return false;
        };
        if event.revision().value() != surface.revision {
            return false;
        }
        let (normalized, len) = live::normalized_bytes(event.message());
        let bytes = if event.raw_bytes().is_empty() {
            &normalized[..len]
        } else {
            event.raw_bytes()
        };
        let was_ready = surface.ready;
        match surface.accept(bytes) {
            Ok(Some(slot)) => self
                .app
                .borrow_mut()
                .requests
                .push(Request::PerformancePad { slot }),
            Ok(None) => {}
            Err(error) => {
                surface.ready = false;
                surface.fault = Some(error.clone());
                self.app.borrow_mut().status = error;
            }
        }
        if !was_ready && surface.ready {
            self.app.borrow_mut().status =
                "CLIPS · Firmware confirmed · Bottom row launches/stops clips".into();
        }
        true
    }

    pub(in crate::synth) fn advance_performance_surface(
        &mut self,
        sound: &Sound<'_>,
    ) -> Result<(), String> {
        let Some(surface) = &mut self.performance_surface else {
            return Ok(());
        };
        let snapshot = self.outputs.snapshot();
        let source_valid = sound
            .active_midi_input_identity()
            .is_some_and(|input| input.revision().value() == surface.revision)
            && sound
                .midi_activity()
                .is_none_or(|input| input.overflow_epoch() == surface.overflow);
        let connected = snapshot.connected.contains(&surface.endpoint);
        if surface.restoring.is_none()
            && (!source_valid
                || !connected
                || (surface.confirmed && surface.reported_mode == Some(false))
                || surface.fault.is_some())
        {
            let fault = surface.fault.take();
            surface.restore(&self.outputs)?;
            self.app.borrow_mut().status = fault.unwrap_or_else(|| {
                "LinnStrument mode or connection changed; restoring Play".into()
            });
        }
        if surface
            .restoring
            .is_some_and(|generation| snapshot.settled_surface_generation >= generation)
            && (surface.reported_mode == Some(false) || !source_valid || !connected)
        {
            self.performance_surface = None;
            if source_valid && connected {
                self.app.borrow_mut().status = "PLAY · Firmware confirmed".into();
            }
            return Ok(());
        }
        if Instant::now() >= surface.deadline && surface.fault.is_none() {
            surface.ready = false;
            surface.fault = Some("LinnStrument mode acknowledgement timed out; check input/feedback pairing and Return to Play".into());
            self.app.borrow_mut().status = surface.fault.clone().unwrap();
            // Keep consuming controller packets until restoration is acknowledged.
        }
        let slots = self
            .performance
            .as_ref()
            .map(|p| p.snapshot().slots)
            .unwrap_or_default();
        surface.display(
            &self.outputs,
            &slots,
            self.performance_playback.as_ref().map(Playback::slot_state),
        )
    }

    pub(in crate::synth) fn performance_pad(
        &mut self,
        sound: &mut Sound<'_>,
        clock: HostClock,
        slot: usize,
    ) -> Result<(), String> {
        if !self
            .performance_surface
            .as_ref()
            .is_some_and(|surface| surface.ready)
        {
            return Ok(());
        }
        if self
            .performance_playback
            .as_ref()
            .is_some_and(|p| p.slot == slot)
        {
            return self.stop(sound);
        }
        if self
            .performance
            .as_ref()
            .is_some_and(|p| p.snapshot().slots.get(slot) == Some(&SlotState::Recorded))
        {
            self.audition_performance(sound, clock, slot)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn queued_and_playing_feedback_use_audio_boundary_state_and_leave_other_clips_alone() {
        let slots = [SlotState::Recorded, SlotState::Recorded, SlotState::Empty];
        for geometry in [Geometry::Pads128, Geometry::Pads200] {
            let queued = clip_frame(geometry, &slots, Some((1, SlotState::Queued)));
            assert_eq!((queued[1][0], queued[2][0], queued[3][0]), (5, 2, 0));
            let playing = clip_frame(geometry, &slots, Some((1, SlotState::Playing)));
            assert_eq!((playing[1][0], playing[2][0], playing[3][0]), (5, 3, 0));
            assert_eq!(clip_frame(geometry, &slots, None)[2][0], 5);
            assert_eq!(playing[0], [0; 8], "control column belongs to firmware");
        }
    }

    fn fixture() -> (Surface, MidiOutputService) {
        let (outputs, _driver) = MidiOutputService::recording_for_test("feedback", 8);
        let lease = outputs.acquire_surface("feedback").unwrap();
        (
            Surface::new("feedback".into(), lease, Geometry::Pads200, 1, 0),
            outputs,
        )
    }
    fn report(surface: &mut Surface, channel: u8, enabled: bool) {
        for bytes in [
            [0xb0 | channel, 99, 1],
            [0xb0 | channel, 98, 117],
            [0xb0 | channel, 6, 0],
            [0xb0 | channel, 38, u8::from(enabled)],
        ] {
            assert_eq!(surface.accept(&bytes).unwrap(), None);
        }
    }
    #[test]
    fn firmware_acknowledgement_gates_pad_edges_and_return_to_play() {
        let (mut surface, outputs) = fixture();
        assert_eq!(surface.accept(&[0x90, 1, 100]).unwrap(), None);
        assert!(!surface.ready);
        report(&mut surface, 8, true);
        assert!(surface.ready);
        assert_eq!(surface.accept(&[0x90, 1, 100]).unwrap(), Some(0));
        assert_eq!(surface.accept(&[1, 110]).unwrap(), None); // Running-status pressure/repeat is not a fresh press.
        assert_eq!(surface.accept(&[0xf8, 1, 0]).unwrap(), None);
        assert_eq!(surface.accept(&[1, 100]).unwrap(), Some(0));
        assert_eq!(surface.accept(&[0x91, 2, 100]).unwrap(), None); // Only the bottom clip row launches.
        assert_eq!(surface.accept(&[0x90, 0, 100]).unwrap(), None); // Firmware owns the control column.
        surface.restore(&outputs).unwrap();
        assert!(!surface.ready);
        assert_eq!(surface.accept(&[0x90, 2, 100]).unwrap(), None);
        report(&mut surface, 0, false);
        assert_eq!(surface.reported_mode, Some(false));
        assert!(!surface.ready);
    }
    #[test]
    fn mode_reports_keep_channel_and_parameter_ownership() {
        let (mut surface, _) = fixture();
        surface.accept(&[0xb8, 99, 1]).unwrap();
        surface.accept(&[0xb8, 98, 117]).unwrap();
        surface.accept(&[0xb7, 6, 0]).unwrap();
        surface.accept(&[0xb7, 38, 1]).unwrap();
        assert!(!surface.ready);
        surface.accept(&[0xb8, 6, 0]).unwrap();
        surface.accept(&[0xb8, 100, 0]).unwrap();
        surface.accept(&[0xb8, 38, 1]).unwrap();
        assert!(!surface.ready);
        report(&mut surface, 8, true);
        assert!(surface.accept(&[0x98, 1, 100]).is_err());
        report(&mut surface, 8, false);
        assert!(!surface.ready);
    }
}
