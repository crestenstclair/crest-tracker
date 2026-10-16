//! Atomic commands, stable occurrence paths and document-wide undo.
pub mod shared;
use crate::composition::*;
use crate::time::{Beat, Division, Tempo};
use serde::{Deserialize, Serialize};
use std::num::NonZeroU64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Occurrence {
    pub track: TrackId,
    pub chain_use: UseId,
    pub phrase_use: UseId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    RenameSong {
        name: String,
    },
    SetTempo {
        tempo: Tempo,
    },
    SetMeters {
        meters: Vec<MeterChange>,
    },
    PutViewGroup {
        name: String,
        selections: Vec<ChainSelection>,
    },
    DeleteViewGroup {
        name: String,
    },
    SetKey {
        key: Key,
    },
    PutChord {
        id: crate::chord_bank::ChordRevisionId,
        revision: crate::chord_bank::ChordRevision,
    },
    PutInstrument {
        id: InstrumentId,
        instrument: InstrumentUse,
    },
    PutPhrase {
        id: PhraseId,
        phrase: Phrase,
    },
    PutChain {
        id: ChainId,
        chain: Chain,
    },
    PutTrack {
        track: Track,
    },
    PutEvent {
        phrase: PhraseId,
        event: Event,
    },
    DeleteEvents {
        phrase: PhraseId,
        events: Vec<EventId>,
    },
    AddPhrase {
        track: TrackId,
        slot: usize,
    },
    MakeUnique {
        occurrence: Occurrence,
    },
    /// Apply an edit to a private copy of one use, as one undo transaction.
    EditUnique {
        location: shared::Location,
        command: Box<Command>,
    },
    Quantize {
        phrase: PhraseId,
        events: Vec<EventId>,
        grid: Beat,
        durations: bool,
    },
    ResizeNotes {
        phrase: PhraseId,
        events: Vec<EventId>,
        duration: Beat,
    },
    ResizePhrase {
        phrase: PhraseId,
        steps: u32,
        trim: bool,
    },
    RetimeDivision {
        phrase: PhraseId,
        division: Division,
    },
    Batch {
        commands: Vec<Command>,
    },
}

