//! Tracker-owned adapter to the independently versioned performance library.
//! The library owns musical behavior; Tracker owns time conversion and instrument bindings.
use crate::{
    composition::{InstrumentId, Song},
    time::Beat,
};
use crest_linnstrument::{Geometry, LedRenderer, MidiMessage, MidiParser, MpeZone, NoteInput};
use crest_performance_core::{
    self as library, BeatTime, Command, DestinationId, Engine, Limits, OutputEvent,
    PerformanceSnapshot, Session, SlotState, SourceId, SurfaceMode, TICKS_PER_BEAT,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Library(#[from] library::Error),
    #[error(transparent)]
    Time(#[from] crate::time::TimeError),
    #[error("Tracker time cannot be represented exactly in performance ticks; explicit quantization is required")]
    TimeResolution,
    #[error("{0}")]
    Binding(&'static str),
}
type Result<T> = std::result::Result<T, Error>;

/// Exact view conversion, including off-grid attacks and expression points.
pub fn tracker_time(at: BeatTime) -> Result<Beat> {
    Ok(Beat::new(at.0, TICKS_PER_BEAT)?)
}

/// Never silently rounds a Tracker edit into the library's fixed-point time.
pub fn performance_time(at: Beat) -> Result<BeatTime> {
    let scaled = u128::from(at.numerator()) * u128::from(TICKS_PER_BEAT);
    let denominator = u128::from(at.denominator());
    if scaled % denominator != 0 {
        return Err(Error::TimeResolution);
    }
    let ticks =
        u64::try_from(scaled / denominator).map_err(|_| crate::time::TimeError::Overflow)?;
    Ok(BeatTime(ticks))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Route {
    pub destination: DestinationId,
    pub instrument: InstrumentId,
}

/// Embedded host state; the library session remains its original versioned schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Document {
    pub version: u32,
    pub session: Session,
    pub routes: Vec<Route>,
}
impl Document {
    pub fn migrate(mut self) -> Result<Self> {
        self.session = self.session.migrate()?;
        self.validate()?;
        Ok(self)
    }
    pub fn validate(&self) -> Result<()> {
        if self.version != 1 {
            return Err(Error::Binding("unsupported performance binding version"));
        }
        self.session.validate()?;
        if self.session.tracks.is_empty() {
            return Err(Error::Binding("performance session needs an input track"));
        }
        let destinations: BTreeSet<_> = self
            .session
            .tracks
            .iter()
            .map(|track| track.instrument.destination)
            .collect();
        let configured: BTreeSet<_> = self.routes.iter().map(|route| route.destination).collect();
        if configured.len() != self.routes.len() || configured != destinations {
            return Err(Error::Binding(
                "performance destinations require exactly one instrument binding",
            ));
        }
        Ok(())
    }
    pub fn validate_instruments(&self, song: &Song) -> Result<()> {
        self.validate()?;
        if self
            .routes
            .iter()
            .any(|route| !song.instruments.contains_key(&route.instrument))
        {
            return Err(Error::Binding(
                "performance binding references an absent Tracker instrument",
            ));
        }
        Ok(())
    }
}

/// Single-owner, host-clock adapter. No sound, OS MIDI, GUI, filesystem or renderer work.
/// Callers drain musical output independently from returned configuration/LED messages.
pub struct Performance {
    engine: Engine,
    routes: Vec<Route>,
    input: NoteInput,
    parser: MidiParser,
    geometry: Geometry,
    leds: LedRenderer,
    mode: SurfaceMode,
    armed_slot: Option<usize>,
    pub launch_quantum: BeatTime,
}
impl Performance {
    pub fn new(document: Document, geometry: Geometry, limits: Limits) -> Result<Self> {
        let document = document.migrate()?;
        let track = document
            .session
            .tracks
            .first()
            .ok_or(Error::Binding("performance session needs an input track"))?
            .id;
        Ok(Self {
            engine: Engine::new(library::API_VERSION, document.session, limits)?,
            routes: document.routes,
            input: NoteInput::mpe(MpeZone::lower(), SourceId(1), track),
            parser: MidiParser::default(),
            geometry,
            leds: LedRenderer::new(geometry),
            mode: SurfaceMode::Play,
            armed_slot: None,
            launch_quantum: BeatTime(0),
        })
    }
    pub fn document(&self) -> Result<Document> {
        Ok(Document {
            version: 1,
            session: self.engine.save_session()?,
            routes: self.routes.clone(),
        })
    }
    /// A stopped recovery copy of the current take; input and playback continue.
    /// This clones and validates musical state on the control thread only.
    pub fn checkpoint_document(&self) -> Result<Document> {
        Ok(Document {
            version: 1,
            session: self.engine.checkpoint_session()?,
            routes: self.routes.clone(),
        })
    }
    pub fn checkpoint_document_at(&self, cutoff: Beat) -> Result<Document> {
        Ok(Document {
            version: 1,
            session: self
                .engine
                .checkpoint_session_at(performance_time(cutoff)?)?,
            routes: self.routes.clone(),
        })
    }
    pub fn snapshot(&self) -> PerformanceSnapshot {
        self.engine.snapshot()
    }
    /// Initial accepted launch boundary, independent of the scheduler horizon.
    pub fn slot_launch_time(&self, slot: usize) -> Result<Option<BeatTime>> {
        Ok(self.engine.slot_launch_time(slot)?)
    }
    /// Read the authoritative clip without converting its expression or timing.
    /// Hosts project notes only when opening or changing an editor, off the clock path.
    pub fn clip(&self, slot: usize) -> Result<Option<&library::Clip>> {
        self.engine
            .session()
            .slots
            .get(slot)
            .map(|slot| slot.clip.as_ref())
            .ok_or(Error::Binding("unknown clip slot"))
    }
    /// Start capture at the next accepted member attack, preserving source time.
    /// Idle UI ticks need not advance the input engine to establish a record boundary.
    pub fn arm_recording(&mut self, slot: usize) -> Result<()> {
        let snapshot = self.engine.snapshot();
        if self.mode == SurfaceMode::Clips
            || snapshot.active_voices != 0
            || snapshot
                .slots
                .iter()
                .any(|state| matches!(state, SlotState::Recording | SlotState::Overdubbing))
        {
            return Err(Error::Binding(
                "release notes and finish the current take before arming",
            ));
        }
        let input_track = self.engine.session().tracks[0].id;
        if self
            .engine
            .session()
            .slots
            .get(slot)
            .is_none_or(|slot| slot.track != input_track)
        {
            return Err(Error::Binding("record slot must belong to the input track"));
        }
        if matches!(snapshot.slots[slot], SlotState::Playing | SlotState::Queued) {
            return Err(Error::Binding("stop the selected slot before arming"));
        }
        self.armed_slot = Some(slot);
        Ok(())
    }
    /// Copy retained input at the host cutoff while keeping live time and ownership.
    pub fn capture_recent_at(
        &mut self,
        slot: usize,
        length: BeatTime,
        cutoff: BeatTime,
    ) -> Result<()> {
        self.require_idle_recorder()?;
        Ok(self.engine.capture_recent_at(slot, length, cutoff)?)
    }
    fn require_idle_recorder(&self) -> Result<()> {
        if self.armed_slot.is_some()
            || self
                .engine
                .snapshot()
                .slots
                .iter()
                .any(|state| matches!(state, SlotState::Recording | SlotState::Overdubbing))
        {
            return Err(Error::Binding(
                "finish or discard the current take before changing clips",
            ));
        }
        Ok(())
    }
    pub fn armed_slot(&self) -> Option<usize> {
        self.armed_slot
    }
    pub fn instrument(&self, destination: DestinationId) -> Result<InstrumentId> {
        self.routes
            .iter()
            .find(|route| route.destination == destination)
            .map(|route| route.instrument)
            .ok_or(Error::Binding("unbound performance destination"))
    }
    pub fn advance(&mut self, at: Beat) -> Result<()> {
        Ok(self.engine.advance(performance_time(at)?)?)
    }
    /// Partial bytes and interleaved realtime share the released device adapter.
    /// Errors retain earlier accepted work; the host must stop/cancel output on failure.
    pub fn accept_midi(&mut self, at: Beat, bytes: &[u8]) -> Result<()> {
        let at = performance_time(at)?;
        self.engine.advance(at)?;
        for byte in bytes {
            if let Some(message) = self.parser.feed(*byte) {
                self.accept_message(at, message)?;
            }
        }
        Ok(())
    }
    fn accept_message(&mut self, at: BeatTime, message: MidiMessage) -> Result<()> {
        if self.mode == SurfaceMode::Clips {
            if let Some(pad) = crest_linnstrument::user_pad(message, self.geometry)? {
                if pad.pressed && pad.row == 0 && pad.column > 0 {
                    self.toggle_slot(usize::from(pad.column - 1))?;
                }
            }
        } else {
            for event in self.input.accept(at, message)? {
                if matches!(event.gesture, library::Gesture::On { .. }) {
                    if let Some(slot) = self.armed_slot {
                        self.engine.command(Command::BeginRecord { slot })?;
                        self.armed_slot = None;
                    }
                }
                self.engine.input(*event)?;
            }
        }
        Ok(())
    }
    fn toggle_slot(&mut self, slot: usize) -> Result<()> {
        let snapshot = self.engine.snapshot();
        match snapshot.slots.get(slot) {
            Some(SlotState::Recorded) => self.engine.command(Command::Launch {
                slot,
                quantum: self.launch_quantum,
            })?,
            Some(SlotState::Playing | SlotState::Queued) => {
                self.engine.command(Command::StopSlot { slot })?
            }
            _ => {}
        }
        Ok(())
    }
    /// Returned mode configuration must be sent to the controller separately.
    pub fn command(&mut self, command: Command) -> Result<Vec<MidiMessage>> {
        if matches!(command, Command::CaptureRecent { .. } | Command::Undo) {
            self.require_idle_recorder()?;
        }
        self.engine.command(command)?;
        if matches!(
            command,
            Command::Stop
                | Command::SetMode(_)
                | Command::BeginRecord { .. }
                | Command::FinishRecord
                | Command::CancelRecord
        ) {
            self.armed_slot = None;
        }
        if matches!(command, Command::Stop | Command::SetMode(_)) {
            self.input.reset();
            self.parser = MidiParser::default();
        }
        if let Command::SetMode(mode) = command {
            self.mode = mode;
            self.leds.invalidate();
            return Ok(crest_linnstrument::user_mode(mode == SurfaceMode::Clips).to_vec());
        }
        Ok(vec![])
    }
    pub fn drain_output(&mut self) -> impl Iterator<Item = OutputEvent> + '_ {
        self.engine.drain_output()
    }
    pub fn led_commands(&mut self, cell_budget: usize) -> Result<Vec<MidiMessage>> {
        if self.mode != SurfaceMode::Clips {
            return Ok(vec![]);
        }
        let mut frame = [[0; 8]; 26];
        for (slot, state) in self.engine.snapshot().slots.iter().take(25).enumerate() {
            frame[slot + 1][0] = crest_linnstrument::slot_color(*state);
        }
        Ok(self.leds.diff(&frame, cell_budget)?)
    }
}
