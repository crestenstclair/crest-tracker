//! Deterministic musical execution; no devices, MIDI encoding or sound internals.
mod resolve;
mod tempo_map;
mod voices;
use crate::composition::*;
use crate::time::{Beat, Tempo};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
pub use tempo_map::TempoMap;
use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum PlaybackError {
    #[error(transparent)]
    Document(#[from] DocumentError),
    #[error(transparent)]
    Time(#[from] crate::time::TimeError),
    #[error(transparent)]
    Harmony(#[from] crate::harmony::HarmonyError),
    #[error("playback admission budget exceeded: {0}")]
    Budget(&'static str),
    #[error("unsupported expression transformation: {0}")]
    Expression(&'static str),
    #[error("scheduler windows must advance monotonically")]
    Backwards,
}

/// Control-side admission budgets. They bound hostile documents and cyclic flow.
#[derive(Debug, Clone, Copy)]
pub struct PlaybackBudget {
    pub steps_per_window: usize,
    pub events_per_window: usize,
}
impl Default for PlaybackBudget {
    fn default() -> Self {
        Self {
            steps_per_window: 65_536,
            events_per_window: 262_144,
        }
    }
}
#[derive(Debug, Clone)]
pub struct Snapshot {
    song: Song,
    revision: u64,
    budget: PlaybackBudget,
}
impl Snapshot {
    pub fn compile(
        song: &Song,
        revision: u64,
        budget: PlaybackBudget,
    ) -> Result<Self, PlaybackError> {
        song.validate()?;
        if budget.steps_per_window == 0 || budget.events_per_window == 0 {
            return Err(PlaybackError::Budget("zero capacity"));
        }
        for phrase in song.phrases.values() {
            resolve::validate(phrase, &song.key, &song.chords, budget)?;
        }
        Ok(Self {
            song: song.clone(),
            revision,
            budget,
        })
    }
    pub fn song(&self) -> &Song {
        &self.song
    }
    pub const fn revision(&self) -> u64 {
        self.revision
    }
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Owner {
    pub track: TrackId,
    pub chain_use: UseId,
    pub phrase_use: UseId,
    pub cycle: u64,
    pub event: EventId,
    pub voice: u16,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Message {
    NoteOn {
        pitch: u8,
        velocity: u8,
    },
    NoteOff {
        pitch: u8,
        velocity: u8,
    },
    /// Force release one owned articulation, bypassing held pedals.
    KillNote {
        pitch: u8,
    },
    Control {
        control: Control,
    },
    /// A control scoped to this attack, never to the destination channel.
    Expression {
        control: Control,
    },
    /// The independently captured master dimension of one articulation.
    MasterExpression {
        control: Control,
    },
    /// Host identity remains addressable after physical key release, through
    /// the explicitly captured sustain/control lifetime.
    ExpressionLifetime {
        duration: Beat,
    },
    EndExpression,
    Send {
        bus: u32,
        amount: u8,
        slot: u8,
    },
    EndSend {
        slot: u8,
    },
    Tempo {
        tempo: Tempo,
    },
}
impl Message {
    fn priority(&self) -> u8 {
        match self {
            Self::NoteOff { .. } | Self::KillNote { .. } | Self::EndSend { .. } => 0,
            Self::Tempo { .. } => 1,
            Self::Control { .. } | Self::Send { .. } => 2,
            Self::NoteOn { .. } => 3,
            // Attack and initial expression execute at the same sample, before
            // rendering. The voice must exist before its owned controls apply.
            Self::ExpressionLifetime { .. } => 4,
            Self::Expression { .. } | Self::MasterExpression { .. } => 5,
            Self::EndExpression => 6,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimedEvent {
    pub at: Beat,
    pub target: Target,
    pub owner: Owner,
    pub message: Message,
}

#[derive(Debug, Clone)]
struct Cursor {
    active: Arc<Snapshot>,
    track_id: TrackId,
    track: usize,
    slot: usize,
    chain_repeat: u32,
    phrase: usize,
    phrase_repeat: u32,
    row: u32,
    next: Beat,
    padding_until: Beat,
    cycle: u64,
    stopped: bool,
    phrase_boundary: bool,
    require_phrase_boundary: bool,
}
#[derive(Debug, Clone, Serialize)]
pub struct Playhead {
    pub track: TrackId,
    pub chain_use: UseId,
    pub phrase_use: UseId,
    pub row: u32,
    pub at: Beat,
}

/// A delivered lane position at the queried transport beat. `local_beat` is
/// measured inside the source phrase, including the fraction after `row`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AudiblePlayhead {
    pub occurrence: crate::editing::Occurrence,
    pub phrase: PhraseId,
    pub row: u32,
    pub local_beat: Beat,
    /// Zero-based repetitions within the current arrangement placement.
    pub phrase_repeat: u32,
    pub chain_repeat: u32,
    pub at: Beat,
    pub cycle: u64,
    pub revision: u64,
}

#[derive(Debug, Clone)]
struct PlayheadSpan {
    start: AudiblePlayhead,
    end: Beat,
}

#[derive(Debug, Clone)]
struct PlayheadHistory {
    padding: Vec<PaddingSpan>,
    spans: Vec<PlayheadSpan>,
    latest: BTreeMap<TrackId, usize>,
    from: Beat,
    previous_budget: usize,
}

#[derive(Debug, Clone)]
struct PaddingSpan {
    selection: crate::composition::ChainSelection,
    start: Beat,
    end: Beat,
}

impl PlayheadHistory {
    fn new(budget: usize) -> Self {
        Self {
            spans: Vec::new(),
            padding: Vec::new(),
            latest: BTreeMap::new(),
            from: Beat::ZERO,
            previous_budget: budget,
        }
    }

    fn record(&mut self, span: PlayheadSpan) {
        let track = span.start.occurrence.track;
        if let Some(index) = self.latest.get(&track) {
            // A stopped lane can resume at the delivered horizon after an edit,
            // before its previously resolved final row's nominal end.
            self.spans[*index].end = self.spans[*index].end.min(span.start.at);
        }
        self.latest.insert(track, self.spans.len());
        self.spans.push(span);
    }

    fn retain_recent(&mut self, budget: usize) {
        let capacity = budget.saturating_add(self.previous_budget);
        self.previous_budget = budget;
        if self.padding.len() > capacity {
            self.discard_before(self.padding[self.padding.len() - capacity].start);
        }
        if self.spans.len() <= capacity {
            return;
        }
        let from = self.spans[self.spans.len() - capacity].start.at;
        self.discard_before(from);
    }

    fn discard_before(&mut self, from: Beat) {
        self.from = self.from.max(from);
        // Keep intervals crossing the cutoff, including slow lanes whose last
        // step began long before faster lanes consumed the history budget.
        self.spans.retain(|span| span.end > self.from);
        self.padding.retain(|span| span.end > self.from);
        self.latest = self
            .spans
            .iter()
            .enumerate()
            .map(|(index, span)| (span.start.occurrence.track, index))
            .collect();
    }

    fn at(&self, at: Beat) -> Result<Vec<AudiblePlayhead>, PlaybackError> {
        let mut heads = BTreeMap::new();
        for span in &self.spans {
            if span.start.at <= at && at < span.end {
                let mut head = span.start.clone();
                head.local_beat = head.local_beat.checked_add(at.checked_sub(head.at)?)?;
                head.at = at;
                heads.insert(head.occurrence.track, head);
            }
        }
        Ok(heads.into_values().collect())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TempoConflict {
    pub at: Beat,
    pub winner: Owner,
    pub ignored: Vec<Owner>,
}

#[derive(Debug, Clone)]
pub struct Playback {
    voices: voices::Voices,
    snapshot: Arc<Snapshot>,
    cursors: Vec<Cursor>,
    pending: Vec<TimedEvent>,
    horizon: Beat,
    tempo_conflicts: Vec<TempoConflict>,
    loop_occurrence: Option<crate::editing::Occurrence>,
    playhead_history: PlayheadHistory,
}
impl Playback {
    pub fn new(snapshot: Arc<Snapshot>) -> Self {
        let cursors = (0..snapshot.song.tracks.len())
            .map(|track| Cursor::new(Arc::clone(&snapshot), track, Beat::ZERO))
            .collect();
        Self {
            voices: voices::Voices::default(),
            playhead_history: PlayheadHistory::new(snapshot.budget.steps_per_window),
            snapshot,
            cursors,
            pending: Vec::new(),
            horizon: Beat::ZERO,
            tempo_conflicts: Vec::new(),
            loop_occurrence: None,
        }
    }
    /// Audition or record one fixed occurrence while unrelated tracks keep
    /// their normal arrangement. No document references or generator seeds change.
    pub fn new_looping(
        snapshot: Arc<Snapshot>,
        path: crate::editing::Occurrence,
    ) -> Result<Self, PlaybackError> {
        let mut playback = Self::new(snapshot);
        let song = &playback.snapshot.song;
        let track = song
            .tracks
            .iter()
            .position(|track| track.id == path.track)
            .ok_or(DocumentError::Invalid("loop track is missing"))?;
        let (slot, phrase) =
            find_occurrence_in_track(song, &song.tracks[track], path.chain_use, path.phrase_use)
                .ok_or(DocumentError::Invalid("loop occurrence is missing"))?;
        let cursor = &mut playback.cursors[track];
        cursor.slot = slot;
        cursor.phrase = phrase;
        cursor.stopped = false;
        playback.loop_occurrence = Some(path);
        Ok(playback)
    }
    pub const fn horizon(&self) -> Beat {
        self.horizon
    }
    pub fn revision(&self) -> u64 {
        self.snapshot.revision
    }
    /// Compilation precedes publication. Step-only edits adopt at the next
    /// unscheduled step; structural edits wait for each lane's phrase boundary.
    pub fn adopt(&mut self, snapshot: Arc<Snapshot>) {
        self.snapshot = snapshot;
    }
    /// Publishes a complete revision at each running lane's next unscheduled
    /// phrase boundary. Already resolved events retain their original ownership.
    /// Later ordinary edits cannot weaken a lane's pending boundary requirement;
    /// deletion, muting and invalidated occurrences still stop new steps promptly.
    pub fn adopt_at_phrase_boundary(&mut self, snapshot: Arc<Snapshot>) {
        for cursor in &mut self.cursors {
            if !cursor.stopped && cursor.active.revision != snapshot.revision {
                cursor.require_phrase_boundary = true;
            }
        }
        self.snapshot = snapshot;
    }
    pub fn pending_revision(&self) -> bool {
        self.cursors
            .iter()
            .any(|cursor| cursor.active.revision != self.snapshot.revision)
            || self.snapshot.song.tracks.iter().any(|track| {
                !self
                    .cursors
                    .iter()
                    .any(|cursor| cursor.track_id == track.id)
            })
    }
    /// Conflicts in the last successfully delivered window, after deterministic arbitration.
    pub fn tempo_conflicts(&self) -> &[TempoConflict] {
        &self.tempo_conflicts
    }
    pub fn finished(&self) -> bool {
        self.cursors.iter().all(|v| v.stopped) && self.pending.is_empty()
    }
    pub fn playheads(&self) -> Vec<Playhead> {
        self.cursors
            .iter()
            .filter_map(|cursor| {
                let (chain_use, usage, _) = cursor.current(&cursor.active.song)?;
                Some(Playhead {
                    track: cursor.active.song.tracks[cursor.track].id,
                    chain_use: chain_use.id,
                    phrase_use: usage.id,
                    row: cursor.row,
                    at: cursor.next,
                })
            })
            .collect()
    }
    /// Queries delivered execution, independently of the lookahead cursors.
    /// `None` means the beat is outside retained history or at/after the delivery
    /// horizon; `Some([])` means no lane was executing at that known beat.
    /// Results use track-ID order and include a final phrase until its actual end,
    /// even when `finished()` is already true. Step intervals are half-open.
    /// History retains the two most recent step budgets plus intervals spanning
    /// their cutoff; short or empty window calls cannot prematurely evict it.
    pub fn playheads_at(&self, at: Beat) -> Result<Option<Vec<AudiblePlayhead>>, PlaybackError> {
        if at < self.playhead_history.from || at >= self.horizon {
            return Ok(None);
        }
        self.playhead_history.at(at).map(Some)
    }
    /// Song-level cursors include timed built-ins without inventing phrases.
    pub fn song_playheads_at(
        &self,
        at: Beat,
    ) -> Result<Option<Vec<ChainSelection>>, PlaybackError> {
        let Some(heads) = self.playheads_at(at)? else {
            return Ok(None);
        };
        let mut selections: BTreeMap<_, _> = heads
            .into_iter()
            .map(|head| {
                (
                    head.occurrence.track,
                    ChainSelection {
                        track: head.occurrence.track,
                        chain_use: head.occurrence.chain_use,
                    },
                )
            })
            .collect();
        for span in &self.playhead_history.padding {
            if span.start <= at && at < span.end {
                selections.insert(span.selection.track, span.selection);
            }
        }
        Ok(Some(selections.into_values().collect()))
    }
    /// Releases presentation history the host no longer needs. Musical cursors,
    /// pending events and already delivered output are unchanged. Slow-lane
    /// intervals crossing the cutoff remain queryable; earlier queries return
    /// `None`. The cutoff is monotonic and cannot exceed the delivered horizon.
    pub fn discard_playheads_before(&mut self, at: Beat) {
        self.playhead_history.discard_before(at.min(self.horizon));
    }
    /// Deliver [previous horizon, end). A failed admission publishes no partial window.
    pub fn window(&mut self, end: Beat) -> Result<Vec<TimedEvent>, PlaybackError> {
        if end < self.horizon {
            return Err(PlaybackError::Backwards);
        }
        let mut candidate = self.clone();
        let events = candidate.advance(end)?;
        *self = candidate;
        Ok(events)
    }
    /// Advances silently to a seek position. Notes that started before the
    /// position are not resumed; later attacks keep their own expression and
    /// releases. The caller cancels already delivered output before seeking.
    pub fn seek_forward(&mut self, position: Beat) -> Result<(), PlaybackError> {
        self.seek_forward_tempos(position).map(|_| ())
    }
    /// Rebuild internal-clock tempo history while silently traversing flow.
    /// Returned changes have passed normal simultaneous-tempo arbitration.
    pub fn seek_forward_tempos(
        &mut self,
        position: Beat,
    ) -> Result<Vec<(Beat, Tempo)>, PlaybackError> {
        if position < self.horizon {
            return Err(PlaybackError::Backwards);
        }
        let mut candidate = self.clone();
        let tempos = candidate
            .advance(position)?
            .into_iter()
            .filter_map(|event| match event.message {
                Message::Tempo { tempo } => Some((event.at, tempo)),
                _ => None,
            })
            .collect();
        let attacks: std::collections::BTreeSet<_> = candidate
            .pending
            .iter()
            .filter(|event| matches!(event.message, Message::NoteOn { .. }))
            .map(|event| event.owner.clone())
            .collect();
        let sends: std::collections::BTreeSet<_> = candidate
            .pending
            .iter()
            .filter_map(|event| {
                if let Message::Send { slot, .. } = event.message {
                    Some((event.owner.clone(), slot))
                } else {
                    None
                }
            })
            .collect();
        candidate.pending.retain(|event| match event.message {
            Message::KillNote { .. }
            | Message::NoteOff { .. }
            | Message::Expression { .. }
            | Message::MasterExpression { .. }
            | Message::ExpressionLifetime { .. }
            | Message::EndExpression => attacks.contains(&event.owner),
            Message::EndSend { slot } => sends.contains(&(event.owner.clone(), slot)),
            _ => true,
        });
        candidate.voices = voices::Voices::default();
        *self = candidate;
        Ok(tempos)
    }
    fn advance(&mut self, end: Beat) -> Result<Vec<TimedEvent>, PlaybackError> {
        self.reconcile_stopped_lanes()?;
        let mut work = 0;
        let mut cuts = Vec::new();
        while let Some(index) = self.next_cursor(end) {
            if work >= self.snapshot.budget.steps_per_window {
                return Err(PlaybackError::Budget("step transitions"));
            }
            work += 1;
            let cursor = &mut self.cursors[index];
            cursor.adopt_if_eligible(&self.snapshot);
            let current = Arc::clone(&cursor.active);
            if let Some(usage) = cursor.padding(&current.song).cloned() {
                let at = cursor.next;
                let track = cursor.track_id;
                let end = current.song.after_measures(at, usage.repeats)?;
                self.playhead_history.padding.push(PaddingSpan {
                    selection: ChainSelection {
                        track,
                        chain_use: usage.id,
                    },
                    start: at,
                    end,
                });
                cursor.next = end;
                cursor.padding_until = end;
                cursor.advance_slot(&current.song)?;
                if usage.chain == ChainRef::Special(SpecialChain::Kill) {
                    cuts.sort_by_key(|(at, _, _): &(Beat, TrackId, Target)| *at);
                    let count = cuts.partition_point(|(cut, _, _)| *cut < at);
                    for (cut, lane, target) in cuts.drain(..count) {
                        self.cut_voices(cut, lane, &target);
                    }
                    self.kill_track(at, track)?;
                }
                continue;
            }
            let Some((chain_use, usage, phrase)) = cursor.current(&current.song) else {
                cursor.stopped = true;
                continue;
            };
            let owner = Owner {
                track: current.song.tracks[cursor.track].id,
                chain_use: chain_use.id,
                phrase_use: usage.id,
                cycle: cursor.cycle,
                event: EventId::new(1)?,
                voice: 0,
            };
            let looping = self.loop_occurrence.is_some_and(|path| {
                path.track == owner.track
                    && path.chain_use == owner.chain_use
                    && path.phrase_use == owner.phrase_use
            });
            let output = resolve::step(
                phrase,
                &current.song.key,
                &current.song.chords,
                cursor.row,
                cursor.next,
                owner,
                self.snapshot.budget,
            )?;
            if self.pending.len().saturating_add(output.events.len())
                > self.snapshot.budget.events_per_window
            {
                return Err(PlaybackError::Budget("pending events"));
            }
            let step_end = cursor.next.checked_add(phrase.division.beat_length())?;
            self.playhead_history.record(PlayheadSpan {
                start: AudiblePlayhead {
                    occurrence: crate::editing::Occurrence {
                        track: cursor.track_id,
                        chain_use: chain_use.id,
                        phrase_use: usage.id,
                    },
                    phrase: usage.phrase,
                    row: cursor.row,
                    phrase_repeat: cursor.phrase_repeat,
                    chain_repeat: cursor.chain_repeat,
                    local_beat: phrase
                        .division
                        .beat_length()
                        .checked_scale(u64::from(cursor.row), 1)?,
                    at: cursor.next,
                    cycle: cursor.cycle,
                    revision: current.revision,
                },
                end: step_end,
            });
            self.pending.extend(output.events);
            cuts.extend(
                output
                    .cuts
                    .into_iter()
                    .map(|(at, target)| (at, cursor.track_id, target)),
            );
            cursor.phrase_boundary = false;
            cursor.next = step_end;
            if let Some(jump) = output.jump {
                cursor.row = jump;
                cursor.cycle = cursor
                    .cycle
                    .checked_add(1)
                    .ok_or(DocumentError::Invalid("playback cycle range exhausted"))?;
            } else {
                cursor.row += 1;
            }
            if cursor.row >= phrase.steps {
                // Adopt before choosing the next occurrence, so edits to looping,
                // repeats or appended slots take effect at this exact boundary.
                cursor.phrase_boundary = true;
                cursor.adopt_if_eligible(&self.snapshot);
                let boundary_snapshot = Arc::clone(&cursor.active);
                if looping {
                    cursor.row = 0;
                    cursor.cycle = cursor
                        .cycle
                        .checked_add(1)
                        .ok_or(DocumentError::Invalid("playback cycle range exhausted"))?;
                } else {
                    cursor.advance_phrase(&boundary_snapshot.song)?;
                }
            }
        }
        cuts.sort_by_key(|(at, _, _)| *at);
        for (at, track, target) in cuts {
            self.cut_voices(at, track, &target);
        }
        self.pending.sort_by(|a, b| {
            (a.at, a.message.priority(), &a.owner).cmp(&(b.at, b.message.priority(), &b.owner))
        });
        let count = self.pending.partition_point(|event| event.at < end);
        let mut events: Vec<_> = self.pending.drain(..count).collect();
        for event in &events {
            self.voices.apply(event);
            if self.voices.len() > self.snapshot.budget.events_per_window {
                return Err(PlaybackError::Budget("active articulations"));
            }
        }
        self.tempo_conflicts = arbitrate_tempos(&mut events, &self.snapshot.song);
        self.playhead_history
            .retain_recent(self.snapshot.budget.steps_per_window);
        self.horizon = end;
        Ok(events)
    }
    fn kill_track(&mut self, at: Beat, track: TrackId) -> Result<(), PlaybackError> {
        // Replay only undelivered events before this boundary into the existing
        // ownership ledger. Later slots have not been expanded yet.
        let mut voices = self.voices.clone();
        let mut earlier: Vec<_> = self.pending.iter().filter(|e| e.at < at).collect();
        earlier.sort_by(|a, b| {
            (a.at, a.message.priority(), &a.owner).cmp(&(b.at, b.message.priority(), &b.owner))
        });
        for event in earlier {
            voices.apply(event);
        }
        let kills = voices.kills(track, at);
        let owners: std::collections::BTreeSet<_> = kills.iter().map(|e| e.owner.clone()).collect();
        let future_sends: std::collections::BTreeSet<_> = self
            .pending
            .iter()
            .filter_map(|e| {
                if e.owner.track == track && e.at >= at {
                    if let Message::Send { slot, .. } = e.message {
                        return Some((e.owner.clone(), slot));
                    }
                }
                None
            })
            .collect();
        self.pending.retain_mut(|e| {
            if e.owner.track != track {
                return true;
            }
            if e.at < at {
                if let Message::ExpressionLifetime { duration } = &mut e.message {
                    *duration = (*duration).min(at.checked_sub(e.at).expect("earlier attack"));
                }
                return true;
            }
            match e.message {
                Message::EndSend { slot } if !future_sends.contains(&(e.owner.clone(), slot)) => {
                    e.at = at
                }
                Message::EndExpression if owners.contains(&e.owner) => e.at = at,
                _ => return false,
            }
            true
        });
        if self.pending.len().saturating_add(kills.len()) > self.snapshot.budget.events_per_window {
            return Err(PlaybackError::Budget("track releases"));
        }
        self.pending.extend(kills);
        Ok(())
    }
    fn cut_voices(&mut self, at: Beat, track: TrackId, target: &Target) {
        // Future attacks are not sounding yet. All other pending releases carry
        // the exact original owner, even across phrase boundaries and windows.
        let future: std::collections::BTreeSet<_> = self
            .pending
            .iter()
            .filter(|event| event.at >= at && matches!(event.message, Message::NoteOn { .. }))
            .map(|event| event.owner.clone())
            .collect();
        let owners: std::collections::BTreeSet<_> = self
            .pending
            .iter()
            .filter(|event| {
                event.owner.track == track
                    && &event.target == target
                    && event.at >= at
                    && matches!(
                        event.message,
                        Message::NoteOff { .. } | Message::EndExpression
                    )
                    && !future.contains(&event.owner)
            })
            .map(|event| event.owner.clone())
            .collect();
        self.pending.retain_mut(|event| {
            if !owners.contains(&event.owner) {
                return true;
            }
            if event.at >= at {
                match event.message {
                    Message::NoteOff { .. }
                    | Message::EndExpression
                    | Message::Expression {
                        control:
                            Control::Cc {
                                number: 64,
                                value: 0,
                            },
                    }
                    | Message::MasterExpression {
                        control:
                            Control::Cc {
                                number: 64,
                                value: 0,
                            },
                    } => event.at = at,
                    Message::Expression { .. } | Message::MasterExpression { .. } => return false,
                    _ => {}
                }
            } else if let Message::ExpressionLifetime { duration } = &mut event.message {
                *duration = (*duration).min(at.checked_sub(event.at).expect("earlier attack"));
            }
            true
        });
    }
    fn reconcile_stopped_lanes(&mut self) -> Result<(), PlaybackError> {
        self.cursors.retain(|cursor| {
            !cursor.stopped
                || self
                    .snapshot
                    .song
                    .tracks
                    .iter()
                    .any(|track| track.id == cursor.track_id)
        });
        for cursor in self.cursors.iter_mut().filter(|cursor| cursor.stopped) {
            cursor.refresh_stopped(&self.snapshot, self.horizon)?;
        }
        for track in 0..self.snapshot.song.tracks.len() {
            let id = self.snapshot.song.tracks[track].id;
            if !self.cursors.iter().any(|cursor| cursor.track_id == id) {
                self.cursors
                    .push(Cursor::new(Arc::clone(&self.snapshot), track, self.horizon));
            }
        }
        Ok(())
    }
    fn next_cursor(&self, end: Beat) -> Option<usize> {
        self.cursors
            .iter()
            .enumerate()
            .filter(|(_, v)| !v.stopped && v.next < end)
            .min_by_key(|(index, v)| (v.next, *index))
            .map(|(index, _)| index)
    }
}
impl Cursor {
    fn new(active: Arc<Snapshot>, track: usize, next: Beat) -> Self {
        let track_id = active.song.tracks[track].id;
        let muted = active.song.tracks[track].muted;
        let mut cursor = Self {
            active,
            track_id,
            track,
            slot: 0,
            chain_repeat: 0,
            phrase: 0,
            phrase_repeat: 0,
            row: 0,
            next,
            padding_until: Beat::ZERO,
            cycle: 0,
            stopped: muted,
            phrase_boundary: true,
            require_phrase_boundary: false,
        };
        cursor.stopped = muted || !cursor.has_current(&cursor.active.song);
        cursor
    }

    /// An empty slot stops a lane. New material resumes that slot at the delivery horizon;
    /// completed earlier slots are not replayed and gaps are never skipped.
    fn refresh_stopped(
        &mut self,
        next: &Arc<Snapshot>,
        horizon: Beat,
    ) -> Result<(), PlaybackError> {
        if self.active.revision == next.revision {
            return Ok(());
        }
        let Some(track) = next
            .song
            .tracks
            .iter()
            .position(|track| track.id == self.track_id)
        else {
            return Ok(());
        };
        let old_track = self
            .active
            .song
            .tracks
            .iter()
            .find(|track| track.id == self.track_id);
        let old_slot = old_track.and_then(|track| track.slots.get(self.slot));
        let next_track = &next.song.tracks[track];
        let mut slot = self.slot;
        if next_track.looped && slot >= next_track.slots.len() {
            slot = 0;
        }
        let changed = old_slot != next_track.slots.get(slot)
            || old_track.is_some_and(|old| {
                old.muted != next_track.muted || old.looped != next_track.looped
            })
            || stopped_chain_changed(
                &self.active.song,
                &next.song,
                next_track.slots.get(slot).and_then(Option::as_ref),
            );
        self.active = Arc::clone(next);
        self.require_phrase_boundary = false;
        self.track = track;
        if !changed || next_track.muted {
            return Ok(());
        }
        self.slot = slot;
        self.phrase = 0;
        self.row = 0;
        self.chain_repeat = 0;
        self.phrase_repeat = 0;
        self.phrase_boundary = true;
        self.next = horizon.max(self.padding_until);
        self.stopped = false;
        self.stopped = !self.has_current(&self.active.song);
        if !self.stopped {
            self.cycle = self
                .cycle
                .checked_add(1)
                .ok_or(DocumentError::Invalid("playback cycle range exhausted"))?;
        }
        Ok(())
    }

    fn adopt_if_eligible(&mut self, next: &Arc<Snapshot>) {
        if self.active.revision == next.revision {
            if self.phrase_boundary {
                self.require_phrase_boundary = false;
            }
            return;
        }
        if let Some(old) = self.padding(&self.active.song) {
            let found = next
                .song
                .tracks
                .iter()
                .enumerate()
                .find_map(|(track, lane)| {
                    if lane.id != self.track_id || lane.muted {
                        return None;
                    }
                    lane.slots
                        .iter()
                        .position(|slot| {
                            slot.as_ref()
                                .is_some_and(|usage| usage.id == old.id && usage.chain == old.chain)
                        })
                        .map(|slot| (track, slot))
                });
            if let Some((track, slot)) = found {
                self.track = track;
                self.slot = slot;
                self.active = Arc::clone(next);
            } else {
                self.stop_at(next, None);
            }
            return;
        }
        let Some((old_chain, old_use, old_phrase)) = self.current(&self.active.song) else {
            self.stop_at(next, None);
            return;
        };
        let Some(track) = next
            .song
            .tracks
            .iter()
            .position(|track| track.id == self.track_id)
        else {
            self.stop_at(next, None);
            return;
        };
        if next.song.tracks[track].muted {
            self.stop_at(next, Some(track));
            return;
        }
        let Some((slot, phrase_index)) = find_occurrence_in_track(
            &next.song,
            &next.song.tracks[track],
            old_chain.id,
            old_use.id,
        ) else {
            self.stop_at(next, Some(track));
            return;
        };
        let next_chain = next.song.tracks[track].slots[slot]
            .as_ref()
            .expect("found use");
        let chain = &next.song.chains[&next_chain.chain.pattern().expect("found pattern")];
        let phrase = &next.song.phrases[&chain.phrases[phrase_index].phrase];
        let step_edit = old_phrase.steps == phrase.steps
            && old_phrase.division == phrase.division
            && old_phrase.form == phrase.form
            && old_phrase.seed == phrase.seed
            && old_chain == next_chain
            && old_use == &chain.phrases[phrase_index]
            && self.active.song.tracks[self.track].slots == next.song.tracks[track].slots
            && self.active.song.tracks[self.track].looped == next.song.tracks[track].looped
            && (!matches!(phrase.form, PhraseForm::Random { .. })
                || old_phrase.key.as_ref().unwrap_or(&self.active.song.key)
                    == phrase.key.as_ref().unwrap_or(&next.song.key));
        if !self.phrase_boundary && (self.require_phrase_boundary || !step_edit) {
            return;
        }
        self.track = track;
        self.slot = slot;
        self.phrase = phrase_index;
        self.active = Arc::clone(next);
        self.require_phrase_boundary = false;
    }
    fn stop_at(&mut self, next: &Arc<Snapshot>, track: Option<usize>) {
        self.stopped = true;
        self.active = Arc::clone(next);
        self.require_phrase_boundary = false;
        if let Some(track) = track {
            self.track = track;
        }
    }
    fn padding<'a>(&self, song: &'a Song) -> Option<&'a ChainUse> {
        if self.stopped {
            return None;
        }
        song.tracks
            .get(self.track)?
            .slots
            .get(self.slot)?
            .as_ref()
            .filter(|usage| usage.chain.special().is_some())
    }
    fn has_current(&self, song: &Song) -> bool {
        self.current(song).is_some() || self.padding(song).is_some()
    }
    fn current<'a>(&self, song: &'a Song) -> Option<(&'a ChainUse, &'a PhraseUse, &'a Phrase)> {
        if self.stopped {
            return None;
        }
        let chain_use = song
            .tracks
            .get(self.track)?
            .slots
            .get(self.slot)?
            .as_ref()?;
        let usage = song
            .chains
            .get(&chain_use.chain.pattern().ok()?)?
            .phrases
            .get(self.phrase)?;
        Some((chain_use, usage, song.phrases.get(&usage.phrase)?))
    }
    fn advance_phrase(&mut self, song: &Song) -> Result<(), PlaybackError> {
        let Some((chain_use, usage, _)) = self.current(song) else {
            self.stopped = true;
            return Ok(());
        };
        self.row = 0;
        self.phrase_boundary = true;
        self.cycle = self
            .cycle
            .checked_add(1)
            .ok_or(DocumentError::Invalid("playback cycle range exhausted"))?;
        self.phrase_repeat += 1;
        if self.phrase_repeat < usage.repeats {
            return Ok(());
        }
        self.phrase_repeat = 0;
        self.phrase += 1;
        if self.phrase < song.chains[&chain_use.chain.pattern()?].phrases.len() {
            return Ok(());
        }
        self.phrase = 0;
        self.chain_repeat += 1;
        if self.chain_repeat < chain_use.repeats {
            return Ok(());
        }
        self.advance_slot(song)
    }
    fn advance_slot(&mut self, song: &Song) -> Result<(), PlaybackError> {
        self.chain_repeat = 0;
        self.phrase_repeat = 0;
        self.phrase = 0;
        self.row = 0;
        self.phrase_boundary = true;
        self.slot += 1;
        let track = &song.tracks[self.track];
        if self.slot >= track.slots.len() && track.looped {
            self.slot = 0;
        }
        if !self.has_current(song) {
            self.stopped = true;
        }
        Ok(())
    }
}

fn find_occurrence_in_track(
    song: &Song,
    track: &Track,
    chain_use: UseId,
    phrase_use: UseId,
) -> Option<(usize, usize)> {
    let slot = track
        .slots
        .iter()
        .position(|slot| slot.as_ref().is_some_and(|usage| usage.id == chain_use))?;
    let chain = song
        .chains
        .get(&track.slots[slot].as_ref()?.chain.pattern().ok()?)?;
    let phrase = chain
        .phrases
        .iter()
        .position(|usage| usage.id == phrase_use)?;
    Some((slot, phrase))
}

fn stopped_chain_changed(old: &Song, next: &Song, usage: Option<&ChainUse>) -> bool {
    let Some(usage) = usage else {
        return false;
    };
    let Ok(id) = usage.chain.pattern() else {
        return false;
    };
    let Some(next_chain) = next.chains.get(&id) else {
        return false;
    };
    let Some(old_chain) = old.chains.get(&id) else {
        return true;
    };
    next_chain.phrases != old_chain.phrases
}

/// Lowest Song track order wins. Within that lane the last emitted tempo wins;
/// stable sorting preserves FX slot order for one event, then event-ID order.
fn arbitrate_tempos(events: &mut Vec<TimedEvent>, song: &Song) -> Vec<TempoConflict> {
    let mut winners: BTreeMap<Beat, (usize, usize, Vec<Owner>)> = BTreeMap::new();
    let mut keep = vec![true; events.len()];
    for (index, event) in events
        .iter()
        .enumerate()
        .filter(|(_, event)| matches!(event.message, Message::Tempo { .. }))
    {
        let rank = song
            .tracks
            .iter()
            .position(|track| track.id == event.owner.track)
            .unwrap_or(usize::MAX);
        if let Some((winner, winner_rank, ignored)) = winners.get_mut(&event.at) {
            if rank <= *winner_rank {
                keep[*winner] = false;
                ignored.push(events[*winner].owner.clone());
                *winner = index;
                *winner_rank = rank;
            } else {
                keep[index] = false;
                ignored.push(event.owner.clone());
            }
        } else {
            winners.insert(event.at, (index, rank, Vec::new()));
        }
    }
    let conflicts = winners
        .into_iter()
        .filter_map(|(at, (index, _, ignored))| {
            (!ignored.is_empty()).then(|| TempoConflict {
                at,
                winner: events[index].owner.clone(),
                ignored,
            })
        })
        .collect();
    let mut index = 0;
    events.retain(|_| {
        let retain = keep[index];
        index += 1;
        retain
    });
    conflicts
}