#[derive(Debug, Clone)]
pub struct Editor {
    song: Song,
    revision: u64,
    undo: Vec<Song>,
    redo: Vec<Song>,
    saved: Song,
    active_gesture: Option<NonZeroU64>,
    recording_guard: Option<crate::recording::RecordingEditGuard>,
}
impl Editor {
    pub fn new(song: Song) -> Result<Self, DocumentError> {
        song.validate()?;
        Ok(Self {
            saved: song.clone(),
            song,
            revision: 0,
            undo: Vec::new(),
            redo: Vec::new(),
            active_gesture: None,
            recording_guard: None,
        })
    }
    pub fn song(&self) -> &Song {
        &self.song
    }
    /// The host refreshes this from its recorder before admitting requests, and
    /// removes it only when the take is committed, discarded or replaced.
    pub fn set_recording_guard(&mut self, guard: Option<crate::recording::RecordingEditGuard>) {
        self.recording_guard = guard;
    }
    /// Whether a retained take requires stable phrase identities and arrangement.
    /// This includes armed, interrupted and stopped-but-uncommitted takes.
    pub fn has_recording_guard(&self) -> bool {
        self.recording_guard.is_some()
    }
    fn validate_recording_edit(&self, candidate: &Song) -> Result<(), DocumentError> {
        if let Some(guard) = &self.recording_guard {
            guard.validate(candidate)?;
        }
        Ok(())
    }
    pub const fn revision(&self) -> u64 {
        self.revision
    }
    /// Replace the open document with a clean baseline and empty history.
    /// Revision identity belongs to this editor session, not the saved Song:
    /// delayed commands from any earlier document must remain stale. Validation
    /// or revision exhaustion leaves the current document and history intact.
    pub fn replace_song(&mut self, song: Song) -> Result<(), DocumentError> {
        song.validate()?;
        self.validate_recording_edit(&song)?;
        let revision = self.next_revision()?;
        self.saved = song.clone();
        self.song = song;
        self.revision = revision;
        self.undo.clear();
        self.redo.clear();
        self.end_gesture();
        Ok(())
    }
    pub fn dirty(&self) -> bool {
        self.song != self.saved
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    /// A completed save records its captured content, never newer unsaved edits,
    /// and separates subsequent edits from any gesture still held by the caller.
    pub fn mark_saved(&mut self, captured: Song) {
        self.end_gesture();
        self.saved = captured;
    }
    pub fn apply(
        &mut self,
        expected_revision: u64,
        command: Command,
    ) -> Result<bool, DocumentError> {
        self.apply_with_gesture(expected_revision, command, None)
    }
    /// Coalesce contiguous successful edits with the same nonzero caller-owned
    /// gesture ID into one undo step. Each change still advances the revision.
    /// Errors preserve all editor state; no-ops create no history. An unrelated
    /// successful or no-op apply, undo, redo, save, or explicit end breaks the
    /// group. Reusing an ID after a boundary always starts a fresh undo baseline.
    pub fn apply_gesture(
        &mut self,
        expected_revision: u64,
        command: Command,
        gesture: u64,
    ) -> Result<bool, DocumentError> {
        let gesture = NonZeroU64::new(gesture).ok_or(DocumentError::Invalid(
            "edit gesture identity must be nonzero",
        ))?;
        self.apply_with_gesture(expected_revision, command, Some(gesture))
    }
    /// End a held edit without changing content, revision, or history.
    pub fn end_gesture(&mut self) {
        self.active_gesture = None;
    }
    fn apply_with_gesture(
        &mut self,
        expected_revision: u64,
        command: Command,
        gesture: Option<NonZeroU64>,
    ) -> Result<bool, DocumentError> {
        if expected_revision != self.revision {
            return Err(DocumentError::Stale {
                expected: expected_revision,
                actual: self.revision,
            });
        }
        let mut candidate = self.song.clone();
        apply_command(&mut candidate, command)?;
        candidate.validate()?;
        self.validate_recording_edit(&candidate)?;
        if candidate == self.song {
            if gesture != self.active_gesture {
                self.end_gesture();
            }
            return Ok(false);
        }
        let next = self.next_revision()?;
        let coalesced = gesture.is_some() && gesture == self.active_gesture;
        let previous = std::mem::replace(&mut self.song, candidate);
        if !coalesced {
            self.undo.push(previous);
        }
        self.redo.clear();
        self.revision = next;
        self.active_gesture = gesture;
        Ok(true)
    }
    pub fn undo(&mut self) -> Result<bool, DocumentError> {
        if self.undo.is_empty() {
            self.end_gesture();
            return Ok(false);
        }
        self.validate_recording_edit(self.undo.last().expect("history checked"))?;
        let next = self.next_revision()?;
        self.end_gesture();
        let previous = self.undo.pop().expect("history checked");
        self.redo.push(std::mem::replace(&mut self.song, previous));
        self.revision = next;
        Ok(true)
    }
    pub fn redo(&mut self) -> Result<bool, DocumentError> {
        if self.redo.is_empty() {
            self.end_gesture();
            return Ok(false);
        }
        self.validate_recording_edit(self.redo.last().expect("history checked"))?;
        let next = self.next_revision()?;
        self.end_gesture();
        let following = self.redo.pop().expect("history checked");
        self.undo.push(std::mem::replace(&mut self.song, following));
        self.revision = next;
        Ok(true)
    }
    fn next_revision(&self) -> Result<u64, DocumentError> {
        self.revision
            .checked_add(1)
            .ok_or(DocumentError::Invalid("revision range exhausted"))
    }
}

fn apply_command(song: &mut Song, command: Command) -> Result<(), DocumentError> {
    match command {
        Command::RenameSong { name } => song.name = name,
        Command::SetTempo { tempo } => song.tempo = tempo,
        Command::SetMeters { meters } => song.meters = meters,
        Command::PutViewGroup { name, selections } => {
            song.view_groups.insert(name, selections);
        }
        Command::DeleteViewGroup { name } => {
            validate_name(&name)?;
            song.view_groups.remove(&name);
        }
        Command::SetKey { key } => song.key = key,
        Command::PutChord { id, revision } => {
            song.chords = song.chords.with_revision(id, revision)?;
        }
        Command::PutInstrument { id, instrument } => {
            song.instruments.insert(id, instrument);
        }
        Command::PutPhrase { id, phrase } => {
            song.phrases.insert(id, phrase);
        }
        Command::PutChain { id, chain } => {
            song.chains.insert(id, chain);
        }
        Command::PutTrack { track } => {
            let current = song.tracks.iter_mut().find(|v| v.id == track.id).ok_or(
                DocumentError::Missing {
                    kind: "track",
                    id: track.id.get(),
                },
            )?;
            *current = track;
        }
        Command::PutEvent { phrase, event } => {
            let phrase = phrase_mut(song, phrase)?;
            if let Some(current) = phrase.events.iter_mut().find(|v| v.id == event.id) {
                *current = event;
            } else {
                phrase.events.push(event);
            }
            phrase.events.sort_by_key(|v| (v.at, v.id));
        }
        Command::DeleteEvents { phrase, events } => {
            phrase_mut(song, phrase)?
                .events
                .retain(|v| !events.contains(&v.id));
        }
        Command::AddPhrase { track, slot } => add_phrase(song, track, slot)?,
        Command::MakeUnique { occurrence } => make_unique(song, occurrence)?,
        Command::EditUnique { location, command } => shared::edit_unique(song, location, *command)?,
        Command::Quantize {
            phrase,
            events,
            grid,
            durations,
        } => quantize(phrase_mut(song, phrase)?, &events, grid, durations)?,
        Command::ResizeNotes {
            phrase,
            events,
            duration,
        } => resize_notes(phrase_mut(song, phrase)?, &events, duration)?,
        Command::ResizePhrase {
            phrase,
            steps,
            trim,
        } => resize(phrase_mut(song, phrase)?, steps, trim)?,
        Command::RetimeDivision { phrase, division } => {
            retime(phrase_mut(song, phrase)?, division)?
        }
        Command::Batch { commands } => {
            for command in commands {
                apply_command(song, command)?;
            }
        }
    }
    Ok(())
}
fn phrase_mut(song: &mut Song, id: PhraseId) -> Result<&mut Phrase, DocumentError> {
    song.phrases.get_mut(&id).ok_or(DocumentError::Missing {
        kind: "phrase",
        id: id.get(),
    })
}
fn allocate(next: &mut u64) -> Result<u64, DocumentError> {
    let id = *next;
    *next = next
        .checked_add(1)
        .ok_or(DocumentError::Invalid("identity range exhausted"))?;
    Ok(id)
}
fn add_phrase(song: &mut Song, track_id: TrackId, slot: usize) -> Result<(), DocumentError> {
    let mut next = song.next_identity()?;
    let phrase = PhraseId::new(allocate(&mut next)?)?;
    let chain = ChainId::new(allocate(&mut next)?)?;
    let phrase_use = UseId::new(allocate(&mut next)?)?;
    let chain_use = UseId::new(allocate(&mut next)?)?;
    let track =
        song.tracks
            .iter_mut()
            .find(|v| v.id == track_id)
            .ok_or(DocumentError::Missing {
                kind: "track",
                id: track_id.get(),
            })?;
    if slot > track.slots.len() {
        return Err(DocumentError::Invalid(
            "new phrase slot must already exist or append to track",
        ));
    }
    let usage = Some(ChainUse {
        id: chain_use,
        chain,
        repeats: 1,
    });
    if slot == track.slots.len() {
        track.slots.push(usage);
    } else if track.slots[slot].is_none() {
        track.slots[slot] = usage;
    } else {
        return Err(DocumentError::Invalid("slot already contains a chain"));
    }
    song.phrases.insert(
        phrase,
        Phrase {
            name: format!("Phrase {:02X}", song.phrases.len()),
            ..Phrase::default()
        },
    );
    song.chains.insert(
        chain,
        Chain {
            name: format!("Chain {:02X}", song.chains.len()),
            phrases: vec![PhraseUse {
                id: phrase_use,
                phrase,
                repeats: 1,
            }],
        },
    );
    Ok(())
}

/// Clone the path as needed: a shared parent chain must not redirect other uses.
fn make_unique(song: &mut Song, path: Occurrence) -> Result<(), DocumentError> {
    let mut next = song.next_identity()?;
    let track = song
        .tracks
        .iter()
        .find(|v| v.id == path.track)
        .ok_or(DocumentError::Missing {
            kind: "track",
            id: path.track.get(),
        })?;
    let slot_index = track
        .slots
        .iter()
        .position(|v| v.as_ref().is_some_and(|v| v.id == path.chain_use))
        .ok_or(DocumentError::Missing {
            kind: "chain use",
            id: path.chain_use.get(),
        })?;
    let old_chain = track.slots[slot_index].as_ref().expect("found slot").chain;
    let mut chain = song
        .chains
        .get(&old_chain)
        .ok_or(DocumentError::Missing {
            kind: "chain",
            id: old_chain.get(),
        })?
        .clone();
    let phrase_index = chain
        .phrases
        .iter()
        .position(|v| v.id == path.phrase_use)
        .ok_or(DocumentError::Missing {
            kind: "phrase use",
            id: path.phrase_use.get(),
        })?;
    let source = chain.phrases[phrase_index].phrase;
    let mut phrase = song
        .phrases
        .get(&source)
        .ok_or(DocumentError::Missing {
            kind: "phrase",
            id: source.get(),
        })?
        .clone();
    let new_phrase = PhraseId::new(allocate(&mut next)?)?;
    phrase.name.push_str(" copy");
    for event in &mut phrase.events {
        event.id = EventId::new(allocate(&mut next)?)?;
    }
    let parent_uses = song
        .tracks
        .iter()
        .flat_map(|v| v.slots.iter().flatten())
        .filter(|v| v.chain == old_chain)
        .count();
    let selected_chain = if parent_uses > 1 {
        for usage in &mut chain.phrases {
            usage.id = UseId::new(allocate(&mut next)?)?;
        }
        ChainId::new(allocate(&mut next)?)?
    } else {
        old_chain
    };
    chain.phrases[phrase_index].phrase = new_phrase;
    song.phrases.insert(new_phrase, phrase);
    song.chains.insert(selected_chain, chain);
    song.tracks
        .iter_mut()
        .find(|v| v.id == path.track)
        .expect("validated track")
        .slots[slot_index]
        .as_mut()
        .expect("validated slot")
        .chain = selected_chain;
    Ok(())
}
fn quantize(
    phrase: &mut Phrase,
    ids: &[EventId],
    grid: Beat,
    durations: bool,
) -> Result<(), DocumentError> {
    let length = phrase.length()?;
    for event in phrase.events.iter_mut().filter(|v| ids.contains(&v.id)) {
        let at = event.at.quantized(grid)?;
        if at >= length {
            return Err(DocumentError::Invalid(
                "quantized note would leave phrase; extend phrase first",
            ));
        }
        event.at = at;
        if durations {
            if let Some(note) = &mut event.note {
                note.quantize_duration(grid)?;
            }
        }
    }
    Ok(())
}
fn resize_notes(phrase: &mut Phrase, ids: &[EventId], duration: Beat) -> Result<(), DocumentError> {
    for event in phrase
        .events
        .iter_mut()
        .filter(|event| ids.contains(&event.id))
    {
        if let Some(note) = &mut event.note {
            note.resize(duration)?;
        }
    }
    Ok(())
}

fn resize(phrase: &mut Phrase, steps: u32, trim: bool) -> Result<(), DocumentError> {
    if steps == 0 {
        return Err(DocumentError::Invalid("phrase length must be positive"));
    }
    let new_length = phrase
        .division
        .beat_length()
        .checked_scale(u64::from(steps), 1)?;
    if !trim
        && phrase.events.iter().any(|v| {
            v.at >= new_length
                || v.note.as_ref().is_some_and(|n| {
                    v.at.checked_add(n.expression_duration())
                        .map_or(true, |end| end > new_length)
                })
        })
    {
        return Err(DocumentError::Invalid(
            "length change requires explicit trim approval",
        ));
    }
    if trim {
        phrase.events.retain(|event| event.at < new_length);
        for event in &mut phrase.events {
            if let Some(note) = &mut event.note {
                note.trim_lifetime(new_length.checked_sub(event.at)?)?;
            }
        }
    }
    phrase.steps = steps;
    Ok(())
}
fn retime(phrase: &mut Phrase, division: Division) -> Result<(), DocumentError> {
    let old = phrase.division.beat_length();
    let new = division.beat_length();
    let factor = new.checked_scale(old.denominator(), old.numerator())?;
    for event in &mut phrase.events {
        event.at = event
            .at
            .checked_scale(factor.numerator(), factor.denominator())?;
        if let Some(note) = &mut event.note {
            note.retime(factor.numerator(), factor.denominator())?;
        }
    }
    phrase.division = division;
    Ok(())
}

#[cfg(test)]
mod meter_and_view_group_tests {
    use super::*;
    use crate::time::Meter;

