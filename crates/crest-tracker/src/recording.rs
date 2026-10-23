//! Fixed-loop, additive MIDI performance capture. The host supplies exact beat timestamps.
//! Ordinary channel controls remain channel-wide. Explicit MPE profiles enable
//! scoped expression capture without inferring a source's channel layout.
use crate::composition::*;
use crate::editing::{Command, Editor, Occurrence};
use crate::time::{Beat, TimeError};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;
use thiserror::Error;
mod edit_guard;
mod mpe_capture;
use crate::mpe::{MpeEvent, MpeProfile};
pub use edit_guard::RecordingEditGuard;
use mpe_capture::{MpeCapture, MpeCaptureCheckpoint};

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RecordingError {
    #[error(transparent)]
    Document(#[from] DocumentError),
    #[error(transparent)]
    Time(#[from] TimeError),
    #[error("recording transition is unavailable: {0}")]
    State(&'static str),
    #[error("input timestamps must not move backwards")]
    Backwards,
    #[error("end take must be a fixed-loop boundary")]
    NotLoopBoundary,
    #[error("capture packet capacity exceeded; the interrupted take is retained")]
    Capacity,
    #[error("invalid recording checkpoint: {0}")]
    Checkpoint(&'static str),
    #[error("recording checkpoint belongs to a different song baseline")]
    BaselineChanged,
    #[error(transparent)]
    Mpe(#[from] crate::mpe::MpeError),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum CaptureState {
    Idle,
    Armed,
    Waiting { boundary: Beat },
    Recording { started_at: Beat },
    Interrupted { reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureMessage {
    NoteOn { pitch: u8, velocity: u8 },
    NoteOff { pitch: u8, velocity: u8 },
    Control(Control),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureInput {
    pub at: Beat,
    pub source_channel: Channel,
    pub target: Target,
    pub message: CaptureMessage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CaptureDiagnostic {
    RepeatedAttack {
        channel: Channel,
        pitch: u8,
        at: Beat,
    },
    UnmatchedRelease {
        channel: Channel,
        pitch: u8,
        at: Beat,
    },
}

/// Captured content remains available after interruption or a stale commit.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TakeDraft {
    pub phrase: PhraseId,
    pub occurrence: Occurrence,
    pub expected_revision: u64,
    pub started_at: Beat,
    pub length: Beat,
    pub events: Vec<Event>,
    pub diagnostics: Vec<CaptureDiagnostic>,
}

#[derive(Debug, Clone)]
pub struct PendingTake {
    pub expected_revision: u64,
    pub command: Command,
}

#[derive(Debug, Clone)]
struct Setup {
    song: Arc<Song>,
    phrase: PhraseId,
    occurrence: Occurrence,
    revision: u64,
    length: Beat,
    first_identity: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Attack {
    id: EventId,
    at: Beat,
    target: Target,
    pitch: u8,
    velocity: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum Held {
    Active(Attack),
    Retired(EventId),
}

/// Portable, versioned recovery data. Decoding does not create a live recorder;
/// `Recorder::recover` validates every relationship and the exact Song baseline.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordingCheckpoint {
    version: u32,
    state: CaptureState,
    setup: Option<CheckpointSetup>,
    draft: Option<TakeDraft>,
    held: Vec<CheckpointHeldQueue>,
    next_identity: u64,
    last_input: Option<Beat>,
    packets: usize,
    capacity: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mpe: Option<MpeCaptureCheckpoint>,
}

impl RecordingCheckpoint {
    /// Stored draft events; still-held voices remain in their separate builders.
    pub fn draft_event_count(&self) -> usize {
        self.draft.as_ref().map_or(0, |draft| draft.events.len())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckpointSetup {
    song: Song,
    phrase: PhraseId,
    occurrence: Occurrence,
    revision: u64,
    length: Beat,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckpointHeldQueue {
    channel: Channel,
    pitch: u8,
    entries: VecDeque<Held>,
}

#[derive(Debug, Clone)]
pub struct Recorder {
    state: CaptureState,
    setup: Option<Setup>,
    draft: Option<TakeDraft>,
    held: BTreeMap<(Channel, u8), VecDeque<Held>>,
    next_identity: u64,
    last_input: Option<Beat>,
    packets: usize,
    capacity: usize,
    mpe: Option<MpeCapture>,
}

impl Default for Recorder {
    fn default() -> Self {
        Self::with_capacity(65_536).expect("the default capture capacity is positive")
    }
}

impl Recorder {
    /// Capacity counts input packets, including releases and controls, not only completed notes.
    pub fn with_capacity(capacity: usize) -> Result<Self, RecordingError> {
        if capacity == 0 {
            return Err(RecordingError::Capacity);
        }
        Ok(Self {
            state: CaptureState::Idle,
            setup: None,
            draft: None,
            held: BTreeMap::new(),
            next_identity: 1,
            last_input: None,
            packets: 0,
            capacity,
            mpe: None,
        })
    }
    pub fn state(&self) -> &CaptureState {
        &self.state
    }
    pub fn draft(&self) -> Option<&TakeDraft> {
        self.draft.as_ref()
    }
    /// The pinned occurrence and source phrase remain stable through navigation
    /// and retained-take review, until commit or discard clears the setup.
    pub fn armed_phrase(&self) -> Option<(Occurrence, PhraseId)> {
        self.setup
            .as_ref()
            .map(|setup| (setup.occurrence, setup.phrase))
    }
    pub fn held_count(&self) -> usize {
        if let Some(mpe) = &self.mpe {
            return mpe.active_count();
        }
        self.held
            .values()
            .flatten()
            .filter(|held| matches!(held, Held::Active(_)))
            .count()
    }

    /// Copies pure capture data, including attacks that have not received a
    /// release and retired FIFO entries still awaiting their original release.
    pub fn checkpoint(&self) -> RecordingCheckpoint {
        RecordingCheckpoint {
            version: 1,
            state: self.state.clone(),
            setup: self.setup.as_ref().map(|setup| CheckpointSetup {
                song: (*setup.song).clone(),
                phrase: setup.phrase,
                occurrence: setup.occurrence,
                revision: setup.revision,
                length: setup.length,
            }),
            draft: self.draft.clone(),
            held: self
                .held
                .iter()
                .map(|(&(channel, pitch), entries)| CheckpointHeldQueue {
                    channel,
                    pitch,
                    entries: entries.clone(),
                })
                .collect(),
            next_identity: self.next_identity,
            last_input: self.last_input,
            packets: self.packets,
            capacity: self.capacity,
            mpe: self.mpe.as_ref().map(MpeCapture::checkpoint),
        }
    }

    /// Restores a stopped take only against its exact original Song. A reopened
    /// editor may restart revision numbering; matching content permits that
    /// explicit rebase. Neither the editor nor an existing recorder is mutated.
    /// Call `finish` at a loop boundary to close recovered held attacks, then
    /// `commit` for one undoable transaction. Recovery never resumes capture.
    pub fn recover(
        mut checkpoint: RecordingCheckpoint,
        editor: &Editor,
    ) -> Result<Self, RecordingError> {
        checkpoint.validate()?;
        let mpe = checkpoint
            .mpe
            .take()
            .map(MpeCapture::restore_validated)
            .transpose()?;
        let setup = checkpoint
            .setup
            .ok_or(RecordingError::Checkpoint("no armed baseline"))?;
        if setup.song != *editor.song() {
            return Err(RecordingError::BaselineChanged);
        }
        if let Some(draft) = &mut checkpoint.draft {
            draft.expected_revision = editor.revision();
        }
        let reason = match checkpoint.state {
            CaptureState::Interrupted { reason } => format!("recovered checkpoint: {reason}"),
            _ => "recovered checkpoint; capture remains stopped".to_owned(),
        };
        Ok(Self {
            state: CaptureState::Interrupted { reason },
            setup: Some(Setup {
                first_identity: setup.song.next_identity()?,
                song: Arc::new(setup.song),
                phrase: setup.phrase,
                occurrence: setup.occurrence,
                revision: editor.revision(),
                length: setup.length,
            }),
            draft: checkpoint.draft,
            held: checkpoint
                .held
                .into_iter()
                .map(|queue| ((queue.channel, queue.pitch), queue.entries))
                .collect(),
            next_identity: checkpoint.next_identity,
            last_input: checkpoint.last_input,
            packets: checkpoint.packets,
            capacity: checkpoint.capacity,
            mpe,
        })
    }

    /// The caller must resolve Edit Shared / Make Unique before arming this exact path.
    pub fn arm(&mut self, editor: &Editor, occurrence: Occurrence) -> Result<(), RecordingError> {
        if self.state != CaptureState::Idle || self.draft.is_some() {
            return Err(RecordingError::State(
                "finish, commit or discard the current take first",
            ));
        }
        let phrase = resolve_occurrence(editor.song(), occurrence)?;
        let length = editor.song().phrases[&phrase].length()?;
        let next_identity = editor.song().next_identity()?;
        self.setup = Some(Setup {
            song: Arc::new(editor.song().clone()),
            phrase,
            occurrence,
            revision: editor.revision(),
            length,
            first_identity: next_identity,
        });
        self.next_identity = next_identity;
        self.state = CaptureState::Armed;
        self.mpe = None;
        Ok(())
    }

    /// Arm an explicitly declared MPE source. Source controls can prime its
    /// decoder before recording, but no pre-roll notes enter the take.
    pub fn arm_mpe(
        &mut self,
        editor: &Editor,
        occurrence: Occurrence,
        profile: MpeProfile,
    ) -> Result<(), RecordingError> {
        let mpe = MpeCapture::new(profile, self.capacity)?;
        self.arm(editor, occurrence)?;
        self.mpe = Some(mpe);
        Ok(())
    }
    pub fn mpe_profile(&self) -> Option<&MpeProfile> {
        self.mpe.as_ref().map(MpeCapture::profile)
    }
    pub fn take_mpe_monitor_events(&mut self) -> Vec<MpeEvent> {
        self.mpe
            .as_mut()
            .map_or_else(Vec::new, MpeCapture::take_monitor_events)
    }

    /// Primes a declared MPE source before the host's recording clock is ready.
    /// Unlike `input`, this does not advance a waiting boundary, even when the
    /// supplied timestamp equals it. No draft notes are created; decoded voices
    /// still prevent recording from starting until they have been released.
    pub fn prime_mpe(&mut self, input: CaptureInput) -> Result<(), RecordingError> {
        if self.mpe.is_none()
            || !matches!(
                self.state,
                CaptureState::Armed | CaptureState::Waiting { .. }
            )
            || self.draft.is_some()
        {
            return Err(RecordingError::State(
                "MPE pre-roll requires an armed or waiting source without a take",
            ));
        }
        self.input_mpe(input).map(|_| ())
    }

    /// The host chooses the next eligible start of the armed occurrence, beyond delivered output.
    pub fn request_record(&mut self, next_boundary: Beat) -> Result<(), RecordingError> {
        if self.state != CaptureState::Armed {
            return Err(RecordingError::State("Record requires Armed"));
        }
        if let Some(mpe) = &self.mpe {
            mpe.check_start(next_boundary)?;
        }
        self.state = CaptureState::Waiting {
            boundary: next_boundary,
        };
        Ok(())
    }
    /// A transport Start/SPP may establish its origin after Record was requested.
    /// Only an empty waiting take can move; accepted performance is never retimed.
    pub fn reschedule_waiting(&mut self, next_boundary: Beat) -> Result<(), RecordingError> {
        if !matches!(self.state, CaptureState::Waiting { .. }) || self.draft.is_some() {
            return Err(RecordingError::State(
                "only a waiting take can change its start",
            ));
        }
        if let Some(mpe) = &self.mpe {
            mpe.check_start(next_boundary)?;
        }
        self.state = CaptureState::Waiting {
            boundary: next_boundary,
        };
        Ok(())
    }

    /// First eligible start at or after a transport position, aligned to the
    /// frozen armed phrase length. Navigation and later edits cannot retime it.
    pub fn next_start_boundary(&self, after: Beat) -> Result<Beat, RecordingError> {
        if !matches!(
            self.state,
            CaptureState::Armed | CaptureState::Waiting { .. }
        ) {
            return Err(RecordingError::State(
                "start boundary requires an armed take",
            ));
        }
        let length = self.setup.as_ref().expect("armed take has a setup").length;
        let cycles = after.units(length)?;
        let boundary = length.checked_scale(cycles, 1)?;
        if boundary == after {
            Ok(boundary)
        } else {
            Ok(boundary.checked_add(length)?)
        }
    }

    /// End strictly after the latest accepted input, even after reopening a
    /// checkpoint whose original device/transport clock no longer exists.
    pub fn next_loop_boundary(&self, after: Beat) -> Result<Beat, RecordingError> {
        let draft = self
            .draft
            .as_ref()
            .ok_or(RecordingError::State("recording has not started"))?;
        let after = after
            .max(self.last_input.unwrap_or(draft.started_at))
            .max(draft.started_at);
        let cycles = after.checked_sub(draft.started_at)?.units(draft.length)?;
        let cycles = cycles
            .checked_add(1)
            .ok_or(RecordingError::State("loop boundary exhausted"))?;
        Ok(draft
            .started_at
            .checked_add(draft.length.checked_scale(cycles, 1)?)?)
    }

    pub fn advance(&mut self, at: Beat) {
        let CaptureState::Waiting { boundary } = self.state else {
            return;
        };
        if at < boundary {
            return;
        }
        if let Some(mpe) = &self.mpe {
            if let Err(error) = mpe.check_start(boundary) {
                let _ = self.close_mpe(boundary);
                self.state = CaptureState::Interrupted {
                    reason: error.to_string(),
                };
                return;
            }
        }
        let setup = self.setup.as_ref().expect("Waiting has an armed setup");
        self.draft = Some(TakeDraft {
            phrase: setup.phrase,
            occurrence: setup.occurrence,
            expected_revision: setup.revision,
            started_at: boundary,
            length: setup.length,
            events: Vec::new(),
            diagnostics: Vec::new(),
        });
        self.last_input = Some(boundary);
        self.state = CaptureState::Recording {
            started_at: boundary,
        };
    }

    /// Returns false before recording begins or after capture stops. Explicit
    /// MPE sources also queue monitor events while armed or waiting.
    pub fn input(&mut self, input: CaptureInput) -> Result<bool, RecordingError> {
        self.advance(input.at);
        if self.mpe.is_some() {
            return self.input_mpe(input);
        }
        if !matches!(self.state, CaptureState::Recording { .. }) {
            return Ok(false);
        }
        self.validate_input(&input)?;
        if self.packets >= self.capacity {
            // The new packet is not admitted; close known notes at the last accepted input.
            let at = self.last_input.expect("Recording has a start");
            let _ = self.interrupt(at, "capture packet capacity exceeded");
            return Err(RecordingError::Capacity);
        }
        match input.message {
            CaptureMessage::NoteOn { pitch, velocity: 0 }
            | CaptureMessage::NoteOff { pitch, velocity: 0 } => self.release(&input, pitch, 0)?,
            CaptureMessage::NoteOn { pitch, velocity } => self.attack(&input, pitch, velocity)?,
            CaptureMessage::NoteOff { pitch, velocity } => self.release(&input, pitch, velocity)?,
            CaptureMessage::Control(control) => self.control(&input, control)?,
        }
        self.last_input = Some(input.at);
        self.packets += 1;
        Ok(true)
    }

    /// Finish at a loop boundary; held keys close there, without waiting for sustain release.
    pub fn finish(&mut self, boundary: Beat) -> Result<(), RecordingError> {
        let draft = self.finishable_draft()?;
        if !within_loop(boundary, draft.started_at, draft.length)?.is_zero() {
            return Err(RecordingError::NotLoopBoundary);
        }
        self.finish_at(boundary)
    }

    /// Ends capture at an explicit timestamp without quantizing to a loop edge.
    /// The host owns this cutoff. It must not precede accepted input or produce
    /// a zero-duration held note; failure preserves all held notes and the draft.
    pub fn finish_at(&mut self, at: Beat) -> Result<(), RecordingError> {
        self.finishable_draft()?;
        if self.mpe.is_some() {
            self.close_mpe(at)?;
        } else {
            self.close_held(at)?;
        }
        self.state = CaptureState::Idle;
        Ok(())
    }

    fn finishable_draft(&self) -> Result<&TakeDraft, RecordingError> {
        if !matches!(
            self.state,
            CaptureState::Recording { .. } | CaptureState::Interrupted { .. }
        ) {
            return Err(RecordingError::State(
                "there is no active or interrupted take",
            ));
        }
        self.draft
            .as_ref()
            .ok_or(RecordingError::State("no captured take"))
    }

    /// No automatic resume/rearm. If zero-duration held notes cannot close, they remain retained.
    pub fn interrupt(&mut self, at: Beat, reason: impl Into<String>) -> Result<(), RecordingError> {
        if matches!(
            self.state,
            CaptureState::Idle | CaptureState::Interrupted { .. }
        ) {
            return Err(RecordingError::State("capture is not active"));
        }
        let result = if self.mpe.is_some() {
            self.close_mpe(at)
        } else if self.draft.is_some() {
            self.close_held(at)
        } else {
            Ok(())
        };
        self.state = CaptureState::Interrupted {
            reason: reason.into(),
        };
        result
    }

    pub fn prepare_commit(&self, editor: &Editor) -> Result<PendingTake, RecordingError> {
        let draft = self.stopped_draft()?;
        if editor.revision() != draft.expected_revision {
            return Err(DocumentError::Stale {
                expected: draft.expected_revision,
                actual: editor.revision(),
            }
            .into());
        }
        if resolve_occurrence(editor.song(), draft.occurrence)? != draft.phrase {
            return Err(DocumentError::Invalid("the armed occurrence was relinked").into());
        }
        let mut candidate = editor.song().clone();
        let phrase = candidate
            .phrases
            .get_mut(&draft.phrase)
            .expect("resolved occurrence has its phrase");
        if phrase.length()? != draft.length {
            return Err(DocumentError::Invalid("the recorded loop length changed").into());
        }
        phrase.events.extend(draft.events.iter().cloned());
        phrase.events.sort_by_key(|event| (event.at, event.id));
        let command = Command::PutPhrase {
            id: draft.phrase,
            phrase: phrase.clone(),
        };
        candidate.validate()?;
        Ok(PendingTake {
            expected_revision: draft.expected_revision,
            command,
        })
    }

    /// One editor transaction; failed commits leave the take available for recovery.
    pub fn commit(&mut self, editor: &mut Editor) -> Result<bool, RecordingError> {
        let pending = self.prepare_commit(editor)?;
        let changed = apply_finalized_take(editor, pending)?;
        self.discard();
        Ok(changed)
    }

    /// Saves the armed baseline phrase plus the retained raw take as a new
    /// Phrase and Chain in an empty slot (or directly after the last slot).
    /// The original occurrence need not still exist or match its old revision.
    /// Every copied event receives a fresh identity; current edits and resource
    /// targets remain untouched. One successful transaction clears the recorder
    /// and can be undone once. Any failure leaves both editor and take intact.
    pub fn save_as_new_phrase(
        &mut self,
        editor: &mut Editor,
        track: TrackId,
        slot: usize,
    ) -> Result<Occurrence, RecordingError> {
        let draft = self.stopped_draft()?;
        let setup = self
            .setup
            .as_ref()
            .ok_or(RecordingError::State("the take has no armed baseline"))?;
        let mut destination = empty_take_destination(editor.song(), track, slot)?;
        // Captured identities may exceed every current identity, including
        // after deletion or Undo. Copies must also differ from retained events.
        let mut next = editor.song().next_identity()?.max(self.next_identity);
        let phrase_id = PhraseId::new(allocate_take_identity(&mut next)?)?;
        let chain_id = ChainId::new(allocate_take_identity(&mut next)?)?;
        let phrase_use = UseId::new(allocate_take_identity(&mut next)?)?;
        let chain_use = UseId::new(allocate_take_identity(&mut next)?)?;
        let phrase = recovered_phrase(setup, draft, &mut next)?;
        let chain = Chain {
            name: format!("Recovered {}", phrase.name),
            phrases: vec![PhraseUse {
                id: phrase_use,
                phrase: phrase_id,
                repeats: 1,
            }],
        };
        let usage = Some(ChainUse {
            id: chain_use,
            chain: (chain_id).into(),
            repeats: 1,
        });
        if slot == destination.slots.len() {
            destination.slots.push(usage);
        } else {
            destination.slots[slot] = usage;
        }
        let expected_revision = editor.revision();
        apply_finalized_take(
            editor,
            PendingTake {
                expected_revision,
                command: Command::Batch {
                    commands: vec![
                        Command::PutPhrase {
                            id: phrase_id,
                            phrase,
                        },
                        Command::PutChain {
                            id: chain_id,
                            chain,
                        },
                        Command::PutTrack { track: destination },
                    ],
                },
            },
        )?;
        self.discard();
        Ok(Occurrence {
            track,
            chain_use,
            phrase_use,
        })
    }

    fn stopped_draft(&self) -> Result<&TakeDraft, RecordingError> {
        if !matches!(
            self.state,
            CaptureState::Idle | CaptureState::Interrupted { .. }
        ) || self.held_count() != 0
        {
            return Err(RecordingError::State("finish held notes before committing"));
        }
        self.draft
            .as_ref()
            .ok_or(RecordingError::State("there is no take to commit"))
    }

    /// Explicitly discard the armed setup and any recoverable draft.
    pub fn discard(&mut self) {
        self.state = CaptureState::Idle;
        self.setup = None;
        self.draft = None;
        self.held.clear();
        self.last_input = None;
        self.packets = 0;
        self.mpe = None;
    }

    fn input_mpe(&mut self, input: CaptureInput) -> Result<bool, RecordingError> {
        if !matches!(
            self.state,
            CaptureState::Armed | CaptureState::Waiting { .. } | CaptureState::Recording { .. }
        ) {
            return Ok(false);
        }
        let mpe = self.mpe.as_ref().expect("MPE path has decoder");
        if input.source_channel != mpe.profile().zone().master()
            && !mpe.profile().zone().contains_member(input.source_channel)
        {
            return Ok(false);
        }
        self.validate_input(&input)?;
        let recording = matches!(self.state, CaptureState::Recording { .. });
        let stage = mpe.stage_input(&input, self.draft.as_ref(), self.next_identity);
        let stage = match stage {
            Ok(stage) => stage,
            Err(error) => {
                if matches!(
                    error,
                    RecordingError::Capacity
                        | RecordingError::Mpe(
                            crate::mpe::MpeError::PacketCapacity
                                | crate::mpe::MpeError::VoiceCapacity
                        )
                ) {
                    let at = self
                        .last_input
                        .or_else(|| self.mpe.as_ref().and_then(MpeCapture::last_input))
                        .unwrap_or(input.at);
                    let _ = self.interrupt(at, "MPE capture capacity exceeded");
                }
                return Err(error);
            }
        };
        let (events, next_identity) = self
            .mpe
            .as_mut()
            .expect("MPE decoder retained")
            .commit_stage(stage);
        self.next_identity = next_identity;
        if recording {
            self.draft
                .as_mut()
                .expect("Recording has a draft")
                .events
                .extend(events);
            self.last_input = Some(input.at);
            self.packets += 1;
        }
        Ok(recording)
    }
    fn close_mpe(&mut self, at: Beat) -> Result<(), RecordingError> {
        if self.last_input.is_some_and(|last| at < last) {
            return Err(RecordingError::Backwards);
        }
        let stage = self
            .mpe
            .as_ref()
            .expect("MPE close has decoder")
            .stage_flush(at, self.draft.as_ref(), self.next_identity)?;
        let (events, next_identity) = self
            .mpe
            .as_mut()
            .expect("MPE decoder retained")
            .commit_stage(stage);
        self.next_identity = next_identity;
        if let Some(draft) = &mut self.draft {
            draft.events.extend(events);
            self.last_input = Some(at);
        }
        Ok(())
    }

    fn validate_input(&self, input: &CaptureInput) -> Result<(), RecordingError> {
        if self.last_input.is_some_and(|at| input.at < at) {
            return Err(RecordingError::Backwards);
        }
        self.setup
            .as_ref()
            .expect("Recording has a setup")
            .song
            .validate_target(&input.target)?;
        match input.message {
            CaptureMessage::NoteOn { pitch, velocity }
            | CaptureMessage::NoteOff { pitch, velocity }
                if pitch > 127 || velocity > 127 =>
            {
                Err(DocumentError::Invalid("capture note exceeds MIDI range").into())
            }
            CaptureMessage::Control(control) => Ok(control.validate()?),
            _ => Ok(()),
        }
    }

    fn allocate(&mut self) -> Result<EventId, RecordingError> {
        let next = self
            .next_identity
            .checked_add(1)
            .ok_or(DocumentError::Invalid("capture identity range exhausted"))?;
        let id = EventId::new(self.next_identity)?;
        self.next_identity = next;
        Ok(id)
    }

    fn attack(
        &mut self,
        input: &CaptureInput,
        pitch: u8,
        velocity: u8,
    ) -> Result<(), RecordingError> {
        let key = (input.source_channel, pitch);
        let old = match self.held.get(&key).and_then(|queue| queue.back()) {
            Some(Held::Active(attack)) => Some(self.completed(attack, input.at, 0)?),
            _ => None,
        };
        let id = self.allocate()?;
        let queue = self.held.entry(key).or_default();
        let draft = self.draft.as_mut().expect("Recording has a draft");
        if let Some(event) = old {
            *queue.back_mut().expect("the old attack is queued") = Held::Retired(event.id);
            draft.events.push(event);
            draft.diagnostics.push(CaptureDiagnostic::RepeatedAttack {
                channel: input.source_channel,
                pitch,
                at: input.at,
            });
        }
        queue.push_back(Held::Active(Attack {
            id,
            at: input.at,
            target: input.target.clone(),
            pitch,
            velocity,
        }));
        Ok(())
    }

    fn release(
        &mut self,
        input: &CaptureInput,
        pitch: u8,
        velocity: u8,
    ) -> Result<(), RecordingError> {
        let key = (input.source_channel, pitch);
        let Some(held) = self.held.get(&key).and_then(|queue| queue.front()).cloned() else {
            self.draft
                .as_mut()
                .expect("Recording has a draft")
                .diagnostics
                .push(CaptureDiagnostic::UnmatchedRelease {
                    channel: input.source_channel,
                    pitch,
                    at: input.at,
                });
            return Ok(());
        };
        match held {
            Held::Active(attack) => {
                let event = self.completed(&attack, input.at, velocity)?;
                self.draft
                    .as_mut()
                    .expect("Recording has a draft")
                    .events
                    .push(event);
            }
            Held::Retired(id) => {
                let draft = self.draft.as_mut().expect("Recording has a draft");
                let event = draft
                    .events
                    .iter_mut()
                    .find(|event| event.id == id)
                    .expect("retired attack was captured");
                event
                    .note
                    .as_mut()
                    .expect("retired event is a note")
                    .release_velocity = velocity;
            }
        }
        let queue = self.held.get_mut(&key).expect("release matched its queue");
        queue.pop_front();
        if queue.is_empty() {
            self.held.remove(&key);
        }
        Ok(())
    }

    fn control(&mut self, input: &CaptureInput, control: Control) -> Result<(), RecordingError> {
        let draft = self.draft.as_ref().expect("Recording has a draft");
        let at = within_loop(input.at, draft.started_at, draft.length)?;
        let id = self.allocate()?;
        self.draft
            .as_mut()
            .expect("Recording has a draft")
            .events
            .push(Event {
                id,
                at,
                target: input.target.clone(),
                note: None,
                controls: vec![control],
                fx: [None, None, None],
            });
        Ok(())
    }

    fn completed(&self, attack: &Attack, at: Beat, velocity: u8) -> Result<Event, RecordingError> {
        let draft = self.draft.as_ref().expect("a captured attack has a draft");
        let mut note = Note::new(attack.pitch, attack.velocity, at.checked_sub(attack.at)?)?;
        note.release_velocity = velocity;
        Ok(Event::note(
            attack.id,
            within_loop(attack.at, draft.started_at, draft.length)?,
            attack.target.clone(),
            note,
        ))
    }

    fn close_held(&mut self, at: Beat) -> Result<(), RecordingError> {
        if self.last_input.is_some_and(|last| at < last) {
            return Err(RecordingError::Backwards);
        }
        let completed = self
            .held
            .values()
            .flatten()
            .filter_map(|held| match held {
                Held::Active(attack) => Some(self.completed(attack, at, 0)),
                Held::Retired(_) => None,
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.draft
            .as_mut()
            .expect("closing held notes requires a draft")
            .events
            .extend(completed);
        self.held.clear();
        self.last_input = Some(at);
        Ok(())
    }
}

fn empty_take_destination(
    song: &Song,
    track: TrackId,
    slot: usize,
) -> Result<Track, DocumentError> {
    let destination = song
        .tracks
        .iter()
        .find(|candidate| candidate.id == track)
        .ok_or(DocumentError::Missing {
            kind: "track",
            id: track.get(),
        })?;
    if slot > destination.slots.len() {
        return Err(DocumentError::Invalid(
            "new phrase slot must already exist or append to track",
        ));
    }
    if destination.slots.get(slot).is_some_and(Option::is_some) {
        return Err(DocumentError::Invalid("slot already contains a chain"));
    }
    Ok(destination.clone())
}

/// Only a successfully finalized take can release the editor's capture guard.
/// A failed candidate preserves the guard, document, revision and undo history.
fn apply_finalized_take(editor: &mut Editor, pending: PendingTake) -> Result<bool, DocumentError> {
    let mut candidate = editor.clone();
    candidate.set_recording_guard(None);
    let changed = candidate.apply(pending.expected_revision, pending.command)?;
    *editor = candidate;
    Ok(changed)
}

fn allocate_take_identity(next: &mut u64) -> Result<u64, DocumentError> {
    let id = *next;
    *next = next
        .checked_add(1)
        .ok_or(DocumentError::Invalid("identity range exhausted"))?;
    Ok(id)
}

fn recovered_phrase(
    setup: &Setup,
    draft: &TakeDraft,
    next: &mut u64,
) -> Result<Phrase, RecordingError> {
    let mut phrase = setup
        .song
        .phrases
        .get(&setup.phrase)
        .ok_or(RecordingError::State(
            "the armed baseline phrase is missing",
        ))?
        .clone();
    phrase.events.extend(draft.events.iter().cloned());
    phrase.events.sort_by_key(|event| (event.at, event.id));
    for event in &mut phrase.events {
        event.id = EventId::new(allocate_take_identity(next)?)?;
    }
    Ok(phrase)
}

impl RecordingCheckpoint {
    fn validate(&self) -> Result<(), RecordingError> {
        checkpoint_require(self.version == 1, "unsupported checkpoint version")?;
        checkpoint_require(
            self.capacity > 0 && self.packets <= self.capacity,
            "invalid packet budget",
        )?;
        let setup = self
            .setup
            .as_ref()
            .ok_or(RecordingError::Checkpoint("no armed baseline"))?;
        setup.song.validate()?;
        checkpoint_require(
            resolve_occurrence(&setup.song, setup.occurrence)? == setup.phrase,
            "armed occurrence does not resolve to its phrase",
        )?;
        checkpoint_require(
            setup.song.phrases[&setup.phrase].length()? == setup.length,
            "armed loop length differs from its phrase",
        )?;
        let first_identity = setup.song.next_identity()?;
        if let Some(mpe) = &self.mpe {
            return mpe.validate(self, setup, first_identity);
        }
        let Some(draft) = &self.draft else {
            checkpoint_require(
                matches!(
                    self.state,
                    CaptureState::Armed
                        | CaptureState::Waiting { .. }
                        | CaptureState::Interrupted { .. }
                ),
                "capture state requires a draft",
            )?;
            return checkpoint_require(
                self.held.is_empty()
                    && self.last_input.is_none()
                    && self.packets == 0
                    && self.next_identity == first_identity,
                "uncaptured setup contains performance data",
            );
        };
        checkpoint_require(
            matches!(
                self.state,
                CaptureState::Idle
                    | CaptureState::Recording { .. }
                    | CaptureState::Interrupted { .. }
            ),
            "uncaptured state contains a draft",
        )?;
        if let CaptureState::Recording { started_at } = self.state {
            checkpoint_require(
                started_at == draft.started_at,
                "capture start differs from its draft",
            )?;
        }
        checkpoint_require(
            draft.phrase == setup.phrase
                && draft.occurrence == setup.occurrence
                && draft.expected_revision == setup.revision
                && draft.length == setup.length,
            "draft differs from its armed setup",
        )?;
        let last = self
            .last_input
            .ok_or(RecordingError::Checkpoint("captured take has no timestamp"))?;
        let elapsed = last.checked_sub(draft.started_at)?;
        checkpoint_require(
            draft.events.len() <= self.packets && draft.diagnostics.len() <= self.packets,
            "draft exceeds its captured packet count",
        )?;
        checkpoint_require(
            !matches!(self.state, CaptureState::Idle) || self.held.is_empty(),
            "finished capture still has held entries",
        )?;

        let mut allocated = BTreeSet::new();
        let mut events = BTreeMap::new();
        let mut note_count = 0usize;
        for event in &draft.events {
            checkpoint_require(allocated.insert(event.id), "duplicate captured identity")?;
            events.insert(event.id, event);
            checkpoint_require(
                event.at <= elapsed && event.at < draft.length,
                "event lies outside captured time",
            )?;
            checkpoint_require(
                event.fx.iter().all(Option::is_none),
                "raw capture contains authored FX",
            )?;
            match &event.note {
                Some(note) => {
                    note_count += 1;
                    note.validate()?;
                    checkpoint_require(
                        event.controls.is_empty()
                            && note.expression.is_empty()
                            && note.master_expression.is_empty()
                            && note.expression_duration.is_none()
                            && note.mpe.is_none(),
                        "raw captured note contains inferred controls or expression",
                    )?;
                    checkpoint_require(
                        event.at.checked_add(note.duration)? <= elapsed,
                        "note ends after captured time",
                    )?;
                }
                None => checkpoint_require(
                    event.controls.len() == 1,
                    "raw control packet has invalid payload",
                )?,
            }
        }
        let mut repeated = BTreeSet::new();
        let mut unmatched = 0usize;
        let mut previous_diagnostic = draft.started_at;
        for diagnostic in &draft.diagnostics {
            let (pitch, at) = match *diagnostic {
                CaptureDiagnostic::RepeatedAttack { channel, pitch, at } => {
                    checkpoint_require(
                        repeated.insert((channel, pitch, at)),
                        "duplicate repeated-attack diagnostic",
                    )?;
                    (pitch, at)
                }
                CaptureDiagnostic::UnmatchedRelease { pitch, at, .. } => {
                    unmatched += 1;
                    (pitch, at)
                }
            };
            checkpoint_require(
                pitch <= 127 && at >= previous_diagnostic && at <= last,
                "diagnostic lies outside captured input",
            )?;
            previous_diagnostic = at;
        }
        checkpoint_require(
            repeated.len() <= note_count,
            "repeated attacks lack completed notes",
        )?;

        let mut keys = BTreeSet::new();
        let mut retired = BTreeSet::new();
        let mut queued = 0usize;
        for queue in &self.held {
            checkpoint_require(
                queue.pitch <= 127 && keys.insert((queue.channel, queue.pitch)),
                "invalid or duplicate held queue",
            )?;
            let Some(Held::Active(attack)) = queue.entries.back() else {
                return Err(RecordingError::Checkpoint(
                    "held FIFO must end with its active attack",
                ));
            };
            checkpoint_require(
                attack.pitch == queue.pitch
                    && (1..=127).contains(&attack.velocity)
                    && attack.at >= draft.started_at
                    && attack.at <= last,
                "held attack has invalid pitch, velocity or time",
            )?;
            setup.song.validate_target(&attack.target)?;
            checkpoint_require(
                allocated.insert(attack.id),
                "held attack reuses a captured identity",
            )?;
            let mut next_attack_at = attack.at;
            let mut next_id = attack.id;
            for entry in queue.entries.iter().rev().skip(1) {
                let Held::Retired(id) = entry else {
                    return Err(RecordingError::Checkpoint(
                        "held FIFO contains multiple active attacks",
                    ));
                };
                checkpoint_require(
                    *id < next_id && retired.insert(*id),
                    "retired FIFO identity is duplicated or unordered",
                )?;
                let event = events.get(id).ok_or(RecordingError::Checkpoint(
                    "retired attack has no captured event",
                ))?;
                let note = event.note.as_ref().ok_or(RecordingError::Checkpoint(
                    "retired attack refers to a control",
                ))?;
                checkpoint_require(
                    note.pitch == queue.pitch && note.release_velocity == 0,
                    "retired attack has inconsistent note data",
                )?;
                checkpoint_require(
                    repeated.contains(&(queue.channel, queue.pitch, next_attack_at)),
                    "retired attack lacks its repeated-attack diagnostic",
                )?;
                next_attack_at = next_attack_at.checked_sub(note.duration)?;
                checkpoint_require(
                    within_loop(next_attack_at, draft.started_at, draft.length)? == event.at,
                    "retired attack timing disagrees with FIFO history",
                )?;
                next_id = *id;
            }
            queued = queued
                .checked_add(queue.entries.len())
                .ok_or(RecordingError::Checkpoint("held entry count overflow"))?;
        }
        let allocated_count = u64::try_from(allocated.len())
            .map_err(|_| RecordingError::Checkpoint("identity count overflow"))?;
        checkpoint_require(
            allocated.len() <= self.packets
                && queued <= allocated.len()
                && self.next_identity.checked_sub(first_identity) == Some(allocated_count)
                && allocated
                    .iter()
                    .all(|id| (first_identity..self.next_identity).contains(&id.get())),
            "capture identity range or budget is inconsistent",
        )?;
        checkpoint_require(
            unmatched <= self.packets - allocated.len(),
            "unmatched releases exceed packet count",
        )?;
        if matches!(self.state, CaptureState::Recording { .. }) || !self.held.is_empty() {
            let attacks = note_count + self.held.len();
            let releases = attacks
                .checked_sub(queued)
                .and_then(|matched| matched.checked_add(unmatched));
            checkpoint_require(
                releases.and_then(|releases| allocated.len().checked_add(releases))
                    == Some(self.packets),
                "recording packets disagree with pending release FIFO",
            )?;
        }
        // The ordinary document validator checks targets, controls and all
        // candidate references before any recovered state can become live.
        let mut candidate = setup.song.clone();
        candidate
            .phrases
            .get_mut(&setup.phrase)
            .ok_or(RecordingError::Checkpoint("armed phrase is missing"))?
            .events
            .extend(draft.events.iter().cloned());
        candidate.validate()?;
        Ok(())
    }
}

fn checkpoint_require(condition: bool, reason: &'static str) -> Result<(), RecordingError> {
    if condition {
        Ok(())
    } else {
        Err(RecordingError::Checkpoint(reason))
    }
}

fn within_loop(at: Beat, start: Beat, length: Beat) -> Result<Beat, TimeError> {
    let elapsed = at.checked_sub(start)?;
    elapsed.checked_sub(length.checked_scale(elapsed.units(length)?, 1)?)
}

fn resolve_occurrence(song: &Song, path: Occurrence) -> Result<PhraseId, DocumentError> {
    let track = song
        .tracks
        .iter()
        .find(|track| track.id == path.track)
        .ok_or(DocumentError::Missing {
            kind: "track",
            id: path.track.get(),
        })?;
    let usage = track
        .slots
        .iter()
        .flatten()
        .find(|usage| usage.id == path.chain_use)
        .ok_or(DocumentError::Missing {
            kind: "chain use",
            id: path.chain_use.get(),
        })?;
    let chain = song
        .chains
        .get(&usage.chain.pattern()?)
        .ok_or(DocumentError::Missing {
            kind: "chain",
            id: usage.chain.pattern()?.get(),
        })?;
    let phrase = chain
        .phrases
        .iter()
        .find(|usage| usage.id == path.phrase_use)
        .ok_or(DocumentError::Missing {
            kind: "phrase use",
            id: path.phrase_use.get(),
        })?
        .phrase;
    if !song.phrases.contains_key(&phrase) {
        return Err(DocumentError::Missing {
            kind: "phrase",
            id: phrase.get(),
        });
    }
    Ok(phrase)
}