    fn selection(track: u64, chain_use: u64) -> ChainSelection {
        ChainSelection {
            track: TrackId::new(track).unwrap(),
            chain_use: UseId::new(chain_use).unwrap(),
        }
    }
    fn meter(at: Beat, numerator: u16, denominator: u16) -> MeterChange {
        MeterChange {
            at,
            meter: Meter::new(numerator, denominator).unwrap(),
        }
    }
    fn apply(editor: &mut Editor, command: Command) {
        assert!(editor.apply(editor.revision(), command).unwrap());
    }

    #[test]
    fn meter_edits_share_song_boundary_validation_and_restore_exact_history() {
        let mut editor = Editor::new(Song::default()).unwrap();
        let original = editor.song().clone();
        let meters = vec![
            meter(Beat::ZERO, 4, 4),
            meter(Beat::integer(8), 3, 4),
            meter(Beat::integer(11), 7, 8),
            meter(Beat::new(29, 2).unwrap(), 5, 8),
        ];
        let command = Command::SetMeters {
            meters: meters.clone(),
        };
        let encoded = serde_json::to_value(&command).unwrap();
        assert_eq!(encoded["kind"], "set_meters");
        apply(&mut editor, serde_json::from_value(encoded).unwrap());
        assert_eq!(editor.song().meters, meters);
        let mut expected = original.clone();
        expected.meters = meters.clone();
        assert_eq!(editor.song(), &expected);
        assert!(editor.dirty());
        let revision = editor.revision();
        assert!(!editor
            .apply(revision, Command::SetMeters { meters })
            .unwrap());
        assert_eq!(editor.revision(), revision);
        editor.undo().unwrap();
        assert_eq!(editor.song(), &original);
        assert!(!editor.dirty());
        editor.redo().unwrap();
        assert_eq!(editor.song(), &expected);
    }

    #[test]
    fn invalid_meter_maps_refuse_atomically_without_discarding_redo_or_batch_state() {
        let mut editor = Editor::new(Song::default()).unwrap();
        apply(
            &mut editor,
            Command::RenameSong {
                name: "Recoverable edit".into(),
            },
        );
        editor.undo().unwrap();
        let original = editor.song().clone();
        let revision = editor.revision();
        for meters in [
            vec![],
            vec![meter(Beat::integer(1), 4, 4)],
            vec![meter(Beat::ZERO, 4, 4), meter(Beat::ZERO, 3, 4)],
            vec![
                meter(Beat::ZERO, 4, 4),
                meter(Beat::integer(8), 4, 4),
                meter(Beat::integer(4), 3, 4),
            ],
            vec![meter(Beat::ZERO, 4, 4), meter(Beat::integer(3), 3, 4)],
            vec![
                meter(Beat::ZERO, 4, 4),
                meter(Beat::integer(8), 3, 4),
                meter(Beat::integer(12), 7, 8),
            ],
        ] {
            let error = editor
                .apply(
                    revision,
                    Command::Batch {
                        commands: vec![
                            Command::RenameSong {
                                name: "Must roll back".into(),
                            },
                            Command::SetMeters { meters },
                        ],
                    },
                )
                .unwrap_err();
            assert!(error.to_string().contains("meter"));
            assert_eq!(editor.song(), &original);
            assert_eq!(editor.revision(), revision);
            assert!(!editor.can_undo());
            assert!(editor.can_redo());
            assert!(!editor.dirty());
        }
        editor.redo().unwrap();
        assert_eq!(editor.song().name, "Recoverable edit");
    }

    #[test]
    fn put_replace_delete_view_groups_are_undoable_and_missing_delete_is_noop() {
        let mut editor = Editor::new(Song::default()).unwrap();
        let first = vec![selection(1, 20), selection(2, 21)];
        let second = vec![selection(2, 21)];
        let put = Command::PutViewGroup {
            name: "Verse".into(),
            selections: first.clone(),
        };
        let encoded = serde_json::to_value(&put).unwrap();
        assert_eq!(encoded["kind"], "put_view_group");
        apply(&mut editor, serde_json::from_value(encoded).unwrap());
        apply(
            &mut editor,
            Command::PutViewGroup {
                name: "Verse".into(),
                selections: second.clone(),
            },
        );
        assert_eq!(editor.song().view_groups["Verse"], second);
        editor.undo().unwrap();
        assert_eq!(editor.song().view_groups["Verse"], first);
        editor.redo().unwrap();
        assert_eq!(editor.song().view_groups["Verse"], second);
        let delete = Command::DeleteViewGroup {
            name: "Verse".into(),
        };
        let encoded = serde_json::to_value(&delete).unwrap();
        assert_eq!(encoded["kind"], "delete_view_group");
        apply(&mut editor, serde_json::from_value(encoded).unwrap());
        assert!(editor.song().view_groups.is_empty());
        let revision = editor.revision();
        assert!(!editor
            .apply(
                revision,
                Command::DeleteViewGroup {
                    name: "Verse".into()
                }
            )
            .unwrap());
        assert_eq!(editor.revision(), revision);
        editor.undo().unwrap();
        assert_eq!(editor.song().view_groups["Verse"], second);
        editor.redo().unwrap();
        assert!(editor.song().view_groups.is_empty());
    }

    #[test]
    fn invalid_view_group_commands_preserve_document_and_history() {
        let mut editor = Editor::new(Song::default()).unwrap();
        let original = editor.song().clone();
        for command in [
            Command::PutViewGroup {
                name: " ".into(),
                selections: vec![selection(1, 20)],
            },
            Command::PutViewGroup {
                name: "Empty".into(),
                selections: vec![],
            },
            Command::PutViewGroup {
                name: "Repeated".into(),
                selections: vec![selection(1, 20), selection(1, 20)],
            },
            Command::DeleteViewGroup {
                name: "invalid\0name".into(),
            },
        ] {
            assert!(editor.apply(0, command).is_err());
            assert_eq!(editor.song(), &original);
            assert_eq!(editor.revision(), 0);
            assert!(!editor.can_undo());
            assert!(!editor.can_redo());
        }
    }

    #[test]
    fn structural_delete_keeps_stale_group_through_save_undo_and_new_allocation() {
        let mut editor = Editor::new(Song::default()).unwrap();
        let track = editor.song().tracks[0].id;
        apply(&mut editor, Command::AddPhrase { track, slot: 0 });
        let old_usage = editor.song().tracks[0].slots[0].as_ref().unwrap().id;
        let retained = ChainSelection {
            track,
            chain_use: old_usage,
        };
        apply(
            &mut editor,
            Command::PutViewGroup {
                name: "Keep this occurrence".into(),
                selections: vec![retained],
            },
        );
        let linked = editor.song().clone();
        let mut changed = editor.song().tracks[0].clone();
        changed.slots[0] = None;
        apply(&mut editor, Command::PutTrack { track: changed });
        assert_eq!(
            editor.song().view_groups["Keep this occurrence"],
            [retained]
        );
        assert!(editor.song().tracks[0].slots[0].is_none());
        assert!(editor.song().next_identity().unwrap() > old_usage.get());
        let restored: Song =
            serde_json::from_str(&serde_json::to_string(editor.song()).unwrap()).unwrap();
        restored.validate().unwrap();
        assert_eq!(&restored, editor.song());
        editor.undo().unwrap();
        assert_eq!(editor.song(), &linked);
        editor.redo().unwrap();
        assert_eq!(editor.song(), &restored);
        apply(&mut editor, Command::AddPhrase { track, slot: 0 });
        assert_ne!(
            editor.song().tracks[0].slots[0].as_ref().unwrap().id,
            old_usage
        );
        assert_eq!(
            editor.song().view_groups["Keep this occurrence"],
            [retained]
        );
    }
}

#[cfg(test)]
mod expression_edit_tests {
    use super::*;
    use crate::mpe::{BendRange, MpeProfile, MpeProvenance, MpeZone};

    fn beat(n: u64, d: u64) -> Beat {
        Beat::new(n, d).unwrap()
    }
    fn fixture() -> (Editor, PhraseId, EventId) {
        let profile = MpeProfile::new(
            "keyboard-17".into(),
            MpeZone::lower(4).unwrap(),
            BendRange::new(48, 0).unwrap(),
            BendRange::new(2, 0).unwrap(),
        )
        .unwrap();
        let mut note = Note::new(63, 101, beat(3, 5)).unwrap();
        note.release_velocity = 29;
        note.mpe = Some(MpeProvenance::new(profile, Channel::new(2).unwrap()).unwrap());
        note.expression_duration = Some(Beat::integer(2));
        note.expression = vec![Expression {
            offset: beat(3, 4),
            control: Control::Bend { value: 12983 },
            sequence: Some(7),
        }];
        note.master_expression = vec![Expression {
            offset: beat(7, 4),
            control: Control::Cc {
                number: 64,
                value: 0,
            },
            sequence: Some(9),
        }];
        let phrase = PhraseId::new(17).unwrap();
        let event = EventId::new(18).unwrap();
        let mut song = Song::default();
        song.phrases.insert(
            phrase,
            Phrase {
                events: vec![Event::note(
                    event,
                    beat(1, 8),
                    Target::Channel {
                        channel: Channel::new(2).unwrap(),
                        endpoint: None,
                    },
                    note,
                )],
                ..Phrase::default()
            },
        );
        (Editor::new(song).unwrap(), phrase, event)
    }
    fn note(editor: &Editor, phrase: PhraseId) -> &Note {
        editor.song().phrases[&phrase].events[0]
            .note
            .as_ref()
            .unwrap()
    }
    fn apply(editor: &mut Editor, command: Command) {
        assert!(editor.apply(editor.revision(), command).unwrap());
    }

    #[test]
    fn start_quantize_preserves_raw_scopes_and_undo_restores_exact_song() {
        let (mut editor, phrase, event) = fixture();
        let original = editor.song().clone();
        apply(
            &mut editor,
            Command::Quantize {
                phrase,
                events: vec![event],
                grid: beat(1, 4),
                durations: false,
            },
        );
        assert_eq!(editor.song().phrases[&phrase].events[0].at, beat(1, 4));
        assert_eq!(
            note(&editor, phrase),
            original.phrases[&phrase].events[0].note.as_ref().unwrap()
        );
        editor.undo().unwrap();
        assert_eq!(editor.song(), &original);
        editor.redo().unwrap();
        assert_eq!(
            note(&editor, phrase),
            original.phrases[&phrase].events[0].note.as_ref().unwrap()
        );
    }

    #[test]
    fn duration_quantize_preserves_mpe_control_lifetime_and_history() {
        let (mut editor, phrase, event) = fixture();
        let original = editor.song().clone();
        let raw = note(&editor, phrase).clone();
        apply(
            &mut editor,
            Command::Quantize {
                phrase,
                events: vec![event],
                grid: beat(1, 2),
                durations: true,
            },
        );
        let changed = note(&editor, phrase);
        assert_eq!(changed.duration, beat(1, 2));
        assert_eq!(changed.expression_duration, raw.expression_duration);
        assert_eq!(changed.expression, raw.expression);
        assert_eq!(changed.master_expression, raw.master_expression);
        assert_eq!(changed.mpe, raw.mpe);
        editor.undo().unwrap();
        assert_eq!(editor.song(), &original);
    }

    #[test]
    fn phrase_trim_requires_consent_for_pedal_tail_and_keeps_shorter_key_duration() {
        let (mut editor, phrase, _) = fixture();
        let original = editor.song().clone();
        assert!(editor
            .apply(
                editor.revision(),
                Command::ResizePhrase {
                    phrase,
                    steps: 4,
                    trim: false
                }
            )
            .unwrap_err()
            .to_string()
            .contains("explicit trim approval"));
        assert_eq!(editor.song(), &original);
        assert_eq!(editor.revision(), 0);
        apply(
            &mut editor,
            Command::ResizePhrase {
                phrase,
                steps: 4,
                trim: true,
            },
        );
        let changed = note(&editor, phrase);
        assert_eq!(changed.duration, beat(3, 5));
        assert_eq!(changed.expression_duration(), beat(7, 8));
        assert_eq!(
            changed.expression[0],
            original.phrases[&phrase].events[0]
                .note
                .as_ref()
                .unwrap()
                .expression[0]
        );
        assert_eq!(
            changed.expression.last().unwrap().control,
            Control::Cc {
                number: 64,
                value: 0
            }
        );
        assert_eq!(
            changed.master_expression.last().unwrap().control,
            Control::Cc {
                number: 64,
                value: 0
            }
        );
        assert_eq!(changed.expression.last().unwrap().sequence, Some(8));
        assert_eq!(changed.master_expression.last().unwrap().sequence, Some(9));
        editor.undo().unwrap();
        assert_eq!(editor.song(), &original);
    }

    #[test]
    fn division_retime_scales_key_tail_and_both_curves_then_undo_restores_raw_values() {
        let (mut editor, phrase, _) = fixture();
        let original = editor.song().clone();
        let raw = note(&editor, phrase).clone();
        apply(
            &mut editor,
            Command::RetimeDivision {
                phrase,
                division: Division::new(8, 1, 1).unwrap(),
            },
        );
        let changed = note(&editor, phrase);
        assert_eq!(editor.song().phrases[&phrase].events[0].at, beat(1, 4));
        assert_eq!(changed.duration, beat(6, 5));
        assert_eq!(changed.expression_duration(), Beat::integer(4));
        assert_eq!(changed.expression[0].offset, beat(3, 2));
        assert_eq!(changed.master_expression[0].offset, beat(7, 2));
        assert_eq!(changed.expression[0].control, raw.expression[0].control);
        assert_eq!(
            changed.master_expression[0].control,
            raw.master_expression[0].control
        );
        assert_eq!(changed.expression[0].sequence, raw.expression[0].sequence);
        assert_eq!(
            changed.master_expression[0].sequence,
            raw.master_expression[0].sequence
        );
        assert_eq!(changed.mpe, raw.mpe);
        editor.undo().unwrap();
        assert_eq!(editor.song(), &original);
    }

    #[test]
    fn ordinary_duration_quantize_failure_preserves_document_revision_and_undo() {
        let (editor, phrase, event) = fixture();
        let mut song = editor.song().clone();
        let mut ordinary = Note::new(63, 101, beat(3, 5)).unwrap();
        ordinary.expression = vec![Expression {
            offset: beat(11, 20),
            control: Control::Pressure { value: 89 },
            sequence: None,
        }];
        song.phrases.get_mut(&phrase).unwrap().events[0].note = Some(ordinary);
        let mut editor = Editor::new(song.clone()).unwrap();
        assert!(editor
            .apply(
                editor.revision(),
                Command::Quantize {
                    phrase,
                    events: vec![event],
                    grid: beat(1, 2),
                    durations: true
                }
            )
            .is_err());
        assert_eq!(editor.song(), &song);
        assert_eq!(editor.revision(), 0);
        assert!(!editor.can_undo());
    }
    #[test]
    fn note_dialog_batch_resizes_through_domain_helper_as_one_undo_step() {
        let (mut editor, phrase, event) = fixture();
        let original = editor.song().clone();
        let mut edited = original.phrases[&phrase].events[0].clone();
        edited.note.as_mut().unwrap().velocity = 87;
        apply(
            &mut editor,
            Command::Batch {
                commands: vec![
                    Command::PutEvent {
                        phrase,
                        event: edited,
                    },
                    Command::ResizeNotes {
                        phrase,
                        events: vec![event],
                        duration: beat(1, 4),
                    },
                ],
            },
        );
        let changed = note(&editor, phrase);
        assert_eq!(changed.velocity, 87);
        assert_eq!(changed.duration, beat(1, 4));
        assert_eq!(changed.expression_duration(), beat(1, 4));
        assert_eq!(
            changed.expression.last().unwrap().control,
            Control::Cc {
                number: 64,
                value: 0
            }
        );
        assert_eq!(
            changed.master_expression.last().unwrap().control,
            Control::Cc {
                number: 64,
                value: 0
            }
        );
        editor.undo().unwrap();
        assert_eq!(editor.song(), &original);
        assert!(!editor.can_undo());
    }

    #[test]
    fn invalid_note_resize_discards_other_edits_in_same_batch() {
        let (mut editor, phrase, event) = fixture();
        let original = editor.song().clone();
        let mut edited = original.phrases[&phrase].events[0].clone();
        edited.note.as_mut().unwrap().velocity = 87;
        assert!(editor
            .apply(
                editor.revision(),
                Command::Batch {
                    commands: vec![
                        Command::PutEvent {
                            phrase,
                            event: edited
                        },
                        Command::ResizeNotes {
                            phrase,
                            events: vec![event],
                            duration: Beat::ZERO
                        },
                    ]
                }
            )
            .is_err());
        assert_eq!(editor.song(), &original);
        assert_eq!(editor.revision(), 0);
        assert!(!editor.can_undo());
    }
}
