//! Tracker application state and immutable presentation. Sound stays in the host adapter.
#[cfg(test)]
pub(crate) mod demo;
mod drum_controls;
mod edit_preview;
mod grid;
mod piano_controls;
mod reference_edit;
pub(crate) use piano_controls::{PianoControl, PianoValue};
mod shared_edit;
mod timeline_focus;
#[cfg(test)]
mod timeline_playback;
mod timeline_window;
use crest_tracker::chord_bank::{
    ChordBank, ChordBankError, ChordDefinition, ChordRevision, ChordRevisionId,
};
use crest_tracker::composition::*;
use crest_tracker::editing::{Command, Editor, Occurrence};
use crest_tracker::harmony::{Chord, Direction as PitchDirection, Pitch, ScaleKind, Spelling};
use crest_tracker::playback::{
    Message, Owner, Playback, PlaybackBudget, PlaybackError, Snapshot, TimedEvent,
};
use crest_tracker::time::{Beat, Tempo};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use shared_edit::PendingEdit;
pub(crate) use shared_edit::SharedChoice;
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum View {
    Song,
    Chain,
    Phrase,
    Instruments,
    Settings,
    Expanded,
    Minimap,
}
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SynthPage {
    Instruments,
    Mixer,
    Sends,
    Settings,
}
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportScope {
    Song,
    Chain,
    Phrase,
}
/// Live host preference. It is not clip quantization or saved project content.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PerformanceLaunchGrid {
    #[default]
    Immediate,
    Beat,
    FourBeats,
}
impl PerformanceLaunchGrid {
    pub fn quantum(self) -> crest_performance_core::BeatTime {
        use crest_performance_core::{BeatTime, TICKS_PER_BEAT};
        BeatTime(match self {
            Self::Immediate => 0,
            Self::Beat => TICKS_PER_BEAT,
            Self::FourBeats => 4 * TICKS_PER_BEAT,
        })
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PerformanceInputMode {
    Play,
    Chords,
}
impl PerformanceInputMode {
    pub fn surface(self) -> crest_performance_core::SurfaceMode {
        match self {
            Self::Play => crest_performance_core::SurfaceMode::Play,
            Self::Chords => crest_performance_core::SurfaceMode::Chords,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "control", rename_all = "snake_case", deny_unknown_fields)]
pub enum PerformanceHarmonyEdit {
    Tonic {
        delta: i8,
    },
    Anchor {
        delta: i8,
    },
    Scale,
    Extension,
    Suspension,
    Borrowed,
    Inversion {
        delta: i8,
    },
    Register {
        delta: i8,
    },
    Spread,
    VoiceLeading,
    Expression,
    Playing,
    Degree {
        degree: u8,
    },
    Articulation {
        value: crest_performance_core::Articulation,
    },
    BlockArticulation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PerformanceChordEdit {
    LearnHeld,
    RestoreDiatonic,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    PerformanceInputMode {
        mode: PerformanceInputMode,
    },
    PerformanceHarmony {
        edit: PerformanceHarmonyEdit,
    },
    PerformanceChord {
        edit: PerformanceChordEdit,
    },
    PerformanceLaunchGrid {
        grid: PerformanceLaunchGrid,
    },
    PerformanceEdit {
        slot: usize,
    },
    PerformanceNote {
        token: u64,
        note: crest_performance_core::NoteKey,
        key: u8,
        start: crest_performance_core::BeatTime,
        duration: crest_performance_core::BeatTime,
    },
    PerformanceUndo {
        token: u64,
    },
    PerformanceInput,
    PerformanceClips {
        endpoint: String,
        pads: u16,
    },
    PerformancePlay,
    #[serde(skip)]
    PerformancePad {
        slot: usize,
    },
    PerformanceRecord {
        slot: usize,
    },
    PerformanceCaptureRecent {
        slot: usize,
    },
    PerformanceUndoClipChange,
    PerformanceFinish,
    PerformanceCancel,
    PerformanceAudition {
        slot: usize,
    },
    View {
        view: View,
    },
    BrowserFolders {
        sample: Option<String>,
        soundfont: Option<String>,
    },
    SynthSettings,
    #[serde(skip)]
    SynthAction {
        action: crest_synth::control::SemanticAction,
    },
    SynthPage {
        page: SynthPage,
    },
    PitchDisplay {
        intervals: bool,
        chromatic: bool,
    },
    PianoControl {
        control: PianoControl,
    },
    #[serde(skip)]
    PianoGesture {
        control: PianoControl,
        gesture: u64,
    },
    KeySettings {
        revision: u64,
        phrase: bool,
        root: u8,
        scale: Option<ScaleKind>,
        pitch_classes: Vec<u8>,
        spelling: Spelling,
    },
    SharedDecision {
        token: u64,
        choice: Option<SharedChoice>,
        remember: bool,
    },
    ResetSharedChoice,
    Focus {
        row: usize,
        column: usize,
    },
    Move {
        horizontal: i32,
        vertical: i32,
    },
    Adjust {
        delta: i32,
    },
    #[serde(skip)]
    AdjustGesture {
        delta: i32,
        gesture: u64,
    },
    #[serde(skip)]
    AdjustOctave {
        delta: i32,
        gesture: Option<u64>,
    },
    ControlsFocus {
        active: bool,
    },
    #[serde(skip)]
    EndGesture,
    Confirm,
    Back,
    Clear,
    Undo,
    Redo,
    Play,
    StartAt {
        scope: TransportScope,
        at: Beat,
    },
    Stop,
    Edit {
        revision: u64,
        command: Box<Command>,
    },
    PreviewEdit {
        revision: u64,
        token: u64,
        command: Box<Command>,
    },
    File {
        action: FileAction,
    },
    FileBrowse {
        token: u64,
        directory: String,
    },
    FileChoose {
        token: u64,
        path: String,
        overwrite: bool,
    },
    FileDecision {
        token: u64,
        decision: String,
    },
    FileCancel {
        token: u64,
    },
    Tempo {
        tempo: Tempo,
    },
    Rename {
        name: String,
    },
    Example,
    TextEntry {
        active: bool,
    },
    Capture {
        action: CaptureAction,
    },
    MidiOutput {
        action: String,
        endpoint: Option<String>,
    },
    Preferences {
        default_tempo: Tempo,
        monitor: bool,
        input_clock: bool,
        output_clock: Option<String>,
        delay_ms: u32,
        #[serde(default)]
        mpe_input: Option<crest_tracker::mpe::MpeProfile>,
    },
    MpeOutputProfile {
        endpoint: String,
        zones: Vec<crest_tracker::mpe::MpeZone>,
    },
    MidiControls {
        bindings: Vec<crate::preferences::MidiControlBinding>,
    },
    RollBindings {
        bindings: crate::preferences::RollBindings,
    },
    SoundLibrary {
        action: String,
        instrument: InstrumentId,
    },
    Recovery {
        action: String,
    },
    RepairResource {
        index: usize,
        action: String,
    },
    Arm {
        #[serde(default)]
        mpe: bool,
        instrument: Option<InstrumentId>,
        #[serde(default)]
        endpoint: Option<String>,
        monitor: bool,
        unique: bool,
    },
    AppendPhrase {
        phrase: Option<PhraseId>,
    },
    AssignPhrase {
        row: usize,
        phrase: PhraseId,
    },
    AssignChain {
        track: TrackId,
        slot: usize,
        chain: ChainId,
    },
    ToggleExpanded {
        track: TrackId,
        chain_use: UseId,
    },
    RecallViewGroup {
        name: String,
    },
    ClearExpanded,
    Locate {
        occurrence: Occurrence,
        at: Beat,
        row: usize,
        view: View,
        #[serde(default)]
        row_at: Option<Beat>,
    },
    PreviewBars {
        bars: u32,
    },
    PreviewStart {
        at: Beat,
    },
    BakeGenerator {
        revision: u64,
        occurrence: Occurrence,
        form: BakeForm,
    },
    DrumToggle {
        revision: u64,
        occurrence: Occurrence,
        step: u32,
        pitch: u8,
        target: Target,
        velocity: u8,
        duration: Beat,
    },
    DrumLane {
        revision: u64,
        occurrence: Occurrence,
        from_pitch: u8,
        from_target: Target,
        pitch: u8,
        target: Target,
        velocity: u8,
        duration: Beat,
    },
    DrumAddLane {
        revision: u64,
        occurrence: Occurrence,
        pitch: u8,
        target: Target,
        velocity: u8,
        duration: Beat,
    },
    SelectEvents {
        revision: u64,
        occurrence: Occurrence,
        events: Vec<EventId>,
        toggle: bool,
    },
    SelectionEdit {
        revision: u64,
        occurrence: Occurrence,
        action: SelectionAction,
    },
    SelectRange {
        revision: u64,
        occurrence: Occurrence,
        range: EventRange,
    },
    PreviewChord {
        revision: u64,
        token: u64,
        draft: ChordDraft,
    },
    SaveChord {
        revision: u64,
        draft: ChordDraft,
    },
    ChordLibrary {
        action: ChordLibraryAction,
    },
    ApplyChord {
        revision: u64,
        occurrence: Occurrence,
        event: EventId,
        slot: u8,
        root: u8,
        source: ChordSource,
    },
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChordDraft {
    pub(crate) name: String,
    pub(crate) intervals: Vec<i8>,
    pub(crate) root: u8,
    pub(crate) inversion: u8,
    pub(crate) open: bool,
    pub(crate) spread: u8,
}
impl ChordDraft {
    pub(crate) fn verified(&self) -> Result<ChordRevision, ChordBankError> {
        ChordRevision::new(
            self.name.clone(),
            ChordDefinition::new(
                Chord::new(self.intervals.clone())?,
                self.inversion,
                self.open,
                self.spread,
            )?,
            Pitch::new(self.root)?,
        )
    }
}
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ChordSource {
    Saved { revision: ChordRevisionId },
    Library { digest: String },
    Default { index: usize },
    Draft { draft: ChordDraft },
}
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ChordLibraryAction {
    Refresh,
    Publish { revision: u64, source: ChordSource },
    Adopt { revision: u64, digest: String },
}
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SelectionAction {
    Move { amount: Beat, backward: bool },
    Transpose { semitones: i16 },
    Duration { duration: Beat },
    Quantize { grid: Beat, durations: bool },
    Copy,
    Cut,
    Paste { at: Beat },
    Duplicate { at: Beat },
    Delete,
}

#[derive(Clone)]
struct EventSelection {
    phrase: PhraseId,
    events: Vec<EventId>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventRange {
    start: Beat,
    end: Beat,
    low: u8,
    high: u8,
    target: Option<Target>,
    include_controls: bool,
}
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BakeForm {
    Tracker,
    PianoRoll,
}

#[derive(Clone, Serialize)]
pub(crate) struct DrumNote {
    pub pitch: u8,
    pub target: Target,
    pub velocity: u8,
    pub duration: Beat,
}
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureAction {
    Record,
    End,
    Commit,
    SaveAsNewPhrase,
    Discard,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileAction {
    Example,
    PerformanceExample,
    PerformanceSampleExample,
    Corridors,
    ImportMidi,
    CollectAssets,
    New,
    Open,
    Save,
    SaveAs,
    Close,
    ImportSynth,
    ExportSynth,
}

#[derive(Clone, Copy)]
struct Selection {
    track: TrackId,
    chain_use: Option<UseId>,
    phrase_use: Option<UseId>,
}

struct PreviewCache {
    revision: u64,
    occurrence: Occurrence,
    phrase: Phrase,
    key: Key,
    value: Value,
    notes: BTreeMap<u64, (u8, u8, Target)>,
}

struct TimelineCache {
    song: Song,
    revision: u64,
    selection: Vec<ChainSelection>,
    bars: u32,
    start: Beat,
    view: View,
    timeline: Timeline,
}

#[derive(Clone, Serialize)]
struct Timeline {
    revision: u64,
    start: Beat,
    end: Beat,
    bars: Vec<BarMarker>,
    lanes: Vec<TimelineLane>,
    bounded: bool,
}

#[derive(Clone, Serialize)]
struct BarMarker {
    at: Beat,
    number: u32,
    meter: crest_tracker::time::Meter,
}

#[derive(Clone, Serialize)]
struct TimelineLane {
    track: TrackId,
    index: usize,
    name: String,
    slot: Option<usize>,
    chain_use: Option<UseId>,
    muted: bool,
    looped: bool,
    spans: Vec<PhraseSpan>,
    notice: Option<String>,
    conditional_from: Option<Beat>,
}

#[derive(Clone, Serialize)]
struct PhraseSpan {
    start: Beat,
    end: Beat,
    occurrence: Occurrence,
    phrase: PhraseId,
    name: String,
    repeat: u32,
    chain_repeat: u32,
    cycle: u32,
    generated: bool,
    conditional: bool,
    rows: Vec<TimelineRow>,
    activity: Vec<Activity>,
}

#[derive(Clone, Serialize)]
struct TimelineRow {
    at: Beat,
    source_row: usize,
    cells: Vec<String>,
}

/// A source occurrence and, when present, its exact displayed timeline row.
/// `start` is the occurrence's song beat; `row_at` is also in song beats.
#[derive(Clone, Copy)]
struct SourceLocation {
    occurrence: Occurrence,
    start: Beat,
    row: usize,
    row_at: Option<Beat>,
}

#[derive(Clone, Serialize)]
struct Activity {
    at: Beat,
    end: Beat,
    pitch: u8,
}

pub struct App {
    pub workspace: crate::workspace::Navigation,
    pub editor: Editor,
    pub view: View,
    pub row: usize,
    pub column: usize,
    pub track: usize,
    pub slot: usize,
    pub chain_row: usize,
    pub status: String,
    pub playing: bool,
    pub playback_scope: TransportScope,
    pub playback_loop: Option<Occurrence>,
    pub play_row: Option<u32>,
    pub play_beat: Option<Beat>,
    pub play_occurrence: Option<Occurrence>,
    pub play_phrase_beat: Option<Beat>,
    pub play_heads: Vec<crest_tracker::playback::AudiblePlayhead>,
    pub playback_revision: Option<u64>,
    pub playback_pending: bool,
    pub playback_fault: Option<String>,
    pub text_entry: bool,
    pub controls_focused: bool,
    pub sound_dirty: bool,
    pub capture_dirty: bool,
    pub performance_dirty: bool,
    pub path: Option<std::path::PathBuf>,
    pub mappings: BTreeMap<InstrumentId, u32>,
    pub requests: Vec<Request>,
    pub edit_held: bool,
    pub shift_held: bool,
    pub capture: Value,
    pub performance: Value,
    pub performance_editor: Option<crate::performance_editor::ClipEditor>,
    pub outputs: Value,
    pub recovery: Value,
    pub resource_repair: Value,
    pub preferences: Value,
    pub file_dialog: Value,
    pub chord_library: Value,
    preview_cache: RefCell<Option<PreviewCache>>,
    expanded_selection: Vec<ChainSelection>,
    pub(crate) preview_bars: u32,
    pub(crate) preview_start: Beat,
    pub(crate) timeline_focus: Option<Beat>,
    timeline_row: usize,
    /// None selects the first displayed time matching the source row in the focused span.
    timeline_row_at: Option<Beat>,
    timeline_cache: RefCell<Option<TimelineCache>>,
    event_selection: Option<EventSelection>,
    clipboard: Vec<Event>,
    chord_preview: Value,
    edit_preview: Value,
    adjustment_gesture: Option<u64>,
    pending_edit: Option<PendingEdit>,
    shared_choice: Option<SharedChoice>,
    shared_token: u64,
    pub(crate) pitch_intervals: bool,
    pub(crate) chromatic_entry: bool,
    pub(crate) roll_grid: Beat,
    roll_cursor: Beat,
    roll_pitch: u8,
    pub(crate) roll_value: PianoValue,
    roll_range_anchor: Option<(Beat, u8)>,
    drum_drafts: BTreeMap<PhraseId, Vec<DrumNote>>,
    pub fine_held: bool,
    pub range_held: bool,
}
impl App {
    pub fn new() -> Self {
        Self {
            workspace: crate::workspace::Navigation::default(),
            editor: Editor::new(Song::default()).expect("valid empty song"),
            view: View::Song,
            row: 0,
            column: 0,
            track: 0,
            slot: 0,
            chain_row: 0,
            status: "READY · Select a cell to create a chain".into(),
            playing: false,
            playback_scope: TransportScope::Song,
            playback_loop: None,
            play_row: None,
            play_beat: None,
            play_occurrence: None,
            play_phrase_beat: None,
            play_heads: Vec::new(),
            playback_revision: None,
            playback_pending: false,
            playback_fault: None,
            text_entry: false,
            controls_focused: false,
            sound_dirty: false,
            capture_dirty: false,
            performance_dirty: false,
            path: None,
            mappings: BTreeMap::new(),
            requests: Vec::new(),
            edit_held: false,
            shift_held: false,
            capture: json!({"state":"idle"}),
            performance: Value::Null,
            performance_editor: None,
            outputs: json!({}),
            recovery: Value::Null,
            resource_repair: Value::Null,
            preferences: Value::Null,
            file_dialog: Value::Null,
            chord_library: json!({"entries":[],"loading":false}),
            preview_cache: RefCell::new(None),
            expanded_selection: Vec::new(),
            preview_bars: 32,
            preview_start: Beat::ZERO,
            timeline_focus: None,
            timeline_row: 0,
            timeline_row_at: None,
            timeline_cache: RefCell::new(None),
            event_selection: None,
            clipboard: Vec::new(),
            chord_preview: Value::Null,
            edit_preview: Value::Null,
            adjustment_gesture: None,
            pending_edit: None,
            shared_choice: None,
            shared_token: 0,
            pitch_intervals: false,
            chromatic_entry: false,
            roll_grid: Beat::new(1, 4).expect("valid grid"),
            roll_cursor: Beat::ZERO,
            roll_pitch: 60,
            roll_value: PianoValue::Velocity,
            roll_range_anchor: None,
            drum_drafts: BTreeMap::new(),
            fine_held: false,
            range_held: false,
        }
    }
    pub fn dirty(&self) -> bool {
        self.editor.dirty() || self.sound_dirty || self.capture_dirty || self.performance_dirty
    }
    /// Drop event IDs when the host replaces the entire editor document.
    pub fn clear_event_selection(&mut self) {
        self.event_selection = None;
        self.pending_edit = None;
    }
    /// A replacement document may reuse phrase IDs from the previous Song.
    pub fn reset_document_editors(&mut self) {
        self.clear_event_selection();
        self.performance_editor = None;
        self.workspace = crate::workspace::Navigation::default();
        self.drum_drafts.clear();
        self.roll_cursor = Beat::ZERO;
        self.roll_pitch = 60;
        self.roll_range_anchor = None;
        self.timeline_focus = None;
        self.timeline_row = 0;
        self.timeline_row_at = None;
    }
    /// Install a validated document and discard transient source selections.
    pub fn replace_song(&mut self, song: Song) -> Result<(), DocumentError> {
        self.editor.replace_song(song)?;
        self.playback_fault = None;
        self.reset_document_editors();
        Ok(())
    }
    pub fn command(&mut self, command: Command) -> Result<(), DocumentError> {
        self.command_at(self.editor.revision(), command)
    }
    fn command_at(&mut self, revision: u64, command: Command) -> Result<(), DocumentError> {
        self.checked_revision(revision)?;
        let command = self.guard_shared_edit(command)?;
        self.commit_command(command)
    }
    fn commit_command(&mut self, command: Command) -> Result<(), DocumentError> {
        let selection = self.selection();
        let unique = matches!(
            command,
            Command::EditUnique { .. } | Command::MakeUnique { .. }
        );
        let drum_lanes = (unique && self.is_drum()).then(|| self.drum_lanes());
        let changed = match self.adjustment_gesture {
            Some(gesture) => self
                .editor
                .apply_gesture(self.editor.revision(), command, gesture)?,
            None => self.editor.apply(self.editor.revision(), command)?,
        };
        self.restore_selection(selection);
        if unique {
            self.event_selection = None;
            if let (Some(lanes), Some((id, _))) = (drum_lanes, self.selected_phrase()) {
                self.drum_drafts.insert(id, lanes);
            }
        }
        if changed {
            self.status = "EDITED".into();
        }
        Ok(())
    }
    pub fn apply(&mut self, request: Request) -> Result<(), DocumentError> {
        // Acknowledgements only retire presentation inputs. They may arrive
        // between held-repeat ticks and must not split an edit gesture.
        let end_gesture = !matches!(
            request,
            Request::AdjustGesture { .. }
                | Request::PianoGesture { .. }
                | Request::AdjustOctave {
                    gesture: Some(_),
                    ..
                }
        );
        match request {
            Request::View { view } => self.open_view(view),
            Request::PianoControl { control } => self.piano_control(control)?,
            Request::PianoGesture { control, gesture } => {
                if gesture == 0 {
                    return Err(DocumentError::Invalid(
                        "piano gesture identity must be nonzero",
                    ));
                }
                self.adjustment_gesture = Some(gesture);
                let result = self.piano_control(control);
                self.adjustment_gesture = None;
                result?;
            }
            Request::SharedDecision {
                token,
                choice,
                remember,
            } => self.resolve_shared_edit(token, choice, remember)?,
            Request::ResetSharedChoice => self.shared_choice = None,
            Request::PitchDisplay {
                intervals,
                chromatic,
            } => {
                self.pitch_intervals = intervals;
                self.chromatic_entry = chromatic;
                *self.timeline_cache.borrow_mut() = None;
            }
            Request::KeySettings {
                revision,
                phrase,
                root,
                scale,
                pitch_classes,
                spelling,
            } => {
                self.checked_revision(revision)?;
                let key = scale
                    .map(|scale| {
                        if scale == ScaleKind::Custom {
                            Key::custom(root, pitch_classes)
                        } else {
                            Key::new(root, scale)
                        }
                    })
                    .transpose()
                    .map_err(|_| DocumentError::Invalid("invalid key or custom pitch classes"))?
                    .map(|key| key.with_spelling(spelling));
                let command = if phrase {
                    let (id, original) = self
                        .selected_phrase()
                        .ok_or(DocumentError::Invalid("select a phrase first"))?;
                    let mut phrase = original.clone();
                    phrase.key = key;
                    Command::PutPhrase { id, phrase }
                } else {
                    Command::SetKey {
                        key: key.ok_or(DocumentError::Invalid("song key cannot inherit"))?,
                    }
                };
                self.command_at(revision, command)?;
            }
            Request::Focus { row, column } => {
                self.row = row;
                self.column = column;
            }
            Request::Move {
                horizontal,
                vertical,
            } => {
                if self.is_piano_roll() {
                    self.piano_control(PianoControl::Navigate {
                        horizontal,
                        vertical,
                    })?;
                } else {
                    self.navigate(horizontal, vertical);
                }
            }
            Request::Adjust { delta } => self.adjust(delta)?,
            Request::AdjustGesture { delta, gesture } => self.adjust_gesture(delta, gesture)?,
            Request::AdjustOctave { delta, gesture } => {
                self.adjustment_gesture = gesture;
                let result = self.adjust_value(delta.saturating_mul(12), true);
                self.adjustment_gesture = None;
                result?;
            }
            Request::ControlsFocus { active } => self.controls_focused = active,
            Request::EndGesture => self.editor.end_gesture(),
            Request::Confirm => self.confirm()?,
            Request::Back => self.open_view(match self.view {
                View::Phrase => View::Chain,
                _ => View::Song,
            }),
            Request::Clear => self.clear()?,
            Request::Undo => {
                let selection = self.selection();
                self.status = if self.editor.undo()? {
                    "UNDONE"
                } else {
                    "NOTHING TO UNDO"
                }
                .into();
                self.restore_selection(selection);
            }
            Request::Redo => {
                let selection = self.selection();
                self.status = if self.editor.redo()? {
                    "REDONE"
                } else {
                    "NOTHING TO REDO"
                }
                .into();
                self.restore_selection(selection);
            }
            Request::Edit { revision, command } => {
                self.command_at(revision, *command)?;
            }
            Request::PreviewEdit {
                revision,
                token,
                command,
            } => {
                self.edit_preview = match self.preview_edit(revision, *command) {
                    Ok(changes) => json!({"token":token,"revision":revision,"changes":changes}),
                    Err(error) => {
                        json!({"token":token,"revision":revision,"error":error.to_string()})
                    }
                };
            }
            Request::Tempo { tempo } => self.command(Command::SetTempo { tempo })?,
            Request::Rename { name } => self.command(Command::RenameSong { name })?,
            Request::TextEntry { active } => {
                self.text_entry = active;
                if active {
                    // A form owns subsequent key/button releases; no edit modifier
                    // may remain latched when focus returns to the composition.
                    self.controls_focused = false;
                    self.edit_held = false;
                    self.shift_held = false;
                }
            }
            Request::AppendPhrase { phrase } => self.append_phrase(phrase)?,
            Request::AssignPhrase { row, phrase } => self.assign_phrase(row, phrase)?,
            Request::AssignChain { track, slot, chain } => self.assign_chain(track, slot, chain)?,
            Request::ToggleExpanded { track, chain_use } => {
                self.toggle_expanded(track, chain_use)?
            }
            Request::RecallViewGroup { name } => self.recall_view_group(&name)?,
            Request::ClearExpanded => self.expanded_selection.clear(),
            Request::Locate {
                occurrence,
                at,
                row,
                view,
                row_at,
            } => {
                self.locate(
                    SourceLocation {
                        occurrence,
                        start: at,
                        row,
                        row_at,
                    },
                    view,
                )?;
            }
            Request::PreviewBars { bars } => {
                if !(1..=64).contains(&bars) {
                    return Err(DocumentError::Invalid("preview must cover 1–64 bars"));
                }
                self.preview_bars = bars;
            }
            Request::PreviewStart { at } => {
                self.preview_start = timeline_window::bar_at(self.editor.song(), at)?.at;
            }
            Request::BakeGenerator {
                revision,
                occurrence,
                form,
            } => self.bake_generator(revision, occurrence, form)?,
            Request::DrumToggle {
                revision,
                occurrence,
                step,
                pitch,
                target,
                velocity,
                duration,
            } => {
                self.drum_toggle(
                    revision,
                    occurrence,
                    step,
                    DrumNote {
                        pitch,
                        target,
                        velocity,
                        duration,
                    },
                )?;
            }
            Request::DrumLane {
                revision,
                occurrence,
                from_pitch,
                from_target,
                pitch,
                target,
                velocity,
                duration,
            } => {
                self.drum_lane(
                    revision,
                    occurrence,
                    from_pitch,
                    from_target,
                    DrumNote {
                        pitch,
                        target,
                        velocity,
                        duration,
                    },
                )?;
            }
            Request::DrumAddLane {
                revision,
                occurrence,
                pitch,
                target,
                velocity,
                duration,
            } => {
                self.add_drum_lane(
                    revision,
                    occurrence,
                    DrumNote {
                        pitch,
                        target,
                        velocity,
                        duration,
                    },
                )?;
            }
            Request::SelectEvents {
                revision,
                occurrence,
                events,
                toggle,
            } => {
                self.select_events(revision, occurrence, events, toggle)?;
            }
            Request::SelectionEdit {
                revision,
                occurrence,
                action,
            } => {
                self.edit_selection(revision, occurrence, action)?;
            }
            Request::PreviewChord {
                revision,
                token,
                draft,
            } => {
                let result = self
                    .checked_revision(revision)
                    .and_then(|_| draft.verified().map_err(DocumentError::from));
                self.chord_preview = match result {
                    Ok(chord) => json!({"token":token,"revision":revision,"chord":chord}),
                    Err(error) => {
                        json!({"token":token,"revision":revision,"error":error.to_string()})
                    }
                };
            }
            Request::SelectRange {
                revision,
                occurrence,
                range,
            } => self.select_range(revision, occurrence, range)?,
            Request::SaveChord { revision, draft } => self.save_chord(revision, draft)?,
            Request::ApplyChord {
                revision,
                occurrence,
                event,
                slot,
                root,
                source,
            } => self.apply_chord(revision, occurrence, event, slot, root, source)?,
            Request::PerformanceInputMode { .. }
            | Request::PerformanceHarmony { .. }
            | Request::PerformanceChord { .. }
            | Request::PerformanceAudition { .. }
            | Request::PerformanceLaunchGrid { .. }
            | Request::PerformanceClips { .. }
            | Request::PerformancePlay
            | Request::PerformancePad { .. }
            | Request::PerformanceEdit { .. }
            | Request::PerformanceNote { .. }
            | Request::PerformanceUndo { .. }
            | Request::PerformanceInput
            | Request::PerformanceRecord { .. }
            | Request::PerformanceCaptureRecent { .. }
            | Request::PerformanceUndoClipChange
            | Request::PerformanceFinish
            | Request::PerformanceCancel
            | Request::Play
            | Request::Example
            | Request::StartAt { .. }
            | Request::Stop
            | Request::SynthSettings
            | Request::SynthPage { .. }
            | Request::ChordLibrary { .. }
            | Request::File { .. }
            | Request::FileBrowse { .. }
            | Request::FileChoose { .. }
            | Request::FileDecision { .. }
            | Request::FileCancel { .. }
            | Request::Capture { .. }
            | Request::MidiOutput { .. }
            | Request::Preferences { .. }
            | Request::SoundLibrary { .. }
            | Request::Recovery { .. }
            | Request::RepairResource { .. }
            | Request::MpeOutputProfile { .. }
            | Request::MidiControls { .. }
            | Request::RollBindings { .. }
            | Request::Arm { .. }
            | Request::SynthAction { .. }
            | Request::BrowserFolders { .. } => {}
        }
        if end_gesture {
            self.editor.end_gesture();
        }
        self.clamp_focus();
        Ok(())
    }
    fn clamp_focus(&mut self) {
        self.track = self.track.min(self.editor.song().tracks.len() - 1);
        self.chain_row = self.chain_row.min(
            self.selected_chain()
                .map_or(0, |(_, chain)| chain.phrases.len()),
        );
        if self.arrangement_view() {
            let track = self.column.min(self.editor.song().tracks.len() - 1);
            let slot = self.row;
            if (track, slot) != (self.track, self.slot) {
                self.chain_row = 0;
            }
            self.track = track;
            self.slot = slot;
            self.row = slot;
            self.column = track;
            return;
        }
        let (rows, columns) = match self.view {
            View::Song | View::Expanded | View::Minimap => (
                self.editor.song().tracks[self.track].slots.len() + 1,
                self.editor.song().tracks.len(),
            ),
            View::Chain => (
                self.selected_chain()
                    .map_or(1, |(_, c)| c.phrases.len() + 1),
                2,
            ),
            View::Phrase => (
                self.selected_phrase().map_or(1, |(_, p)| p.steps as usize),
                if self.is_drum() {
                    self.drum_lanes().len().max(1)
                } else {
                    6
                },
            ),
            _ => (1, 1),
        };
        self.row = self.row.min(rows.saturating_sub(1));
        self.column = self.column.min(columns - 1);
        if self.view == View::Chain {
            self.chain_row = self.row;
        }
        if self.view == View::Phrase {
            self.chain_row = self.chain_row.min(
                self.selected_chain()
                    .map_or(0, |(_, chain)| chain.phrases.len().saturating_sub(1)),
            );
            self.reconcile_piano_cursor();
        }
    }
    fn arrangement_view(&self) -> bool {
        matches!(self.view, View::Song | View::Expanded | View::Minimap)
    }
    fn open_view(&mut self, view: View) {
        self.roll_range_anchor = None;
        self.controls_focused = false;
        self.view = view;
        if self.arrangement_view() {
            self.row = self.slot;
            self.column = self.track;
        } else {
            self.row = if view == View::Chain {
                self.chain_row
            } else {
                0
            };
            self.column = 0;
        }
    }
    fn selection(&self) -> Selection {
        let track = &self.editor.song().tracks[self.track.min(self.editor.song().tracks.len() - 1)];
        let chain_use = track.slots.get(self.slot).and_then(Option::as_ref);
        Selection {
            track: track.id,
            chain_use: chain_use.map(|usage| usage.id),
            phrase_use: self.occurrence().map(|path| path.phrase_use),
        }
    }
    fn restore_selection(&mut self, selection: Selection) {
        self.track = self
            .editor
            .song()
            .tracks
            .iter()
            .position(|track| track.id == selection.track)
            .unwrap_or(self.track.min(self.editor.song().tracks.len() - 1));
        let track = &self.editor.song().tracks[self.track];
        self.slot = selection
            .chain_use
            .and_then(|id| {
                track
                    .slots
                    .iter()
                    .position(|slot| slot.as_ref().is_some_and(|usage| usage.id == id))
            })
            .unwrap_or(self.slot);
        if let Some((_, chain)) = self.selected_chain() {
            self.chain_row = selection
                .phrase_use
                .and_then(|id| chain.phrases.iter().position(|usage| usage.id == id))
                .unwrap_or(self.chain_row.min(chain.phrases.len().saturating_sub(1)));
        } else {
            self.chain_row = 0;
        }
        if matches!(self.view, View::Chain | View::Phrase) && self.selected_chain().is_none() {
            self.open_view(View::Song);
        } else if self.view == View::Phrase && self.selected_phrase().is_none() {
            self.open_view(View::Chain);
        }
        if self.arrangement_view() {
            self.row = self.slot;
            self.column = self.track;
        }
        if self.view == View::Chain {
            self.row = self.chain_row;
        }
        self.clamp_focus();
    }
    pub fn occurrence(&self) -> Option<Occurrence> {
        let track = self.editor.song().tracks.get(self.track)?;
        let chain_use = track.slots.get(self.slot)?.as_ref()?;
        let phrase_use = self
            .editor
            .song()
            .chains
            .get(&chain_use.chain)?
            .phrases
            .get(self.chain_row)?;
        Some(Occurrence {
            track: track.id,
            chain_use: chain_use.id,
            phrase_use: phrase_use.id,
        })
    }
    fn navigate(&mut self, horizontal: i32, vertical: i32) {
        if self.is_drum() {
            self.row = self.row.saturating_add_signed(horizontal as isize);
            self.column = self.column.saturating_add_signed(vertical as isize);
            self.clamp_focus();
            return;
        }
        if matches!(self.view, View::Expanded | View::Minimap)
            && self.navigate_timeline(horizontal, vertical)
        {
            return;
        }
        self.column = self.column.saturating_add_signed(horizontal as isize);
        self.row = self.row.saturating_add_signed(vertical as isize);
        self.clamp_focus();
    }
    pub fn selected_chain(&self) -> Option<(ChainId, &Chain)> {
        let usage = self
            .editor
            .song()
            .tracks
            .get(self.track)?
            .slots
            .get(self.slot)?
            .as_ref()?;
        Some((usage.chain, self.editor.song().chains.get(&usage.chain)?))
    }
    pub fn selected_phrase(&self) -> Option<(PhraseId, &Phrase)> {
        let (_, chain) = self.selected_chain()?;
        let usage = chain.phrases.get(self.chain_row)?;
        Some((usage.phrase, self.editor.song().phrases.get(&usage.phrase)?))
    }
    /// Confirm opens existing sources or selects occupied Piano notes without
    /// editing music. The host uses this distinction while a take is retained.
    pub fn confirm_edits_song(&self) -> bool {
        match self.view {
            View::Song => self.selected_chain().is_none(),
            View::Chain => self
                .selected_chain()
                .is_none_or(|(_, chain)| chain.phrases.get(self.row).is_none()),
            View::Phrase if self.is_piano_roll() => self.piano_notes_at_cursor().is_empty(),
            View::Phrase => true,
            _ => false,
        }
    }
    fn confirm(&mut self) -> Result<(), DocumentError> {
        match self.view {
            View::Expanded | View::Minimap => {
                self.open_timeline_source()?;
            }
            View::Song => {
                if self.selected_chain().is_none() {
                    let track = &self.editor.song().tracks[self.track];
                    self.command(Command::AddPhrase {
                        track: track.id,
                        slot: self.slot,
                    })?;
                }
                self.chain_row = 0;
                self.row = 0;
                self.column = 0;
                self.view = View::Chain;
            }
            View::Chain => {
                self.chain_row = self.row;
                if self.selected_phrase().is_none() {
                    self.append_phrase(None)?;
                }
                if self.selected_phrase().is_some() {
                    self.open_view(View::Phrase);
                }
            }
            View::Phrase if self.is_piano_roll() => self.piano_confirm()?,
            View::Phrase if self.is_drum() => self.drum_confirm(false)?,
            View::Phrase => self.adjust(1)?,
            _ => {}
        }
        Ok(())
    }
    fn adjust_gesture(&mut self, delta: i32, gesture: u64) -> Result<(), DocumentError> {
        if gesture == 0 {
            return Err(DocumentError::Invalid(
                "edit gesture identity must be nonzero",
            ));
        }
        self.adjustment_gesture = Some(gesture);
        let result = self.adjust(delta);
        // Clear context before propagating errors so a failed held adjustment
        // cannot accidentally coalesce a later ordinary command.
        self.adjustment_gesture = None;
        result
    }
    fn adjust(&mut self, delta: i32) -> Result<(), DocumentError> {
        self.adjust_value(delta, false)
    }
    fn adjust_value(&mut self, delta: i32, chromatic_pitch: bool) -> Result<(), DocumentError> {
        if delta == 0 {
            return Ok(());
        }
        if self.view == View::Song {
            return self.adjust_chain_reference(delta);
        }
        if self.view == View::Chain {
            return if self.column == 0 {
                self.adjust_phrase_reference(delta)
            } else {
                self.adjust_repeats(delta)
            };
        }
        if self.view != View::Phrase {
            return Ok(());
        }
        if self.is_drum() {
            return self.adjust_drum_velocity(delta);
        }
        let (id, phrase) = self
            .selected_phrase()
            .ok_or(DocumentError::Invalid("select a phrase first"))?;
        let at = phrase
            .division
            .beat_length()
            .checked_scale(self.row as u64, 1)?;
        let mut event = match phrase.events.iter().find(|event| event.at == at) {
            Some(event) => event.clone(),
            None => Event::note(
                EventId::new(self.editor.song().next_identity()?)?,
                at,
                self.default_target()?,
                Note::new(
                    self.entry_pitch(phrase)?,
                    100,
                    phrase.division.beat_length().checked_scale(9, 10)?,
                )?,
            ),
        };
        if (3..=5).contains(&self.column) {
            if let Some(Fx::ChordRevision { revision }) = event.fx[self.column - 3] {
                if let Some(note) = &event.note {
                    return self.cycle_chord(event.id, note.pitch, revision, delta);
                }
            }
        }
        match self.column {
            0 => {
                if let Some(note) = &mut event.note {
                    let key = phrase.key.as_ref().unwrap_or(&self.editor.song().key);
                    let direction = if delta >= 0 {
                        PitchDirection::Up
                    } else {
                        PitchDirection::Down
                    };
                    if phrase.events.iter().any(|v| v.id == event.id) {
                        if chromatic_pitch
                            || self.chromatic_entry
                            || matches!(phrase.form, PhraseForm::PianoRoll)
                        {
                            note.pitch =
                                i32::from(note.pitch).saturating_add(delta).clamp(0, 127) as u8;
                        } else {
                            let mut pitch = Pitch::new(note.pitch)
                                .map_err(|_| DocumentError::Invalid("invalid pitch"))?;
                            for _ in 0..delta.unsigned_abs() {
                                let Some(next) = key.next_pitch(pitch, direction) else {
                                    break;
                                };
                                pitch = next;
                            }
                            note.pitch = pitch.midi();
                        }
                    }
                } else {
                    event.note = Some(Note::new(
                        self.entry_pitch(phrase)?,
                        100,
                        phrase.division.beat_length().checked_scale(9, 10)?,
                    )?);
                }
            }
            1 => {
                let ids: Vec<_> = self.editor.song().instruments.keys().copied().collect();
                let current = match event.target {
                    Target::Instrument { id, .. } => ids.iter().position(|v| *v == id).unwrap_or(0),
                    _ => 0,
                };
                if let Some(id) = ids.get(
                    current
                        .saturating_add_signed(delta as isize)
                        .min(ids.len().saturating_sub(1)),
                ) {
                    event.target = Target::instrument(*id);
                }
            }
            2 => {
                if let Some(note) = &mut event.note {
                    note.velocity =
                        i32::from(note.velocity).saturating_add(delta).clamp(1, 127) as u8
                }
            }
            3..=5 => {
                let slot = &mut event.fx[self.column - 3];
                let value = match slot {
                    Some(Fx::Offset { semitones }) => i32::from(*semitones),
                    None => 0,
                    _ => {
                        return Err(DocumentError::Invalid(
                            "use Edit + Up to edit this FX value",
                        ))
                    }
                };
                *slot = Some(Fx::Offset {
                    semitones: value.saturating_add(delta).clamp(-24, 24) as i8,
                });
            }
            _ => {}
        }
        self.command(Command::PutEvent { phrase: id, event })
    }
    pub(crate) fn default_target(&self) -> Result<Target, DocumentError> {
        self.editor
            .song()
            .instruments
            .keys()
            .next()
            .copied()
            .map(Target::instrument)
            .ok_or(DocumentError::Invalid("choose an instrument first"))
    }
    fn entry_pitch(&self, phrase: &Phrase) -> Result<u8, DocumentError> {
        let key = phrase.key.as_ref().unwrap_or(&self.editor.song().key);
        let middle_c = Pitch::new(60).expect("middle C is a MIDI pitch");
        if key.contains(middle_c) {
            return Ok(60);
        }
        key.next_pitch(middle_c, PitchDirection::Up)
            .map(Pitch::midi)
            .ok_or(DocumentError::Invalid("scale has no available entry pitch"))
    }
    fn adjust_repeats(&mut self, delta: i32) -> Result<(), DocumentError> {
        if self.column != 1 {
            return Ok(());
        }
        let Some((id, chain)) = self.selected_chain() else {
            return Ok(());
        };
        let mut chain = chain.clone();
        let Some(usage) = chain.phrases.get_mut(self.chain_row) else {
            return Ok(());
        };
        usage.repeats =
            (i64::from(usage.repeats) + i64::from(delta)).clamp(1, i64::from(u32::MAX)) as u32;
        self.command(Command::PutChain { id, chain })
    }
    fn append_phrase(&mut self, source: Option<PhraseId>) -> Result<(), DocumentError> {
        let (chain_id, chain) = self
            .selected_chain()
            .ok_or(DocumentError::Invalid("select a chain first"))?;
        let mut chain = chain.clone();
        let row = chain.phrases.len();
        let next = self.editor.song().next_identity()?;
        let mut commands = Vec::new();
        let (phrase, usage) = if let Some(phrase) = source {
            if !self.editor.song().phrases.contains_key(&phrase) {
                return Err(DocumentError::Missing {
                    kind: "phrase",
                    id: phrase.get(),
                });
            }
            (phrase, UseId::new(next)?)
        } else {
            let id = PhraseId::new(next)?;
            commands.push(Command::PutPhrase {
                id,
                phrase: Phrase {
                    name: format!("{:02}", self.editor.song().phrases.len()),
                    ..Phrase::default()
                },
            });
            (id, UseId::new(fresh_identity(next, 1)?)?)
        };
        chain.phrases.push(PhraseUse {
            id: usage,
            phrase,
            repeats: 1,
        });
        commands.push(Command::PutChain {
            id: chain_id,
            chain,
        });
        self.command(Command::Batch { commands })?;
        self.chain_row = row;
        self.open_view(View::Chain);
        Ok(())
    }
    fn assign_phrase(&mut self, row: usize, phrase: PhraseId) -> Result<(), DocumentError> {
        if !self.editor.song().phrases.contains_key(&phrase) {
            return Err(DocumentError::Missing {
                kind: "phrase",
                id: phrase.get(),
            });
        }
        let (id, chain) = self
            .selected_chain()
            .ok_or(DocumentError::Invalid("select a chain first"))?;
        if row > chain.phrases.len() {
            return Err(DocumentError::Invalid(
                "phrase row must exist or append to the chain",
            ));
        }
        if row == chain.phrases.len() {
            return self.append_phrase(Some(phrase));
        }
        let mut chain = chain.clone();
        chain.phrases[row].phrase = phrase;
        self.command(Command::PutChain { id, chain })?;
        self.chain_row = row;
        self.open_view(View::Chain);
        Ok(())
    }
    fn assign_chain(
        &mut self,
        track_id: TrackId,
        slot: usize,
        chain: ChainId,
    ) -> Result<(), DocumentError> {
        if !self.editor.song().chains.contains_key(&chain) {
            return Err(DocumentError::Missing {
                kind: "chain",
                id: chain.get(),
            });
        }
        let index = self
            .editor
            .song()
            .tracks
            .iter()
            .position(|track| track.id == track_id)
            .ok_or(DocumentError::Missing {
                kind: "track",
                id: track_id.get(),
            })?;
        let mut track = self.editor.song().tracks[index].clone();
        let usage = match track.slots.get(slot).and_then(Option::as_ref) {
            Some(usage) if usage.chain == chain => usage.clone(),
            _ => ChainUse {
                id: UseId::new(self.editor.song().next_identity()?)?,
                chain,
                repeats: 1,
            },
        };
        track.ensure_slot(slot)?;
        track.slots[slot] = Some(usage);
        self.command(Command::PutTrack { track })?;
        self.track = index;
        self.slot = slot;
        self.chain_row = 0;
        self.open_view(View::Song);
        Ok(())
    }
    fn clear(&mut self) -> Result<(), DocumentError> {
        if self.is_drum() {
            return self.drum_confirm(true);
        }
        if self.arrangement_view() {
            let mut track = self.editor.song().tracks[self.track].clone();
            if let Some(slot) = track.slots.get_mut(self.slot) {
                *slot = None;
                self.command(Command::PutTrack { track })?;
            }
            return Ok(());
        }
        if self.view == View::Chain {
            if let Some((id, chain)) = self.selected_chain() {
                let mut chain = chain.clone();
                if self.chain_row < chain.phrases.len() {
                    chain.phrases.remove(self.chain_row);
                    self.command(Command::PutChain { id, chain })?;
                }
            }
            return Ok(());
        }
        if self.view != View::Phrase {
            return Ok(());
        }
        if self
            .selected_phrase()
            .is_some_and(|(_, phrase)| matches!(phrase.form, PhraseForm::PianoRoll))
            && !self.selected_event_ids().is_empty()
        {
            return self.edit_selection(
                self.editor.revision(),
                self.occurrence()
                    .ok_or(DocumentError::Invalid("select a phrase occurrence first"))?,
                SelectionAction::Delete,
            );
        }
        if let Some((id, phrase)) = self.selected_phrase() {
            let at = phrase
                .division
                .beat_length()
                .checked_scale(self.row as u64, 1)?;
            let Some(mut event) = phrase.events.iter().find(|event| event.at == at).cloned() else {
                return Ok(());
            };
            match self.column {
                0 => event.note = None,
                3..=5 => event.fx[self.column - 3] = None,
                _ => {
                    return Err(DocumentError::Invalid(
                        "target and velocity are required; adjust them instead",
                    ))
                }
            }
            let command = if event.note.is_none()
                && event.controls.is_empty()
                && event.fx.iter().all(Option::is_none)
            {
                Command::DeleteEvents {
                    phrase: id,
                    events: vec![event.id],
                }
            } else {
                Command::PutEvent { phrase: id, event }
            };
            self.command(command)?;
        }
        Ok(())
    }
    #[cfg(test)]
    pub fn projection(&self) -> Value {
        self.build_projection(true)
    }
    #[cfg(test)]
    fn build_projection(&self, include_phrase: bool) -> Value {
        let song = self.editor.song();
        let generator_preview = self.generator_preview();
        let (title, subtitle, columns, rows) = self.grid();
        let mut projection = json!({"view":self.view,"nextIdentity":song.next_identity().ok(),"revision":self.editor.revision(),"song":song.name,"title":title,"subtitle":subtitle,
            "tempo":song.tempo.bpm(),"key":song.key.name(),"status":self.status,"playing":self.playing,"playRow":self.play_row,
            "playBeat":self.play_beat,"playbackRevision":self.playback_revision,
            "playbackPending":self.playback_pending,"playbackFault":self.playback_fault,
            "dirty":self.dirty(),"row":self.row,"column":self.column,"columns":columns,"rows":rows.values,
            "track":self.track,"slot":self.slot,
            "canUndo":self.editor.can_undo(),"canRedo":self.editor.can_redo(),"instruments":song.instruments,
            "chains":song.chains.iter().map(|(id,chain)| (id, json!({"name":chain.name}))).collect::<BTreeMap<_,_>>(),
            "phrases":song.phrases.iter().map(|(id,phrase)| (id, json!({"name":phrase.name,"steps":phrase.steps}))).collect::<BTreeMap<_,_>>(),
            "tracks":song.tracks,"chainRow":self.chain_row,"occurrence":self.occurrence(),"capture":self.capture,"outputs":self.outputs,"recovery":self.recovery,"resourceRepair":self.resource_repair,"scale":song.key,
            "selectedChain":self.selected_chain().map(|(id,c)|json!({"id":id,"value":c})),"generatorPreview":generator_preview,
            "path":self.path.as_ref().and_then(|v|v.file_name()).map(|v|v.to_string_lossy())});
        if include_phrase {
            projection["phrase"] = self
                .selected_phrase()
                .map_or(Value::Null, |(id, phrase)| json!({"id":id,"value":phrase}));
        }
        projection.as_object_mut().expect("projection is an object").extend(
            json!({"rowStart":rows.start,"rowCount":rows.total,"rowPageSize":grid::PAGE_ROWS,
                "timeline":self.timeline_projection(),"timelineFocus":self.timeline_focus,"timelineRow":self.timeline_row,"timelineRowAt":self.timeline_row_at,"expandedSelection":self.expanded_selection,"previewBars":self.preview_bars,
                "selectedEvents":self.selected_event_ids(),"clipboardCount":self.clipboard.len(),"controlsFocused":self.controls_focused,
                "playOccurrence":self.play_occurrence,"playPhraseBeat":self.play_phrase_beat})
                .as_object().expect("presentation fields are an object").clone(),
        );
        projection["chordBank"] = self.chord_bank_projection();
        projection["chordLibrary"] = self.chord_library.clone();
        projection["playHeads"] = json!(self.play_heads);
        projection["timelinePlayheads"] = self.timeline_playheads();
        projection["sharedEdit"] = self.shared_edit_projection();
        projection["sharedReferences"] = self.shared_references_projection();
        projection["sharedChoice"] = json!(self.shared_choice);
        projection["pitchIntervals"] = json!(self.pitch_intervals);
        projection["phraseKey"] = json!(self.selected_phrase().map(|(_, phrase)| phrase
            .key
            .as_ref()
            .unwrap_or(&song.key)
            .name()));
        if self.is_piano_roll() {
            let key = self
                .selected_phrase()
                .and_then(|(_, phrase)| phrase.key.as_ref())
                .unwrap_or(&song.key);
            projection["pitchLabels"] = json!((0..=127)
                .map(|midi| {
                    let pitch = Pitch::new(midi).expect("valid pitch range");
                    if self.pitch_intervals {
                        format!(
                            "{} · oct {}",
                            key.interval_name(pitch),
                            i16::from(midi) / 12 - 1
                        )
                    } else {
                        key.pitch_name(pitch)
                    }
                })
                .collect::<Vec<_>>());
        }
        projection["pianoControls"] = json!({"grid":self.roll_grid,"at":self.roll_cursor,"pitch":self.roll_pitch,"value":self.roll_value,"fine":self.fine_held,"range":self.range_held});
        projection["drumLanes"] = json!(self.drum_lanes());
        projection["chromaticEntry"] = json!(self.chromatic_entry);
        projection["scaleCatalog"] =
            json!(ScaleKind::CATALOG.map(|kind| json!({"value":kind,"label":kind.name()})));
        projection["preferences"] = self.preferences.clone();
        projection["fileDialog"] = self.file_dialog.clone();
        projection["chordPreview"] = self.chord_preview.clone();
        projection["editPreview"] = self.edit_preview.clone();
        projection["meters"] = json!(song.meters);
        projection["viewSelection"] = json!(self.timeline_selection());
        projection["expandedMissing"] =
            json!(self.missing_chain_selections(&self.expanded_selection));
        projection["viewGroups"] = json!(song.view_groups.iter().map(|(name, selections)| {
            (name.clone(), json!({"selections":selections,"missing":self.missing_chain_selections(selections)}))
        }).collect::<BTreeMap<_, _>>());
        projection
    }
    fn missing_chain_selections(&self, selections: &[ChainSelection]) -> usize {
        selections
            .iter()
            .filter(|selection| {
                !self.editor.song().tracks.iter().any(|track| {
                    track.id == selection.track
                        && track
                            .slots
                            .iter()
                            .flatten()
                            .any(|usage| usage.id == selection.chain_use)
                })
            })
            .count()
    }
    fn recall_view_group(&mut self, name: &str) -> Result<(), DocumentError> {
        let selections = self
            .editor
            .song()
            .view_groups
            .get(name)
            .ok_or(DocumentError::Invalid("saved View Group no longer exists"))?
            .clone();
        let missing = self.missing_chain_selections(&selections);
        self.expanded_selection = selections;
        self.open_view(View::Expanded);
        self.status = if missing == 0 {
            format!("VIEW GROUP · {name}")
        } else {
            format!("VIEW GROUP · {name} · {missing} missing source(s)")
        };
        Ok(())
    }
    fn toggle_expanded(&mut self, track: TrackId, chain_use: UseId) -> Result<(), DocumentError> {
        let selection = ChainSelection { track, chain_use };
        if !self.editor.song().tracks.iter().any(|lane| {
            lane.id == track
                && lane
                    .slots
                    .iter()
                    .flatten()
                    .any(|usage| usage.id == chain_use)
        }) {
            return Err(DocumentError::Invalid(
                "selected chain occurrence no longer exists",
            ));
        }
        if let Some(index) = self
            .expanded_selection
            .iter()
            .position(|item| *item == selection)
        {
            self.expanded_selection.remove(index);
        } else {
            if self.expanded_selection.len() >= MAX_CHAIN_SELECTIONS {
                return Err(DocumentError::Invalid(
                    "Expanded supports up to 16 selected chains",
                ));
            }
            self.expanded_selection.push(selection);
        }
        Ok(())
    }
    fn locate(&mut self, location: SourceLocation, view: View) -> Result<(), DocumentError> {
        if !matches!(
            view,
            View::Song | View::Chain | View::Phrase | View::Expanded | View::Minimap
        ) {
            return Err(DocumentError::Invalid(
                "occurrence navigation requires an arrangement or phrase view",
            ));
        }
        let exact_source_at = self.timeline_source_beat(&location, view)?;
        let SourceLocation {
            occurrence,
            start,
            row,
            row_at,
        } = location;
        let song = self.editor.song();
        let track = song
            .tracks
            .iter()
            .position(|track| track.id == occurrence.track)
            .ok_or(DocumentError::Invalid("source track no longer exists"))?;
        let slot = song.tracks[track]
            .slots
            .iter()
            .position(|slot| {
                slot.as_ref()
                    .is_some_and(|usage| usage.id == occurrence.chain_use)
            })
            .ok_or(DocumentError::Invalid(
                "source chain occurrence no longer exists",
            ))?;
        let chain = &song.chains[&song.tracks[track].slots[slot]
            .as_ref()
            .expect("located chain use")
            .chain];
        let chain_row = chain
            .phrases
            .iter()
            .position(|usage| usage.id == occurrence.phrase_use)
            .ok_or(DocumentError::Invalid(
                "source phrase occurrence no longer exists",
            ))?;
        let phrase = &song.phrases[&chain.phrases[chain_row].phrase];
        let source_row = row.min(phrase.steps as usize - 1);
        let source_at = match exact_source_at {
            Some(at) => at,
            None => phrase
                .division
                .beat_length()
                .checked_scale(source_row as u64, 1)?,
        };
        self.track = track;
        self.slot = slot;
        self.chain_row = chain_row;
        self.open_view(view);
        if view == View::Phrase {
            self.row = source_row;
            self.focus_piano_source(source_at);
        }
        self.timeline_focus = Some(start);
        self.timeline_row = source_row;
        self.timeline_row_at = row_at;
        Ok(())
    }
    fn timeline_selection(&self) -> Vec<ChainSelection> {
        if !self.expanded_selection.is_empty() {
            return self.expanded_selection.clone();
        }
        let selected = self.selection();
        selected
            .chain_use
            .map(|chain_use| {
                vec![ChainSelection {
                    track: selected.track,
                    chain_use,
                }]
            })
            .unwrap_or_default()
    }
    pub(crate) fn follow_minimap_window(&mut self, at: Beat) -> Result<(), DocumentError> {
        let current = timeline_window::bar_at(self.editor.song(), at)?;
        let first = timeline_window::bar_at(self.editor.song(), self.preview_start)?;
        if current.number < first.number || current.number - first.number >= self.preview_bars {
            self.preview_start = current.at;
        }
        Ok(())
    }

    pub(crate) fn timeline_projection(&self) -> Value {
        if !matches!(self.view, View::Expanded | View::Minimap) {
            return Value::Null;
        }
        match self.timeline() {
            Ok(timeline) => {
                serde_json::to_value(timeline).expect("timeline has serializable values")
            }
            Err(error) => json!({"error":error.to_string()}),
        }
    }
    fn timeline(&self) -> Result<Timeline, DocumentError> {
        let selection = self.timeline_selection();
        let revision = self.editor.revision();
        if let Some(cache) = self.timeline_cache.borrow().as_ref() {
            if cache.revision == revision
                && cache.view == self.view
                && cache.bars == self.preview_bars
                && cache.start == self.preview_start
                && cache.selection == selection
                && cache.song == *self.editor.song()
            {
                return Ok(cache.timeline.clone());
            }
        }
        let timeline = self.build_timeline(&selection)?;
        *self.timeline_cache.borrow_mut() = Some(TimelineCache {
            song: self.editor.song().clone(),
            revision,
            selection,
            bars: self.preview_bars,
            start: self.preview_start,
            view: self.view,
            timeline: timeline.clone(),
        });
        Ok(timeline)
    }
    fn navigate_timeline(&mut self, horizontal: i32, vertical: i32) -> bool {
        let Ok(timeline) = self.timeline() else {
            return false;
        };
        if timeline.lanes.is_empty() || timeline.lanes.iter().all(|lane| lane.spans.is_empty()) {
            return false;
        }
        let occurrence = self.occurrence();
        let current = timeline
            .lanes
            .iter()
            .position(|lane| {
                lane.track == self.editor.song().tracks[self.track].id
                    && (lane.chain_use.is_none()
                        || lane.chain_use == occurrence.map(|path| path.chain_use))
            })
            .unwrap_or(0);
        let index = current
            .saturating_add_signed(horizontal as isize)
            .min(timeline.lanes.len() - 1);
        if self.view == View::Expanded {
            return self.navigate_expanded_rows(&timeline, index, vertical, horizontal != 0);
        }
        let lane = &timeline.lanes[index];
        let Some(first) = lane.spans.first() else {
            self.track = lane.index;
            self.slot = 0;
            self.chain_row = 0;
            self.row = 0;
            self.column = self.track;
            return true;
        };
        let focused_time = self.timeline_focus.unwrap_or(Beat::ZERO);
        let position = lane
            .spans
            .iter()
            .position(|span| span.start <= focused_time && focused_time < span.end)
            .or_else(|| {
                lane.spans
                    .iter()
                    .position(|span| Some(span.occurrence) == occurrence)
            })
            .unwrap_or(0);
        let position = position
            .saturating_add_signed(vertical as isize)
            .min(lane.spans.len() - 1);
        let span = lane.spans.get(position).unwrap_or(first);
        self.locate(
            SourceLocation {
                occurrence: span.occurrence,
                start: span.start,
                row: 0,
                row_at: None,
            },
            self.view,
        )
        .is_ok()
    }
    fn build_timeline(&self, selection: &[ChainSelection]) -> Result<Timeline, DocumentError> {
        let song = self.editor.song();
        let first = timeline_window::bar_at(song, self.preview_start)?;
        let start = first.at;
        let mut at = start;
        let mut bars = Vec::new();
        for offset in 0..self.preview_bars {
            let number = first
                .number
                .checked_add(offset)
                .ok_or(DocumentError::Invalid("bar number overflow"))?;
            let meter = song
                .meters
                .iter()
                .rev()
                .find(|change| change.at <= at)
                .expect("meter map starts at zero")
                .meter;
            bars.push(BarMarker { at, number, meter });
            at = at.checked_add(meter.bar_length())?;
        }
        let end = at;
        let mut lanes = Vec::new();
        if self.view == View::Expanded {
            for selected in selection {
                let Some((index, track)) = song
                    .tracks
                    .iter()
                    .enumerate()
                    .find(|(_, track)| track.id == selected.track)
                else {
                    continue;
                };
                let Some((slot, usage)) =
                    track.slots.iter().enumerate().find_map(|(slot, usage)| {
                        usage
                            .as_ref()
                            .filter(|usage| usage.id == selected.chain_use)
                            .map(|usage| (slot, usage))
                    })
                else {
                    continue;
                };
                let mut lane = empty_timeline_lane(track, index);
                lane.slot = Some(slot);
                lane.chain_use = Some(usage.id);
                lane.name = format!(
                    "{:02} · CHAIN {} / {}",
                    index + 1,
                    grid::definition_label(&song.chains, &usage.chain),
                    song.chains[&usage.chain].name
                );
                let mut at = Beat::ZERO;
                let mut details = 2048;
                self.append_timeline_chain(&mut lane, usage, &mut at, end, 0, &mut details)?;
                lanes.push(lane);
            }
        } else {
            for (index, track) in song.tracks.iter().enumerate() {
                let mut lane = empty_timeline_lane(track, index);
                let mut at = Beat::ZERO;
                let mut cycle = 0;
                if track.looped && track.slots.iter().all(Option::is_some) {
                    if let Some(length) = timeline_window::linear_track_length(song, track)? {
                        let skipped = start.units(length)?;
                        cycle = u32::try_from(skipped).map_err(|_| {
                            DocumentError::Invalid("preview cycle exceeds supported range")
                        })?;
                        at = length.checked_scale(skipped, 1)?;
                    }
                }
                let mut details = 1024;
                'cycles: loop {
                    let before = at;
                    for usage in &track.slots {
                        let Some(usage) = usage else {
                            break 'cycles;
                        };
                        if !self.append_timeline_chain(
                            &mut lane,
                            usage,
                            &mut at,
                            end,
                            cycle,
                            &mut details,
                        )? {
                            break 'cycles;
                        }
                    }
                    if !track.looped || at == before || at >= end {
                        break;
                    }
                    cycle += 1;
                }
                lanes.push(lane);
            }
        }
        Ok(Timeline {
            revision: self.editor.revision(),
            start,
            end,
            bars,
            lanes,
            bounded: true,
        })
    }
    fn append_timeline_chain(
        &self,
        lane: &mut TimelineLane,
        usage: &ChainUse,
        at: &mut Beat,
        end: Beat,
        cycle: u32,
        details: &mut usize,
    ) -> Result<bool, DocumentError> {
        let song = self.editor.song();
        let chain = &song.chains[&usage.chain];
        if chain.phrases.is_empty() {
            return Ok(true);
        }
        let start = timeline_window::bar_at(song, self.preview_start)?.at;
        let skipped = if let Some(length) = timeline_window::linear_chain_length(song, chain)? {
            timeline_window::skip_repeats(at, start, length, usage.repeats)?
        } else {
            0
        };
        for chain_repeat in skipped..usage.repeats {
            for phrase_use in &chain.phrases {
                let phrase = &song.phrases[&phrase_use.phrase];
                let source_events = &phrase.events[..phrase
                    .events
                    .len()
                    .min(timeline_window::PREVIEW_EVENT_LIMIT)];
                let source_limited = source_events.len() != phrase.events.len();
                let conditional = source_limited
                    || source_events.iter().any(|event| {
                        event
                            .fx
                            .iter()
                            .flatten()
                            .any(|fx| matches!(fx, Fx::Jump { .. }))
                    });
                let mut by_time = BTreeMap::new();
                if self.view == View::Expanded && *details > 0 {
                    for event in source_events {
                        by_time.entry(event.at).or_insert(event);
                    }
                }
                let skipped = if !conditional {
                    timeline_window::skip_repeats(at, start, phrase.length()?, phrase_use.repeats)?
                } else {
                    0
                };
                for repeat in skipped..phrase_use.repeats {
                    if *at >= end {
                        return Ok(false);
                    }
                    if lane.spans.len() >= 256 {
                        lane.notice =
                            Some("Preview limited to 256 phrase occurrences per lane".into());
                        return Ok(false);
                    }
                    let length = phrase.length()?;
                    let remaining = end.checked_sub(*at)?;
                    let source_end = at.checked_add(length.min(remaining))?;
                    if conditional {
                        lane.conditional_from.get_or_insert(*at);
                        lane.notice = Some(if source_limited {
                            "Source scan limited to 8192 events; subsequent timing may contain unseen Jump effects".into()
                        } else {
                            "Jump: timing after this phrase begins is conditional or unbounded; source structure shown".into()
                        });
                    }
                    if source_end <= start {
                        *at = source_end;
                        if conditional && self.view == View::Minimap {
                            return Ok(false);
                        }
                        continue;
                    }
                    let mut span = PhraseSpan {
                        start: *at,
                        end: source_end.min(end),
                        occurrence: Occurrence {
                            track: lane.track,
                            chain_use: usage.id,
                            phrase_use: phrase_use.id,
                        },
                        phrase: phrase_use.phrase,
                        name: phrase.name.clone(),
                        repeat,
                        chain_repeat,
                        cycle,
                        generated: matches!(
                            phrase.form,
                            PhraseForm::Euclidean { .. } | PhraseForm::Random { .. }
                        ),
                        conditional: lane.conditional_from.is_some(),
                        rows: Vec::new(),
                        activity: Vec::new(),
                    };
                    if self.view == View::Expanded {
                        let mut rows = BTreeMap::new();
                        let source_start = start.checked_sub(*at).unwrap_or(Beat::ZERO);
                        let first_row = source_start
                            .units(phrase.division.beat_length())?
                            .min(u64::from(phrase.steps));
                        for row in first_row..u64::from(phrase.steps) {
                            let source_at = phrase.division.beat_length().checked_scale(row, 1)?;
                            if source_at >= remaining {
                                break;
                            }
                            if rows.len() >= *details {
                                break;
                            }
                            rows.insert(source_at, row as usize);
                        }
                        for event in source_events {
                            if rows.len() >= *details {
                                break;
                            }
                            if event.at >= source_start && event.at < remaining {
                                rows.insert(
                                    event.at,
                                    event.at.units(phrase.division.beat_length())? as usize,
                                );
                            }
                        }
                        for (source_at, source_row) in rows {
                            let cells = by_time
                                .get(&source_at)
                                .map(|event| self.event_cells(phrase, event))
                                .unwrap_or_else(|| vec!["—".into(); 6]);
                            span.rows.push(TimelineRow {
                                at: at.checked_add(source_at)?,
                                source_row,
                                cells,
                            });
                        }
                        *details -= span.rows.len();
                    } else {
                        for event in source_events {
                            if *details == 0 {
                                break;
                            }
                            if let Some(note) = &event.note {
                                if event.at >= remaining {
                                    continue;
                                }
                                let onset = at.checked_add(event.at)?;
                                if onset < start {
                                    continue;
                                }
                                span.activity.push(Activity {
                                    at: onset,
                                    end: onset
                                        .checked_add(note.duration.min(end.checked_sub(onset)?))?,
                                    pitch: note.pitch,
                                });
                                *details -= 1;
                            }
                        }
                    }
                    lane.spans.push(span);
                    *at = source_end;
                    if conditional && self.view == View::Minimap {
                        return Ok(false);
                    }
                    if *details == 0 {
                        lane.notice.get_or_insert(
                            "Activity detail limit reached; open a source phrase for all events"
                                .into(),
                        );
                    }
                }
            }
        }
        Ok(true)
    }
    fn generator_preview(&self) -> Value {
        let Some((phrase_id, phrase)) = self.selected_phrase() else {
            return Value::Null;
        };
        if !matches!(
            phrase.form,
            PhraseForm::Euclidean { .. } | PhraseForm::Random { .. }
        ) {
            return Value::Null;
        }
        let Some(occurrence) = self.occurrence() else {
            return Value::Null;
        };
        let revision = self.editor.revision();
        let key = &self.editor.song().key;
        if let Some(cache) = self.preview_cache.borrow().as_ref() {
            if cache.revision == revision
                && cache.occurrence == occurrence
                && cache.phrase == *phrase
                && cache.key == *key
            {
                return cache.value.clone();
            }
        }
        let value = self.compile_generator_preview(phrase_id, phrase);
        let notes = preview_notes(&value, phrase);
        *self.preview_cache.borrow_mut() = Some(PreviewCache {
            revision,
            occurrence,
            phrase: phrase.clone(),
            key: key.clone(),
            value: value.clone(),
            notes,
        });
        value
    }
    fn compile_generator_preview(&self, phrase_id: PhraseId, phrase: &Phrase) -> Value {
        let song = self.isolated_phrase_song(phrase_id, phrase);
        let result = (|| {
            let length = phrase.length()?;
            let snapshot =
                Snapshot::compile(&song, self.editor.revision(), PlaybackBudget::default())?;
            let events = Playback::new(Arc::new(snapshot)).window(length)?;
            Ok::<_, crest_tracker::playback::PlaybackError>(
                json!({"events":events,"length":length,"revision":self.editor.revision()}),
            )
        })();
        result.unwrap_or_else(|error| json!({"error":error.to_string()}))
    }
    fn isolated_phrase_song(&self, phrase_id: PhraseId, phrase: &Phrase) -> Song {
        let mut song = self.editor.song().clone();
        let mut track = song.tracks[self.track].clone();
        let mut chain_use = track.slots[self.slot]
            .as_ref()
            .expect("selected occurrence has a chain")
            .clone();
        let mut chain = song.chains[&chain_use.chain].clone();
        let mut phrase_use = chain.phrases[self.chain_row].clone();
        phrase_use.repeats = 1;
        chain.phrases = vec![phrase_use];
        chain_use.repeats = 1;
        let chain_id = chain_use.chain;
        track.slots = vec![Some(chain_use)];
        track.looped = false;
        track.muted = false;
        song.tracks = vec![track];
        song.chains = BTreeMap::from([(chain_id, chain)]);
        song.phrases = BTreeMap::from([(phrase_id, phrase.clone())]);
        song
    }
    pub(crate) fn checked_revision(&self, revision: u64) -> Result<(), DocumentError> {
        if revision != self.editor.revision() {
            return Err(DocumentError::Stale {
                expected: revision,
                actual: self.editor.revision(),
            });
        }
        Ok(())
    }
    pub(crate) fn default_chords(&self) -> Result<Vec<ChordRevision>, ChordBankError> {
        crest_tracker::chord_bank::shared_catalog()
    }
    #[cfg(test)]
    fn chord_bank_projection(&self) -> Value {
        let scale = self
            .selected_phrase()
            .and_then(|(_, phrase)| phrase.key.as_ref())
            .unwrap_or(&self.editor.song().key);
        match self.default_chords() {
            Ok(defaults) => {
                json!({"scale":scale.name(),"defaults":defaults,"custom":self.editor.song().chords})
            }
            Err(error) => {
                json!({"scale":scale.name(),"defaults":[],"custom":self.editor.song().chords,"error":error.to_string()})
            }
        }
    }
    fn save_chord(&mut self, revision: u64, draft: ChordDraft) -> Result<(), DocumentError> {
        self.checked_revision(revision)?;
        let chord = draft.verified()?;
        let id = ChordRevisionId::new(self.editor.song().next_identity()?)?;
        let name = chord.name().to_owned();
        self.command(Command::PutChord {
            id,
            revision: chord,
        })?;
        self.status = format!("CHORD SAVED · {name} · revision {id}");
        Ok(())
    }
    pub(crate) fn chosen_chord(
        &self,
        source: ChordSource,
    ) -> Result<(ChordRevisionId, Option<ChordRevision>), DocumentError> {
        match source {
            ChordSource::Library { digest } => {
                let entry = self.chord_library["entries"]
                    .as_array()
                    .and_then(|entries| entries.iter().find(|entry| entry["digest"] == digest))
                    .ok_or(DocumentError::Invalid("shared chord revision unavailable"))?;
                let chord: ChordRevision = serde_json::from_value(entry["chord"].clone())
                    .map_err(|_| DocumentError::Invalid("invalid shared chord revision"))?;
                if let Some((id, _)) = self
                    .editor
                    .song()
                    .chords
                    .entries()
                    .find(|(_, saved)| **saved == chord)
                {
                    return Ok((*id, None));
                }
                Ok((
                    ChordRevisionId::new(self.editor.song().next_identity()?)?,
                    Some(chord),
                ))
            }
            ChordSource::Draft { draft } => Ok((
                ChordRevisionId::new(self.editor.song().next_identity()?)?,
                Some(draft.verified()?),
            )),
            ChordSource::Saved { revision } => {
                self.editor
                    .song()
                    .chords
                    .get(&revision)
                    .ok_or(DocumentError::Missing {
                        kind: "chord revision",
                        id: revision.get(),
                    })?;
                Ok((revision, None))
            }
            ChordSource::Default { index } => {
                let chord = self
                    .default_chords()?
                    .get(index)
                    .cloned()
                    .ok_or(DocumentError::Invalid("default chord no longer exists"))?;
                if let Some((id, _)) = self
                    .editor
                    .song()
                    .chords
                    .entries()
                    .find(|(_, saved)| **saved == chord)
                {
                    return Ok((*id, None));
                }
                Ok((
                    ChordRevisionId::new(self.editor.song().next_identity()?)?,
                    Some(chord),
                ))
            }
        }
    }
    fn cycle_chord(
        &mut self,
        event: EventId,
        root: u8,
        current: ChordRevisionId,
        delta: i32,
    ) -> Result<(), DocumentError> {
        let defaults = self.default_chords()?;
        let mut choices: Vec<_> = defaults
            .iter()
            .enumerate()
            .map(|(index, chord)| (ChordSource::Default { index }, chord.clone()))
            .collect();
        for (id, chord) in self.editor.song().chords.entries() {
            if !defaults.contains(chord) {
                choices.push((ChordSource::Saved { revision: *id }, chord.clone()));
            }
        }
        if choices.is_empty() {
            return Ok(());
        }
        let current = self
            .editor
            .song()
            .chords
            .get(&current)
            .ok_or(DocumentError::Invalid("selected chord revision is missing"))?;
        let index = choices
            .iter()
            .position(|(_, chord)| chord == current)
            .unwrap_or(0);
        let next = (index as i64 + i64::from(delta)).rem_euclid(choices.len() as i64) as usize;
        self.apply_chord(
            self.editor.revision(),
            self.occurrence()
                .ok_or(DocumentError::Invalid("select a chord occurrence"))?,
            event,
            (self.column - 3) as u8,
            root,
            choices[next].0.clone(),
        )
    }
    fn apply_chord(
        &mut self,
        revision: u64,
        occurrence: Occurrence,
        event_id: EventId,
        slot: u8,
        root: u8,
        source: ChordSource,
    ) -> Result<(), DocumentError> {
        let phrase = self.checked_source(revision, occurrence)?;
        if slot >= 3 {
            return Err(DocumentError::Invalid(
                "chord must use one of the three FX slots",
            ));
        }
        let root = Pitch::new(root).map_err(ChordBankError::from)?.midi();
        let mut event = self.editor.song().phrases[&phrase]
            .events
            .iter()
            .find(|event| event.id == event_id)
            .cloned()
            .ok_or(DocumentError::Invalid("selected note no longer exists"))?;
        let note = event.note.as_mut().ok_or(DocumentError::Invalid(
            "choose a note event to apply a chord",
        ))?;
        note.pitch = root;
        for point in &mut note.expression {
            if let Control::PolyPressure { pitch, .. } = &mut point.control {
                *pitch = root;
            }
        }
        let (id, stored) = self.chosen_chord(source)?;
        event.fx[usize::from(slot)] = Some(Fx::ChordRevision { revision: id });
        // Validate the actual note plus preserved FX with the playback owner. A
        // saved snapshot at another root cannot prove this combination is legal.
        let mut probe_phrase = self.editor.song().phrases[&phrase].clone();
        probe_phrase.form = PhraseForm::Tracker;
        probe_phrase.events = vec![event.clone()];
        let mut probe = self.isolated_phrase_song(phrase, &probe_phrase);
        if let Some(chord) = &stored {
            probe.chords = probe.chords.with_revision(id, chord.clone())?;
        }
        Snapshot::compile(&probe, revision, PlaybackBudget::default())
            .map_err(chord_playback_error)?;
        let mut commands = Vec::new();
        if let Some(chord) = stored {
            commands.push(Command::PutChord {
                id,
                revision: chord,
            });
        }
        commands.push(Command::PutEvent { phrase, event });
        self.command(Command::Batch { commands })?;
        self.status = format!("CHORD APPLIED · FX {}", slot + 1);
        Ok(())
    }
    fn checked_source(
        &self,
        revision: u64,
        occurrence: Occurrence,
    ) -> Result<PhraseId, DocumentError> {
        self.checked_revision(revision)?;
        if self.occurrence() != Some(occurrence) {
            return Err(DocumentError::Invalid(
                "phrase selection changed; reopen the source editor",
            ));
        }
        self.selected_phrase()
            .map(|(id, _)| id)
            .ok_or(DocumentError::Invalid("select a phrase first"))
    }
    fn bake_generator(
        &mut self,
        revision: u64,
        occurrence: Occurrence,
        form: BakeForm,
    ) -> Result<(), DocumentError> {
        let source_id = self.checked_source(revision, occurrence)?;
        let source = &self.editor.song().phrases[&source_id];
        if !matches!(
            source.form,
            PhraseForm::Euclidean { .. } | PhraseForm::Random { .. }
        ) {
            return Err(DocumentError::Invalid(
                "only generator phrases can be baked",
            ));
        }
        if source
            .events
            .iter()
            .flat_map(|event| event.fx.iter().flatten())
            .any(|fx| matches!(fx, Fx::Jump { .. } | Fx::Send { .. } | Fx::Tempo { .. }))
        {
            return Err(DocumentError::Invalid(
                "bake does not support Jump, Send, or Tempo effects; remove them before baking",
            ));
        }
        let song = self.isolated_phrase_song(source_id, source);
        let snapshot = Snapshot::compile(&song, revision, PlaybackBudget::default())
            .map_err(bake_playback_error)?;
        let mut playback = Playback::new(Arc::new(snapshot));
        // The isolated phrase has one occurrence and no Jump or loop. One bounded
        // scheduler pass therefore resolves the complete release/expression tail.
        let resolved = playback
            .window(Beat::integer(u64::MAX))
            .map_err(bake_playback_error)?;
        if !playback.finished() {
            return Err(DocumentError::Invalid(
                "bake cannot represent the complete performance tail",
            ));
        }
        let mut candidate = Editor::new(self.editor.song().clone())?;
        candidate.apply(0, Command::MakeUnique { occurrence })?;
        let chain = candidate.song().tracks[self.track].slots[self.slot]
            .as_ref()
            .expect("source occurrence exists")
            .chain;
        let id = candidate.song().chains[&chain].phrases[self.chain_row].phrase;
        let mut next = candidate.song().next_identity()?;
        let mut phrase = source.clone();
        phrase.name.push_str(" baked");
        phrase.events = materialize_performance(&resolved, source.length()?, &mut next)?;
        phrase.form = match form {
            BakeForm::Tracker => PhraseForm::Tracker,
            BakeForm::PianoRoll => PhraseForm::PianoRoll,
        };
        self.command(Command::Batch {
            commands: vec![
                Command::MakeUnique { occurrence },
                Command::PutPhrase { id, phrase },
            ],
        })?;
        self.open_view(View::Phrase);
        self.status = "BAKED · Exact first cycle saved; original generator recipe retained".into();
        Ok(())
    }
    fn drum_toggle(
        &mut self,
        revision: u64,
        occurrence: Occurrence,
        step: u32,
        draft: DrumNote,
    ) -> Result<(), DocumentError> {
        let id = self.checked_source(revision, occurrence)?;
        self.editor.song().validate_target(&draft.target)?;
        let note = Note::new(draft.pitch, draft.velocity, draft.duration)?;
        let mut phrase = self.editor.song().phrases[&id].clone();
        if !matches!(phrase.form, PhraseForm::Drum) || step >= phrase.steps {
            return Err(DocumentError::Invalid("select a valid Drum phrase step"));
        }
        let at = phrase
            .division
            .beat_length()
            .checked_scale(u64::from(step), 1)?;
        let matches = |event: &Event| {
            event.at == at
                && event.target == draft.target
                && event
                    .note
                    .as_ref()
                    .is_some_and(|note| note.pitch == draft.pitch)
        };
        if phrase.events.iter().any(matches) {
            phrase.events.retain_mut(|event| {
                if !matches(event) {
                    return true;
                }
                event.note = None;
                !event.controls.is_empty() || event.fx.iter().any(Option::is_some)
            });
        } else {
            phrase.events.push(Event::note(
                EventId::new(self.editor.song().next_identity()?)?,
                at,
                draft.target.clone(),
                note,
            ));
        }
        self.command(Command::PutPhrase { id, phrase })?;
        self.row = step as usize;
        self.remember_drum_lane(None, draft);
        Ok(())
    }
    fn drum_lane(
        &mut self,
        revision: u64,
        occurrence: Occurrence,
        from_pitch: u8,
        from_target: Target,
        draft: DrumNote,
    ) -> Result<(), DocumentError> {
        let id = self.checked_source(revision, occurrence)?;
        self.editor.song().validate_target(&draft.target)?;
        Note::new(draft.pitch, draft.velocity, draft.duration)?;
        let mut phrase = self.editor.song().phrases[&id].clone();
        if !matches!(phrase.form, PhraseForm::Drum) {
            return Err(DocumentError::Invalid(
                "lane settings require a Drum phrase",
            ));
        }
        let mut changed = false;
        for event in &mut phrase.events {
            if event.target != from_target {
                continue;
            }
            let Some(note) = &mut event.note else {
                continue;
            };
            if note.pitch != from_pitch {
                continue;
            }
            note.pitch = draft.pitch;
            note.velocity = draft.velocity;
            note.duration = draft.duration;
            event.target = draft.target.clone();
            changed = true;
        }
        if changed {
            self.command(Command::PutPhrase { id, phrase })?;
        }
        self.remember_drum_lane(Some((from_pitch, from_target)), draft);
        Ok(())
    }
    pub(crate) fn selected_event_ids(&self) -> Vec<EventId> {
        let Some((id, phrase)) = self.selected_phrase() else {
            return Vec::new();
        };
        self.event_selection
            .as_ref()
            .filter(|selection| selection.phrase == id)
            .map(|selection| {
                selection
                    .events
                    .iter()
                    .copied()
                    .filter(|id| phrase.events.iter().any(|event| event.id == *id))
                    .collect()
            })
            .unwrap_or_default()
    }
    fn select_range(
        &mut self,
        revision: u64,
        occurrence: Occurrence,
        range: EventRange,
    ) -> Result<(), DocumentError> {
        let id = self.checked_source(revision, occurrence)?;
        let phrase = &self.editor.song().phrases[&id];
        if range.start >= range.end
            || range.end > phrase.length()?
            || range.low > range.high
            || range.high > 127
        {
            return Err(DocumentError::Invalid(
                "range requires valid time and pitch bounds inside the phrase",
            ));
        }
        if let Some(target) = &range.target {
            self.editor.song().validate_target(target)?;
        }
        let events = phrase
            .events
            .iter()
            .filter(|event| {
                event.at >= range.start
                    && event.at < range.end
                    && range
                        .target
                        .as_ref()
                        .is_none_or(|target| target == &event.target)
                    && event.note.as_ref().map_or(range.include_controls, |note| {
                        (range.low..=range.high).contains(&note.pitch)
                    })
            })
            .map(|event| event.id)
            .collect();
        self.select_events(revision, occurrence, events, false)
    }
    fn select_events(
        &mut self,
        revision: u64,
        occurrence: Occurrence,
        mut events: Vec<EventId>,
        toggle: bool,
    ) -> Result<(), DocumentError> {
        let phrase = self.checked_source(revision, occurrence)?;
        if events.iter().any(|id| {
            !self.editor.song().phrases[&phrase]
                .events
                .iter()
                .any(|event| event.id == *id)
        }) {
            return Err(DocumentError::Invalid("selected event no longer exists"));
        }
        events.sort_unstable();
        events.dedup();
        if toggle {
            let mut selected = self.selected_event_ids();
            for id in events {
                if let Some(index) = selected.iter().position(|current| *current == id) {
                    selected.remove(index);
                } else {
                    selected.push(id);
                }
            }
            events = selected;
        }
        if let Some(first) = events.first().and_then(|id| {
            self.editor.song().phrases[&phrase]
                .events
                .iter()
                .find(|event| event.id == *id)
        }) {
            self.roll_cursor = first.at;
            if let Some(note) = &first.note {
                self.roll_pitch = note.pitch;
            }
            self.row = first
                .at
                .units(self.editor.song().phrases[&phrase].division.beat_length())?
                as usize;
            self.column = 0;
        }
        self.status = format!("SELECTED · {} events", events.len());
        self.event_selection = Some(EventSelection { phrase, events });
        Ok(())
    }
    fn edit_selection(
        &mut self,
        revision: u64,
        occurrence: Occurrence,
        action: SelectionAction,
    ) -> Result<(), DocumentError> {
        let phrase = self.checked_source(revision, occurrence)?;
        if let SelectionAction::Paste { at } = action {
            if self.clipboard.is_empty() {
                return Err(DocumentError::Invalid("copy events before pasting"));
            }
            let mut next = self.editor.song().next_identity()?;
            let mut ids = Vec::new();
            let mut commands = Vec::new();
            for copied in &self.clipboard {
                let mut event = copied.clone();
                event.id = allocate_event_id(&mut next)?;
                event.at = at.checked_add(event.at)?;
                ids.push(event.id);
                commands.push(Command::PutEvent { phrase, event });
            }
            self.command(Command::Batch { commands })?;
            self.event_selection = Some(EventSelection {
                phrase,
                events: ids,
            });
            self.status = format!("PASTED · {} events", self.clipboard.len());
            return Ok(());
        }
        let ids = self.selected_event_ids();
        if ids.is_empty() {
            return Err(DocumentError::Invalid("select source events first"));
        }
        let mut events: Vec<_> = self.editor.song().phrases[&phrase]
            .events
            .iter()
            .filter(|event| ids.contains(&event.id))
            .cloned()
            .collect();
        match action {
            SelectionAction::Duplicate { at } => {
                let anchor = events
                    .iter()
                    .map(|event| event.at)
                    .min()
                    .expect("nonempty selection");
                let mut next = self.editor.song().next_identity()?;
                let mut commands = Vec::new();
                for mut event in events {
                    event.id = allocate_event_id(&mut next)?;
                    event.at = at.checked_add(event.at.checked_sub(anchor)?)?;
                    commands.push(Command::PutEvent { phrase, event });
                }
                self.command(Command::Batch { commands })?;
                self.status = "DUPLICATED · Owned expression retained; clipboard unchanged".into();
                return Ok(());
            }
            SelectionAction::Copy | SelectionAction::Cut => {
                let anchor = events
                    .iter()
                    .map(|event| event.at)
                    .min()
                    .expect("selection is nonempty");
                for event in &mut events {
                    event.at = event.at.checked_sub(anchor)?;
                }
                if matches!(action, SelectionAction::Cut) {
                    // Preserve copied data while shared-edit consent is pending.
                    self.clipboard = events.clone();
                    self.command(Command::DeleteEvents {
                        phrase,
                        events: ids,
                    })?;
                }
                self.clipboard = events;
                self.status = format!(
                    "COPIED · {} events with relative timing",
                    self.clipboard.len()
                );
                return Ok(());
            }
            SelectionAction::Delete => {
                return self.command(Command::DeleteEvents {
                    phrase,
                    events: ids,
                })
            }
            SelectionAction::Quantize { grid, durations } => {
                return self.command(Command::Quantize {
                    phrase,
                    events: ids,
                    grid,
                    durations,
                });
            }
            SelectionAction::Move { amount, backward } => {
                for event in &mut events {
                    event.at = if backward {
                        event.at.checked_sub(amount)?
                    } else {
                        event.at.checked_add(amount)?
                    };
                }
            }
            SelectionAction::Transpose { semitones } => {
                for event in &mut events {
                    let Some(note) = &mut event.note else {
                        continue;
                    };
                    let pitch = i32::from(note.pitch) + i32::from(semitones);
                    if !(0..=127).contains(&pitch) {
                        return Err(DocumentError::Invalid(
                            "transposed selection would leave MIDI pitch range",
                        ));
                    }
                    note.pitch = pitch as u8;
                    for point in &mut note.expression {
                        if let Control::PolyPressure { pitch, .. } = &mut point.control {
                            *pitch = note.pitch;
                        }
                    }
                }
            }
            SelectionAction::Duration { duration } => {
                if duration.is_zero() {
                    return Err(DocumentError::Invalid("note duration must be positive"));
                }
                for event in &mut events {
                    if let Some(note) = &mut event.note {
                        note.resize(duration)?;
                    }
                }
            }
            SelectionAction::Paste { .. } => unreachable!("paste handled before source selection"),
        }
        self.command(Command::Batch {
            commands: events
                .into_iter()
                .map(|event| Command::PutEvent { phrase, event })
                .collect(),
        })
    }
}
fn chord_playback_error(error: PlaybackError) -> DocumentError {
    match error {
        PlaybackError::Document(error) => error,
        PlaybackError::Time(error) => error.into(),
        PlaybackError::Harmony(error) => ChordBankError::from(error).into(),
        PlaybackError::Expression(message) => DocumentError::Invalid(message),
        _ => DocumentError::Invalid("chord and preserved FX exceed playback admission limits"),
    }
}
fn bake_playback_error(error: PlaybackError) -> DocumentError {
    match error {
        PlaybackError::Document(error) => error,
        PlaybackError::Time(error) => error.into(),
        PlaybackError::Expression(message) => DocumentError::Invalid(message),
        PlaybackError::Budget(_) => {
            DocumentError::Invalid("generator bake exceeds the bounded preview budget")
        }
        _ => DocumentError::Invalid("generator bake could not resolve the source performance"),
    }
}

fn materialize_performance(
    resolved: &[TimedEvent],
    length: Beat,
    next: &mut u64,
) -> Result<Vec<Event>, DocumentError> {
    let mut events: Vec<Event> = Vec::new();
    let mut active: BTreeMap<Owner, VecDeque<usize>> = BTreeMap::new();
    let mut owners: BTreeMap<Owner, Vec<usize>> = BTreeMap::new();
    for event in resolved {
        match event.message {
            Message::NoteOn { pitch, velocity } => {
                if event.at >= length {
                    return Err(DocumentError::Invalid("resolved note starts outside the phrase; bake would change arrangement timing"));
                }
                let index = events.len();
                let id = allocate_event_id(next)?;
                events.push(Event::note(
                    id,
                    event.at,
                    event.target.clone(),
                    Note::new(pitch, velocity, Beat::ONE)?,
                ));
                active
                    .entry(event.owner.clone())
                    .or_default()
                    .push_back(index);
                owners.entry(event.owner.clone()).or_default().push(index);
            }
            Message::NoteOff { pitch, velocity } => {
                let index = active
                    .get_mut(&event.owner)
                    .and_then(VecDeque::pop_front)
                    .ok_or(DocumentError::Invalid(
                        "resolved note release has no matching attack",
                    ))?;
                let captured = &mut events[index];
                let note = captured.note.as_mut().expect("attack creates a note");
                if captured.target != event.target || note.pitch != pitch {
                    return Err(DocumentError::Invalid(
                        "resolved note release target differs from its attack",
                    ));
                }
                note.duration = event.at.checked_sub(captured.at)?;
                note.release_velocity = velocity;
                note.validate()?;
            }
            Message::Control { control } => {
                if event.at >= length {
                    return Err(DocumentError::Invalid(
                        "resolved channel control lies outside the phrase",
                    ));
                }
                events.push(Event {
                    id: allocate_event_id(next)?,
                    at: event.at,
                    target: event.target.clone(),
                    note: None,
                    controls: vec![control],
                    fx: [None, None, None],
                });
            }
            Message::Expression { .. } => {}
            Message::MasterExpression { .. }
            | Message::ExpressionLifetime { .. }
            | Message::EndExpression
            | Message::Send { .. }
            | Message::EndSend { .. }
            | Message::Tempo { .. } => {
                return Err(DocumentError::Invalid(
                    "bake cannot discard Send or Tempo side effects",
                ));
            }
        }
    }
    if active.values().any(|notes| !notes.is_empty()) {
        return Err(DocumentError::Invalid(
            "resolved performance contains an unreleased note",
        ));
    }
    for event in resolved {
        let Message::Expression { control } = event.message else {
            continue;
        };
        let matching: Vec<_> = owners
            .get(&event.owner)
            .into_iter()
            .flatten()
            .copied()
            .filter(|index| {
                let note = &events[*index];
                note.target == event.target
                    && note.at <= event.at
                    && note
                        .at
                        .checked_add(note.note.as_ref().expect("owned attack").duration)
                        .is_ok_and(|end| event.at <= end)
            })
            .collect();
        if matching.len() != 1 {
            return Err(DocumentError::Invalid(
                "resolved expression has ambiguous note ownership",
            ));
        }
        let note = &mut events[matching[0]];
        let offset = event.at.checked_sub(note.at)?;
        note.note
            .as_mut()
            .expect("owned attack")
            .expression
            .push(Expression {
                sequence: None,
                offset,
                control,
            });
    }
    events.sort_by_key(|event| (event.at, event.id));
    Ok(events)
}

fn allocate_event_id(next: &mut u64) -> Result<EventId, DocumentError> {
    let id = EventId::new(*next)?;
    *next = fresh_identity(*next, 1)?;
    Ok(id)
}

fn empty_timeline_lane(track: &Track, index: usize) -> TimelineLane {
    TimelineLane {
        track: track.id,
        index,
        name: track.name.clone(),
        slot: None,
        chain_use: None,
        muted: track.muted,
        looped: track.looped,
        spans: Vec::new(),
        notice: None,
        conditional_from: None,
    }
}
fn preview_notes(value: &Value, phrase: &Phrase) -> BTreeMap<u64, (u8, u8, Target)> {
    let mut notes = BTreeMap::new();
    let events: Vec<TimedEvent> =
        serde_json::from_value(value["events"].clone()).unwrap_or_default();
    for event in events {
        if let Message::NoteOn { pitch, velocity } = event.message {
            if let Ok(row) = event.at.units(phrase.division.beat_length()) {
                notes.entry(row).or_insert((pitch, velocity, event.target));
            }
        }
    }
    notes
}
fn fresh_identity(first: u64, offset: u64) -> Result<u64, DocumentError> {
    first
        .checked_add(offset)
        .ok_or(DocumentError::Invalid("identity range exhausted"))
}
fn fx_label(fx: &Option<Fx>, chords: &ChordBank) -> String {
    match fx {
        None => "—".into(),
        Some(Fx::Offset { semitones }) => format!("OFS {semitones:+}"),
        Some(Fx::Chance { percent }) => format!("CHA {percent}%"),
        Some(Fx::ChordRevision { revision }) => chords
            .get(revision)
            .map(crate::workspace::chords::compact_name)
            .unwrap_or_else(|| "CRD MISSING".into()),
        Some(Fx::Chord { intervals }) => format!(
            "CRD {}",
            intervals
                .iter()
                .map(i8::to_string)
                .collect::<Vec<_>>()
                .join("/")
        ),
        Some(value) => serde_json::to_value(value)
            .ok()
            .and_then(|v| v.get("kind").and_then(Value::as_str).map(str::to_uppercase))
            .unwrap_or_default(),
    }
}

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;

#[cfg(test)]
mod timeline_tests {
    use super::*;
    use crest_tracker::time::{Division, Meter};

    fn two_lanes() -> App {
        let mut app = App::new();
        app.command(Command::PutInstrument {
            id: InstrumentId::new(17).unwrap(),
            instrument: InstrumentUse {
                name: "Lead".into(),
                resource: None,
            },
        })
        .unwrap();
        for index in 0..2 {
            app.command(Command::AddPhrase {
                track: app.editor.song().tracks[index].id,
                slot: 0,
            })
            .unwrap();
            let usage = app.editor.song().tracks[index].slots[0]
                .as_ref()
                .unwrap()
                .clone();
            let id = app.editor.song().chains[&usage.chain].phrases[0].phrase;
            let mut phrase = app.editor.song().phrases[&id].clone();
            phrase.steps = if index == 0 { 3 } else { 6 };
            phrase.division = Division::new(
                8,
                if index == 0 { 3 } else { 1 },
                if index == 0 { 2 } else { 1 },
            )
            .unwrap();
            app.command(Command::PutPhrase { id, phrase }).unwrap();
            app.apply(Request::ToggleExpanded {
                track: app.editor.song().tracks[index].id,
                chain_use: usage.id,
            })
            .unwrap();
        }
        app
    }

    #[test]
    fn arrangement_views_keep_exact_musical_time_and_never_edit_source_data() {
        let mut app = two_lanes();
        let before = app.editor.song().clone();
        let revision = app.editor.revision();
        app.apply(Request::View {
            view: View::Expanded,
        })
        .unwrap();
        let expanded = app.timeline().unwrap();
        assert_eq!(expanded.lanes.len(), 2);
        assert_eq!(
            expanded.lanes[0].spans[0].rows[1].at,
            Beat::new(1, 3).unwrap()
        );
        assert_eq!(
            expanded.lanes[1].spans[0].rows[1].at,
            Beat::new(1, 2).unwrap()
        );
        assert_eq!(expanded.lanes[0].spans[0].end, Beat::ONE);
        assert_eq!(expanded.lanes[1].spans[0].end, Beat::integer(3));
        let selected = &expanded.lanes[1].spans[0];
        app.apply(Request::Locate {
            row_at: None,
            occurrence: selected.occurrence,
            at: selected.start,
            row: 4,
            view: View::Expanded,
        })
        .unwrap();
        app.apply(Request::Confirm).unwrap();
        assert_eq!(app.view, View::Phrase);
        assert_eq!(app.occurrence(), Some(selected.occurrence));
        assert_eq!(app.row, 4);
        app.apply(Request::View {
            view: View::Minimap,
        })
        .unwrap();
        assert_eq!(app.timeline().unwrap().lanes.len(), 16);
        assert_eq!(app.editor.song(), &before);
        assert_eq!(app.editor.revision(), revision);
    }

    #[test]
    fn expanded_navigation_visits_rows_and_crosses_repeat_boundaries() {
        let mut app = two_lanes();
        let mut song = app.editor.song().clone();
        song.tracks[0].slots[0].as_mut().unwrap().repeats = 2;
        app.editor = Editor::new(song.clone()).unwrap();
        app.apply(Request::View {
            view: View::Expanded,
        })
        .unwrap();
        let timeline = app.timeline().unwrap();
        let span = &timeline.lanes[0].spans[0];
        app.locate(
            SourceLocation {
                occurrence: span.occurrence,
                start: span.start,
                row: 0,
                row_at: None,
            },
            View::Expanded,
        )
        .unwrap();
        let revision = app.editor.revision();
        for row in [1, 2, 0] {
            app.apply(Request::Move {
                horizontal: 0,
                vertical: 1,
            })
            .unwrap();
            assert_eq!(app.timeline_row, row);
        }
        assert_eq!(app.timeline_focus, Some(Beat::ONE));
        app.apply(Request::Move {
            horizontal: 0,
            vertical: -1,
        })
        .unwrap();
        assert_eq!(app.timeline_row, 2);
        assert_eq!(app.timeline_focus, Some(Beat::ZERO));
        app.apply(Request::Confirm).unwrap();
        assert_eq!(app.view, View::Phrase);
        assert_eq!(app.row, 2);
        assert_eq!(app.editor.song(), &song);
        assert_eq!(app.editor.revision(), revision);
    }

    #[test]
    fn expanded_source_open_preserves_exact_offgrid_time_in_repeated_piano_phrase() {
        let mut app = two_lanes();
        app.apply(Request::View {
            view: View::Expanded,
        })
        .unwrap();
        let timeline = app.timeline().unwrap();
        let first = &timeline.lanes[0].spans[0];
        let (phrase, occurrence) = (first.phrase, first.occurrence);
        let mut song = app.editor.song().clone();
        let event = Event::note(
            EventId::new(song.next_identity().unwrap()).unwrap(),
            Beat::new(1, 12).unwrap(),
            Target::instrument(InstrumentId::new(17).unwrap()),
            Note::new(67, 88, Beat::new(1, 8).unwrap()).unwrap(),
        );
        song.phrases.get_mut(&phrase).unwrap().form = PhraseForm::PianoRoll;
        song.phrases.get_mut(&phrase).unwrap().events.push(event);
        song.tracks[0].slots[0].as_mut().unwrap().repeats = 2;
        app.replace_song(song.clone()).unwrap();
        let revision = app.editor.revision();
        for pointer in [false, true] {
            app.apply(Request::View {
                view: View::Expanded,
            })
            .unwrap();
            app.apply(Request::Locate {
                occurrence,
                at: Beat::ONE,
                row: 0,
                row_at: Some(Beat::new(13, 12).unwrap()),
                view: View::Expanded,
            })
            .unwrap();
            app.roll_cursor = Beat::new(2, 3).unwrap();
            app.roll_pitch = 60;
            if pointer {
                app.apply(Request::Locate {
                    occurrence,
                    at: Beat::ONE,
                    row: 0,
                    row_at: Some(Beat::new(13, 12).unwrap()),
                    view: View::Phrase,
                })
                .unwrap();
            } else {
                app.apply(Request::Confirm).unwrap();
            }
            assert_eq!(app.view, View::Phrase);
            assert_eq!(app.roll_cursor, Beat::new(1, 12).unwrap());
            assert_eq!(app.roll_pitch, 67);
            assert_eq!(app.row, 0);
            assert_eq!(app.editor.song(), &song);
            assert_eq!(app.editor.revision(), revision);
        }
    }

    #[test]
    fn saved_view_group_recall_keeps_missing_sources_visible_without_editing_song() {
        let mut app = two_lanes();
        let selections = app.expanded_selection.clone();
        app.command(Command::PutViewGroup {
            name: "Verse".into(),
            selections: selections.clone(),
        })
        .unwrap();
        let mut track = app.editor.song().tracks[0].clone();
        track.slots[0] = None;
        app.command(Command::PutTrack { track }).unwrap();
        app.apply(Request::ClearExpanded).unwrap();
        let before = app.editor.song().clone();
        let revision = app.editor.revision();
        app.apply(Request::RecallViewGroup {
            name: "Verse".into(),
        })
        .unwrap();
        assert_eq!(app.view, View::Expanded);
        assert_eq!(app.expanded_selection, selections);
        assert_eq!(app.timeline().unwrap().lanes.len(), 1);
        let projection = app.projection();
        assert_eq!(projection["expandedMissing"], 1);
        assert_eq!(projection["viewGroups"]["Verse"]["missing"], 1);
        assert_eq!(projection["viewSelection"].as_array().unwrap().len(), 2);
        assert!(app.status.contains("1 missing source"));
        assert_eq!(app.editor.song(), &before);
        assert_eq!(app.editor.revision(), revision);
        assert!(app
            .apply(Request::RecallViewGroup {
                name: "Gone".into()
            })
            .is_err());
        assert_eq!(app.expanded_selection, selections);
    }

    #[test]
    fn meter_edit_requests_preserve_exact_positions_and_reject_invalid_boundaries_atomically() {
        let mut app = two_lanes();
        let before = app.editor.song().clone();
        let request = |revision, at| {
            serde_json::from_value::<Request>(json!({
                "kind":"edit", "revision":revision,
                "command":{"kind":"set_meters","meters":[
                    {"at":{"numerator":0,"denominator":1},"meter":[3,8]},
                    {"at":{"numerator":at,"denominator":2},"meter":[5,8]}
                ]}
            }))
            .unwrap()
        };
        app.apply(request(app.editor.revision(), 3)).unwrap();
        let edited = app.editor.song().clone();
        assert_eq!(edited.meters[1].at, Beat::new(3, 2).unwrap());
        assert_eq!(
            app.projection()["meters"][1]["at"],
            json!({"numerator":3,"denominator":2})
        );
        let revision = app.editor.revision();
        assert!(app.apply(request(revision, 2)).is_err());
        assert_eq!(app.editor.revision(), revision);
        assert_eq!(app.editor.song(), &edited);
        app.apply(Request::Undo).unwrap();
        assert_eq!(app.editor.song(), &before);
        app.apply(Request::Redo).unwrap();
        assert_eq!(app.editor.song(), &edited);
    }

    #[test]
    fn minimap_respects_independent_repeats_stops_and_meter_changes() {
        let mut app = two_lanes();
        let mut song = app.editor.song().clone();
        song.tracks[0].looped = true;
        song.tracks[1].slots[0].as_mut().unwrap().repeats = 2;
        song.tracks[1].slots.push(None);
        song.tracks[1].looped = true;
        song.meters = vec![
            MeterChange {
                at: Beat::ZERO,
                meter: Meter::new(3, 4).unwrap(),
            },
            MeterChange {
                at: Beat::integer(6),
                meter: Meter::new(5, 8).unwrap(),
            },
        ];
        app.editor = Editor::new(song).unwrap();
        app.apply(Request::PreviewBars { bars: 8 }).unwrap();
        app.apply(Request::View {
            view: View::Minimap,
        })
        .unwrap();
        let timeline = app.timeline().unwrap();
        assert_eq!(timeline.end, Beat::integer(21));
        assert_eq!(timeline.bars[2].at, Beat::integer(6));
        assert_eq!(timeline.lanes[0].spans.len(), 21);
        assert_eq!(timeline.lanes[0].spans[1].cycle, 1);
        assert_eq!(timeline.lanes[1].spans.len(), 2);
        assert_eq!(timeline.lanes[1].spans[1].start, Beat::integer(3));
        assert_eq!(timeline.lanes[1].spans[1].chain_repeat, 1);
        let selected = timeline.lanes[1].spans[1].clone();
        app.apply(Request::Locate {
            row_at: None,
            occurrence: selected.occurrence,
            at: selected.start,
            row: 0,
            view: View::Minimap,
        })
        .unwrap();
        app.apply(Request::Move {
            horizontal: -1,
            vertical: 0,
        })
        .unwrap();
        assert_eq!(app.track, 0);
        assert_eq!(app.timeline_focus, Some(Beat::integer(3)));
    }

    #[test]
    fn jump_preview_marks_uncertainty_and_dense_loops_hit_explicit_limits() {
        let mut app = two_lanes();
        let mut song = app.editor.song().clone();
        let first = song.tracks[0].slots[0].as_ref().unwrap().clone();
        let phrase_id = song.chains[&first.chain].phrases[0].phrase;
        let phrase = song.phrases.get_mut(&phrase_id).unwrap();
        let mut event = Event::note(
            EventId::new(100).unwrap(),
            Beat::ZERO,
            Target::instrument(InstrumentId::new(17).unwrap()),
            Note::new(60, 100, Beat::ONE).unwrap(),
        );
        event.fx[0] = Some(Fx::Jump { step: 0 });
        event.fx[1] = Some(Fx::Chance { percent: 50 });
        phrase.events.push(event);
        song.tracks[0].looped = true;
        let second = song.tracks[1].slots[0].as_ref().unwrap().clone();
        let dense = song.chains[&second.chain].phrases[0].phrase;
        song.phrases.get_mut(&dense).unwrap().steps = 1;
        song.phrases.get_mut(&dense).unwrap().division = Division::new(64, 255, 1).unwrap();
        song.tracks[1].looped = true;
        app.editor = Editor::new(song.clone()).unwrap();
        app.apply(Request::View {
            view: View::Minimap,
        })
        .unwrap();
        let timeline = app.timeline().unwrap();
        assert_eq!(timeline.lanes[0].spans.len(), 1);
        assert_eq!(timeline.lanes[0].conditional_from, Some(Beat::ZERO));
        assert!(timeline.lanes[0]
            .notice
            .as_ref()
            .unwrap()
            .contains("conditional or unbounded"));
        assert_eq!(timeline.lanes[1].spans.len(), 256);
        assert!(timeline.lanes[1].notice.as_ref().unwrap().contains("256"));
        assert_eq!(app.editor.song(), &song);
    }

    #[test]
    fn stale_occurrence_navigation_rejects_without_changing_focus_or_document() {
        let mut app = two_lanes();
        let before = (app.track, app.slot, app.chain_row, app.view);
        let song = app.editor.song().clone();
        let occurrence = Occurrence {
            track: song.tracks[1].id,
            chain_use: UseId::new(999).unwrap(),
            phrase_use: UseId::new(998).unwrap(),
        };
        assert!(app
            .apply(Request::Locate {
                row_at: None,
                occurrence,
                at: Beat::ZERO,
                row: 0,
                view: View::Phrase
            })
            .is_err());
        assert_eq!((app.track, app.slot, app.chain_row, app.view), before);
        assert_eq!(app.editor.song(), &song);
    }
}

#[cfg(test)]
mod materialization_tests {
    use super::*;

    fn phrase_app(form: PhraseForm) -> App {
        let mut app = App::new();
        app.command(Command::PutInstrument {
            id: InstrumentId::new(17).unwrap(),
            instrument: InstrumentUse {
                name: "Lead".into(),
                resource: None,
            },
        })
        .unwrap();
        app.apply(Request::Confirm).unwrap();
        app.apply(Request::Confirm).unwrap();
        let (id, source) = app.selected_phrase().unwrap();
        let mut phrase = source.clone();
        phrase.form = form;
        app.command(Command::PutPhrase { id, phrase }).unwrap();
        app
    }
    fn target() -> Target {
        Target::instrument(InstrumentId::new(17).unwrap())
    }
    fn euclidean() -> PhraseForm {
        PhraseForm::Euclidean {
            pitches: vec![],
            pulses: 7,
            rotation: 3,
            pitch: 60,
            velocity: 99,
            gate: Beat::new(5, 4).unwrap(),
            target: target(),
        }
    }
    fn performance(app: &App) -> Vec<(Beat, Target, Message)> {
        let (id, phrase) = app.selected_phrase().unwrap();
        let song = app.isolated_phrase_song(id, phrase);
        let snapshot =
            Snapshot::compile(&song, app.editor.revision(), PlaybackBudget::default()).unwrap();
        Playback::new(Arc::new(snapshot))
            .window(Beat::integer(u64::MAX))
            .unwrap()
            .into_iter()
            .map(|event| (event.at, event.target, event.message))
            .collect()
    }
    #[test]
    fn bake_matches_core_performance_including_tails_raw_controls_and_owned_expression() {
        let mut app = phrase_app(euclidean());
        let (source_id, source) = app.selected_phrase().unwrap();
        let mut phrase = source.clone();
        let mut note = Note::new(67, 81, Beat::integer(5)).unwrap();
        note.release_velocity = 55;
        note.expression = vec![
            Expression {
                sequence: None,
                offset: Beat::ZERO,
                control: Control::Bend { value: 8201 },
            },
            Expression {
                sequence: None,
                offset: Beat::new(1, 3).unwrap(),
                control: Control::Pressure { value: 57 },
            },
            Expression {
                sequence: None,
                offset: Beat::integer(5),
                control: Control::Cc {
                    number: 74,
                    value: 42,
                },
            },
        ];
        let mut authored = Event::note(
            EventId::new(app.editor.song().next_identity().unwrap()).unwrap(),
            Beat::new(1, 8).unwrap(),
            target(),
            note,
        );
        authored.controls.push(Control::Cc {
            number: 64,
            value: 127,
        });
        authored.fx = [
            Some(Fx::Gate {
                fraction: Beat::new(3, 2).unwrap(),
            }),
            Some(Fx::Delay {
                beats: Beat::new(1, 16).unwrap(),
            }),
            Some(Fx::Velocity { value: 82 }),
        ];
        phrase.events.push(authored);
        app.command(Command::PutPhrase {
            id: source_id,
            phrase,
        })
        .unwrap();
        let before = app.editor.song().clone();
        let resolved = performance(&app);
        let revision = app.editor.revision();
        let minimum_id = before.next_identity().unwrap();
        app.apply(Request::BakeGenerator {
            revision,
            occurrence: app.occurrence().unwrap(),
            form: BakeForm::PianoRoll,
        })
        .unwrap();
        assert_eq!(performance(&app), resolved);
        assert_eq!(app.editor.revision(), revision + 1);
        assert_ne!(app.selected_phrase().unwrap().0, source_id);
        assert_eq!(
            app.editor.song().phrases[&source_id],
            before.phrases[&source_id]
        );
        let baked = app.selected_phrase().unwrap().1;
        assert!(matches!(baked.form, PhraseForm::PianoRoll));
        assert!(baked
            .events
            .iter()
            .all(|event| event.id.get() >= minimum_id && event.fx.iter().all(Option::is_none)));
        assert!(baked
            .events
            .iter()
            .any(|event| event
                .note
                .as_ref()
                .is_some_and(|note| note.release_velocity == 55
                    && note.expression.len() == 3
                    && note.duration == Beat::new(15, 2).unwrap())));
        assert!(baked
            .events
            .iter()
            .any(|event| event.note.is_none() && !event.controls.is_empty()));
        app.apply(Request::Undo).unwrap();
        assert_eq!(app.editor.song(), &before);
    }
    #[test]
    fn random_bake_uses_selected_occurrence_seed_and_preserves_other_shared_uses() {
        let mut app = phrase_app(PhraseForm::Random {
            seed: 42,
            vary: true,
            density: 100,
            low: 48,
            high: 84,
            velocity_low: 50,
            velocity_high: 120,
            gate: Beat::new(3, 2).unwrap(),
            gate_high: None,
            target: target(),
        });
        let chain = app.selected_chain().unwrap().0;
        let track = app.editor.song().tracks[1].id;
        app.apply(Request::AssignChain {
            track,
            slot: 0,
            chain,
        })
        .unwrap();
        app.apply(Request::Confirm).unwrap();
        app.apply(Request::Confirm).unwrap();
        let before = app.editor.song().clone();
        let resolved = performance(&app);
        app.apply(Request::BakeGenerator {
            revision: app.editor.revision(),
            occurrence: app.occurrence().unwrap(),
            form: BakeForm::Tracker,
        })
        .unwrap();
        assert_eq!(performance(&app), resolved);
        assert_eq!(app.editor.song().tracks[0], before.tracks[0]);
        assert_eq!(app.editor.song().chains[&chain], before.chains[&chain]);
        assert!(matches!(
            app.selected_phrase().unwrap().1.form,
            PhraseForm::Tracker
        ));
    }
    #[test]
    fn stale_bake_and_side_effects_reject_without_any_document_change() {
        let mut app = phrase_app(euclidean());
        let occurrence = app.occurrence().unwrap();
        let old_revision = app.editor.revision();
        app.apply(Request::Rename {
            name: "Changed".into(),
        })
        .unwrap();
        let before = app.editor.song().clone();
        assert!(matches!(
            app.apply(Request::BakeGenerator {
                revision: old_revision,
                occurrence,
                form: BakeForm::Tracker
            }),
            Err(DocumentError::Stale { .. })
        ));
        assert_eq!(app.editor.song(), &before);
        for fx in [
            Fx::Jump { step: 0 },
            Fx::Tempo {
                tempo: Tempo::new(90000).unwrap(),
            },
            Fx::Send {
                bus: 1,
                amount: 50,
                duration: Beat::ONE,
            },
            Fx::Delay {
                beats: Beat::integer(8),
            },
        ] {
            let (id, source) = app.selected_phrase().unwrap();
            let mut phrase = source.clone();
            let mut event = Event::note(
                EventId::new(100).unwrap(),
                Beat::ZERO,
                target(),
                Note::new(60, 100, Beat::ONE).unwrap(),
            );
            event.fx[0] = Some(fx);
            phrase.events = vec![event];
            app.command(Command::PutPhrase { id, phrase }).unwrap();
            let before = app.editor.song().clone();
            let revision = app.editor.revision();
            assert!(app
                .apply(Request::BakeGenerator {
                    revision,
                    occurrence,
                    form: BakeForm::PianoRoll
                })
                .is_err());
            assert_eq!(app.editor.song(), &before);
            assert_eq!(app.editor.revision(), revision);
        }
    }
    #[test]
    fn drum_toggle_preserves_other_lanes_controls_and_undo() {
        let mut app = phrase_app(PhraseForm::Drum);
        let occurrence = app.occurrence().unwrap();
        for pitch in [36, 38] {
            app.apply(Request::DrumToggle {
                revision: app.editor.revision(),
                occurrence,
                step: 2,
                pitch,
                target: target(),
                velocity: 101,
                duration: Beat::new(1, 8).unwrap(),
            })
            .unwrap();
        }
        let (id, source) = app.selected_phrase().unwrap();
        let mut phrase = source.clone();
        phrase.events[0].controls.push(Control::Cc {
            number: 1,
            value: 83,
        });
        phrase.events[0].fx[0] = Some(Fx::Offset { semitones: 2 });
        app.command(Command::PutPhrase { id, phrase }).unwrap();
        let before = app.editor.song().clone();
        let revision = app.editor.revision();
        app.apply(Request::DrumToggle {
            revision,
            occurrence,
            step: 2,
            pitch: 36,
            target: target(),
            velocity: 101,
            duration: Beat::new(1, 8).unwrap(),
        })
        .unwrap();
        let events = &app.selected_phrase().unwrap().1.events;
        assert_eq!(events.len(), 2);
        assert!(events[0].note.is_none());
        assert_eq!(events[0].controls, before.phrases[&id].events[0].controls);
        assert_eq!(events[0].fx, before.phrases[&id].events[0].fx);
        assert_eq!(events[1], before.phrases[&id].events[1]);
        assert_eq!(app.editor.revision(), revision + 1);
        app.apply(Request::Undo).unwrap();
        assert_eq!(app.editor.song(), &before);
    }
    #[test]
    fn drum_toggle_preserves_precise_near_grid_notes() {
        let mut app = phrase_app(PhraseForm::Drum);
        let occurrence = app.occurrence().unwrap();
        let (id, source) = app.selected_phrase().unwrap();
        let mut phrase = source.clone();
        let offgrid = Event::note(
            EventId::new(app.editor.song().next_identity().unwrap()).unwrap(),
            Beat::new(1, 10_000_000_000).unwrap(),
            target(),
            Note::new(36, 101, Beat::new(1, 8).unwrap()).unwrap(),
        );
        phrase.events.push(offgrid.clone());
        app.command(Command::PutPhrase { id, phrase }).unwrap();
        let before = app.editor.song().clone();
        for expected_count in [2, 1] {
            app.apply(Request::DrumToggle {
                revision: app.editor.revision(),
                occurrence,
                step: 0,
                pitch: 36,
                target: target(),
                velocity: 101,
                duration: Beat::new(1, 8).unwrap(),
            })
            .unwrap();
            let events = &app.selected_phrase().unwrap().1.events;
            assert_eq!(events.len(), expected_count);
            assert!(events.contains(&offgrid));
            assert_eq!(
                events.iter().filter(|event| event.at == Beat::ZERO).count(),
                expected_count - 1
            );
        }
        assert_eq!(app.editor.song(), &before);
        app.apply(Request::Undo).unwrap();
        assert_eq!(app.selected_phrase().unwrap().1.events.len(), 2);
        app.apply(Request::Undo).unwrap();
        assert_eq!(app.editor.song(), &before);
    }
    #[test]
    fn drum_lane_settings_are_atomic_and_reject_stale_or_invalid_requests() {
        let mut app = phrase_app(PhraseForm::Drum);
        let occurrence = app.occurrence().unwrap();
        for step in [0, 4] {
            app.apply(Request::DrumToggle {
                revision: app.editor.revision(),
                occurrence,
                step,
                pitch: 36,
                target: target(),
                velocity: 90,
                duration: Beat::new(1, 8).unwrap(),
            })
            .unwrap();
        }
        let before = app.editor.song().clone();
        let revision = app.editor.revision();
        app.apply(Request::DrumLane {
            revision,
            occurrence,
            from_pitch: 36,
            from_target: target(),
            pitch: 40,
            target: Target::Channel {
                channel: Channel::new(10).unwrap(),
                endpoint: None,
            },
            velocity: 117,
            duration: Beat::new(1, 4).unwrap(),
        })
        .unwrap();
        let changed = app.editor.song().clone();
        assert!(app
            .selected_phrase()
            .unwrap()
            .1
            .events
            .iter()
            .all(|event| event.target.channel().get() == 10
                && event.note.as_ref().is_some_and(|note| note.pitch == 40
                    && note.velocity == 117
                    && note.duration == Beat::new(1, 4).unwrap())));
        assert!(app
            .apply(Request::DrumToggle {
                revision,
                occurrence,
                step: 0,
                pitch: 36,
                target: target(),
                velocity: 90,
                duration: Beat::ONE
            })
            .is_err());
        assert!(app
            .apply(Request::DrumToggle {
                revision: app.editor.revision(),
                occurrence,
                step: u32::MAX,
                pitch: 36,
                target: target(),
                velocity: 90,
                duration: Beat::ONE
            })
            .is_err());
        assert_eq!(app.editor.song(), &changed);
        app.apply(Request::Undo).unwrap();
        assert_eq!(app.editor.song(), &before);
    }
}

#[cfg(test)]
mod selection_tests {
    use super::*;

    fn roll() -> App {
        let mut app = App::new();
        app.command(Command::PutInstrument {
            id: InstrumentId::new(17).unwrap(),
            instrument: InstrumentUse {
                name: "Lead".into(),
                resource: None,
            },
        })
        .unwrap();
        app.apply(Request::Confirm).unwrap();
        app.apply(Request::Confirm).unwrap();
        let (id, source) = app.selected_phrase().unwrap();
        let mut phrase = source.clone();
        phrase.form = PhraseForm::PianoRoll;
        let mut next = app.editor.song().next_identity().unwrap();
        let mut first = Event::note(
            allocate_event_id(&mut next).unwrap(),
            Beat::new(1, 7).unwrap(),
            Target::instrument(InstrumentId::new(17).unwrap()),
            Note::new(60, 103, Beat::new(5, 7).unwrap()).unwrap(),
        );
        first.note.as_mut().unwrap().release_velocity = 43;
        first.note.as_mut().unwrap().expression = vec![
            Expression {
                sequence: None,
                offset: Beat::new(1, 97).unwrap(),
                control: Control::Bend { value: 8231 },
            },
            Expression {
                sequence: None,
                offset: Beat::new(1, 7).unwrap(),
                control: Control::PolyPressure {
                    pitch: 60,
                    value: 83,
                },
            },
        ];
        first.controls = vec![Control::PolyPressure {
            pitch: 48,
            value: 59,
        }];
        let mut overlap = first.clone();
        overlap.id = allocate_event_id(&mut next).unwrap();
        overlap.note = Some(Note::new(72, 77, Beat::new(2, 3).unwrap()).unwrap());
        overlap.controls.clear();
        let raw = Event {
            id: allocate_event_id(&mut next).unwrap(),
            at: Beat::new(2, 7).unwrap(),
            target: first.target.clone(),
            note: None,
            controls: vec![
                Control::Bend { value: 8193 },
                Control::Cc {
                    number: 64,
                    value: 127,
                },
            ],
            fx: [None, None, None],
        };
        phrase.events = vec![first, overlap, raw];
        app.command(Command::PutPhrase { id, phrase }).unwrap();
        select_all(&mut app);
        app
    }

    fn select_all(app: &mut App) {
        app.apply(Request::SelectEvents {
            revision: app.editor.revision(),
            occurrence: app.occurrence().unwrap(),
            events: app
                .selected_phrase()
                .unwrap()
                .1
                .events
                .iter()
                .map(|event| event.id)
                .collect(),
            toggle: false,
        })
        .unwrap();
    }

    fn edit(app: &mut App, action: SelectionAction) -> Result<(), DocumentError> {
        app.apply(Request::SelectionEdit {
            revision: app.editor.revision(),
            occurrence: app.occurrence().unwrap(),
            action,
        })
    }

    #[test]
    fn event_selection_distinguishes_overlaps_controls_and_rejects_stale_ids() {
        let mut app = roll();
        let before = app.editor.song().clone();
        let revision = app.editor.revision();
        let ids = app.selected_event_ids();
        let occurrence = app.occurrence().unwrap();
        assert_eq!(ids.len(), 3);
        app.apply(Request::SelectEvents {
            revision,
            occurrence,
            events: vec![ids[1], ids[1]],
            toggle: true,
        })
        .unwrap();
        assert_eq!(app.selected_event_ids(), vec![ids[0], ids[2]]);
        assert!(matches!(
            app.apply(Request::SelectEvents {
                revision: revision - 1,
                occurrence,
                events: vec![],
                toggle: false
            }),
            Err(DocumentError::Stale { .. })
        ));
        assert!(app
            .apply(Request::SelectEvents {
                revision,
                occurrence,
                events: vec![EventId::new(999).unwrap()],
                toggle: false
            })
            .is_err());
        assert_eq!(app.selected_event_ids(), vec![ids[0], ids[2]]);
        assert_eq!(app.editor.song(), &before);
        assert_eq!(app.editor.revision(), revision);
        app.clear_event_selection();
        assert!(app.projection()["selectedEvents"]
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn selection_move_is_exact_atomic_revision_checked_and_one_undo() {
        let mut app = roll();
        let before = app.editor.song().clone();
        let revision = app.editor.revision();
        let phrase = app.selected_phrase().unwrap().0;
        for action in [
            SelectionAction::Move {
                amount: Beat::ONE,
                backward: true,
            },
            SelectionAction::Move {
                amount: Beat::integer(100),
                backward: false,
            },
            SelectionAction::Transpose { semitones: 60 },
            SelectionAction::Duration {
                duration: Beat::ZERO,
            },
        ] {
            assert!(edit(&mut app, action).is_err());
            assert_eq!(app.editor.song(), &before);
            assert_eq!(app.editor.revision(), revision);
        }
        edit(
            &mut app,
            SelectionAction::Move {
                amount: Beat::new(1, 3).unwrap(),
                backward: false,
            },
        )
        .unwrap();
        assert_eq!(app.editor.revision(), revision + 1);
        for (original, moved) in before.phrases[&phrase]
            .events
            .iter()
            .zip(&app.editor.song().phrases[&phrase].events)
        {
            let mut expected = original.clone();
            expected.at = original.at.checked_add(Beat::new(1, 3).unwrap()).unwrap();
            assert_eq!(moved, &expected);
        }
        let moved = app.editor.song().clone();
        assert!(matches!(
            app.apply(Request::SelectionEdit {
                revision,
                occurrence: app.occurrence().unwrap(),
                action: SelectionAction::Delete
            }),
            Err(DocumentError::Stale { .. })
        ));
        assert_eq!(app.editor.song(), &moved);
        app.apply(Request::Undo).unwrap();
        assert_eq!(app.editor.song(), &before);
    }

    #[test]
    fn transpose_updates_owned_poly_pressure_but_preserves_raw_channel_messages() {
        let mut app = roll();
        let before = app.editor.song().clone();
        let phrase = app.selected_phrase().unwrap().0;
        edit(&mut app, SelectionAction::Transpose { semitones: 3 }).unwrap();
        let events = &app.editor.song().phrases[&phrase].events;
        assert_eq!(events[0].note.as_ref().unwrap().pitch, 63);
        assert_eq!(events[1].note.as_ref().unwrap().pitch, 75);
        assert_eq!(
            events[0].note.as_ref().unwrap().expression[1].control,
            Control::PolyPressure {
                pitch: 63,
                value: 83
            }
        );
        assert_eq!(
            events[0].note.as_ref().unwrap().expression[0],
            before.phrases[&phrase].events[0]
                .note
                .as_ref()
                .unwrap()
                .expression[0]
        );
        assert_eq!(
            events[0].controls,
            before.phrases[&phrase].events[0].controls
        );
        assert_eq!(events[2], before.phrases[&phrase].events[2]);
        let transposed = app.editor.song().clone();
        edit(
            &mut app,
            SelectionAction::Duration {
                duration: Beat::integer(2),
            },
        )
        .unwrap();
        let resized = &app.editor.song().phrases[&phrase].events;
        assert_eq!(resized[0].note.as_ref().unwrap().duration, Beat::integer(2));
        assert_eq!(resized[1].note.as_ref().unwrap().duration, Beat::integer(2));
        assert_eq!(
            resized[0].note.as_ref().unwrap().expression,
            transposed.phrases[&phrase].events[0]
                .note
                .as_ref()
                .unwrap()
                .expression
        );
        app.apply(Request::Undo).unwrap();
        assert_eq!(app.editor.song(), &transposed);
        app.apply(Request::Undo).unwrap();
        assert_eq!(app.editor.song(), &before);
    }

    #[test]
    fn clipboard_preserves_relative_timing_raw_values_fresh_ids_and_atomic_undo() {
        let mut app = roll();
        let before = app.editor.song().clone();
        let phrase = app.selected_phrase().unwrap().0;
        let revision = app.editor.revision();
        edit(&mut app, SelectionAction::Copy).unwrap();
        assert_eq!(app.editor.revision(), revision);
        assert_eq!(app.clipboard[0].at, Beat::ZERO);
        assert_eq!(app.clipboard[2].at, Beat::new(1, 7).unwrap());
        let clipboard = app.clipboard.clone();
        assert!(edit(
            &mut app,
            SelectionAction::Paste {
                at: Beat::integer(100)
            }
        )
        .is_err());
        assert_eq!(app.editor.song(), &before);
        assert_eq!(app.clipboard, clipboard);
        edit(&mut app, SelectionAction::Paste { at: Beat::ONE }).unwrap();
        assert_eq!(app.editor.revision(), revision + 1);
        let pasted_ids = app.selected_event_ids();
        assert_eq!(pasted_ids.len(), 3);
        for (index, copied) in app.clipboard.iter().enumerate() {
            assert!(!before.phrases[&phrase]
                .events
                .iter()
                .any(|event| event.id == pasted_ids[index]));
            let pasted = app.editor.song().phrases[&phrase]
                .events
                .iter()
                .find(|event| event.id == pasted_ids[index])
                .unwrap();
            let mut expected = copied.clone();
            expected.id = pasted_ids[index];
            expected.at = copied.at.checked_add(Beat::ONE).unwrap();
            assert_eq!(pasted, &expected);
        }
        app.apply(Request::Undo).unwrap();
        assert_eq!(app.editor.song(), &before);
        assert!(app.selected_event_ids().is_empty());
        select_all(&mut app);
        edit(&mut app, SelectionAction::Cut).unwrap();
        assert!(app.editor.song().phrases[&phrase].events.is_empty());
        assert_eq!(app.clipboard, clipboard);
        app.apply(Request::Undo).unwrap();
        assert_eq!(app.editor.song(), &before);
        app.apply(Request::Clear).unwrap();
        assert!(app.editor.song().phrases[&phrase].events.is_empty());
        app.apply(Request::Undo).unwrap();
        assert_eq!(app.editor.song(), &before);
    }

    #[test]
    fn explicit_quantize_keeps_expression_offsets_and_rejects_truncated_duration() {
        let mut app = roll();
        let before = app.editor.song().clone();
        let phrase = app.selected_phrase().unwrap().0;
        assert!(edit(
            &mut app,
            SelectionAction::Quantize {
                grid: Beat::integer(2),
                durations: true
            }
        )
        .is_err());
        assert_eq!(app.editor.song(), &before);
        edit(
            &mut app,
            SelectionAction::Quantize {
                grid: Beat::new(1, 4).unwrap(),
                durations: false,
            },
        )
        .unwrap();
        for (original, quantized) in before.phrases[&phrase]
            .events
            .iter()
            .zip(&app.editor.song().phrases[&phrase].events)
        {
            let mut expected = original.clone();
            expected.at = Beat::new(1, 4).unwrap();
            assert_eq!(quantized, &expected);
        }
        app.apply(Request::Undo).unwrap();
        assert_eq!(app.editor.song(), &before);
        edit(
            &mut app,
            SelectionAction::Quantize {
                grid: Beat::new(1, 8).unwrap(),
                durations: true,
            },
        )
        .unwrap();
        assert_eq!(
            app.editor.song().phrases[&phrase].events[0]
                .note
                .as_ref()
                .unwrap()
                .duration,
            Beat::new(3, 4).unwrap()
        );
        assert_eq!(
            app.editor.song().phrases[&phrase].events[0]
                .note
                .as_ref()
                .unwrap()
                .expression,
            before.phrases[&phrase].events[0]
                .note
                .as_ref()
                .unwrap()
                .expression
        );
        app.apply(Request::Undo).unwrap();
        assert_eq!(app.editor.song(), &before);
    }
}

#[cfg(test)]
mod ui_input_tests {
    use super::*;

    #[test]
    fn modal_entry_retires_held_composition_modifiers() {
        let mut app = App::new();
        app.edit_held = true;
        app.shift_held = true;
        app.apply(Request::TextEntry { active: true }).unwrap();
        assert!(app.text_entry);
        assert!(!app.edit_held);
        assert!(!app.shift_held);
        app.apply(Request::TextEntry { active: false }).unwrap();
        assert!(!app.text_entry);
        assert_eq!(app.editor.revision(), 0);
    }
}

#[cfg(test)]
mod edit_status_tests {
    use super::*;

    #[test]
    fn edited_status_replaces_saved_only_after_a_successful_mutation() {
        let mut app = App::new();
        app.status = "SAVED".into();
        let original = app.editor.song().clone();
        app.apply(Request::Edit {
            revision: 0,
            command: Box::new(Command::RenameSong {
                name: original.name.clone(),
            }),
        })
        .unwrap();
        assert_eq!(app.status, "SAVED");
        assert_eq!(app.editor.revision(), 0);
        assert!(!app.dirty());
        assert!(app
            .apply(Request::Edit {
                revision: 0,
                command: Box::new(Command::RenameSong {
                    name: String::new()
                }),
            })
            .is_err());
        assert_eq!(app.status, "SAVED");
        assert_eq!(app.editor.song(), &original);
        assert!(app
            .apply(Request::Edit {
                revision: 9,
                command: Box::new(Command::RenameSong {
                    name: "Stale title".into()
                }),
            })
            .is_err());
        assert_eq!(app.status, "SAVED");
        assert_eq!(app.editor.song(), &original);
        app.apply(Request::Edit {
            revision: 0,
            command: Box::new(Command::RenameSong {
                name: "Edited title".into(),
            }),
        })
        .unwrap();
        assert_eq!(app.status, "EDITED");
        assert_eq!(app.projection()["status"], "EDITED");
        assert_eq!(app.editor.revision(), 1);
        assert!(app.dirty());
        app.apply(Request::Undo).unwrap();
        assert_eq!(app.editor.song(), &original);
    }
}

#[cfg(test)]
mod chord_app_tests {
    use super::*;
    use crest_tracker::harmony::{Scale, ScaleKind};

    fn draft() -> ChordDraft {
        ChordDraft {
            name: "My minor".into(),
            intervals: vec![0, 3, 7],
            root: 60,
            inversion: 1,
            open: true,
            spread: 2,
        }
    }
    fn fixture() -> App {
        let mut app = App::new();
        app.command(Command::PutInstrument {
            id: InstrumentId::new(17).unwrap(),
            instrument: InstrumentUse {
                name: "Lead".into(),
                resource: None,
            },
        })
        .unwrap();
        app.apply(Request::Confirm).unwrap();
        app.apply(Request::Confirm).unwrap();
        let (id, original) = app.selected_phrase().unwrap();
        let mut phrase = original.clone();
        phrase.key = Some(Scale::new(3, ScaleKind::NaturalMinor).unwrap());
        let mut event = Event::note(
            EventId::new(app.editor.song().next_identity().unwrap()).unwrap(),
            Beat::ZERO,
            Target::instrument(InstrumentId::new(17).unwrap()),
            Note::new(60, 100, Beat::integer(1)).unwrap(),
        );
        event.controls = vec![Control::PolyPressure {
            pitch: 48,
            value: 59,
        }];
        event.fx = [
            Some(Fx::Velocity { value: 81 }),
            None,
            Some(Fx::Control {
                control: Control::Cc {
                    number: 74,
                    value: 91,
                },
            }),
        ];
        phrase.events = vec![event];
        app.command(Command::PutPhrase { id, phrase }).unwrap();
        app
    }
    fn event(app: &App) -> &Event {
        &app.selected_phrase().unwrap().1.events[0]
    }
    fn apply(app: &mut App, source: ChordSource, root: u8, slot: u8) -> Result<(), DocumentError> {
        app.apply(Request::ApplyChord {
            revision: app.editor.revision(),
            occurrence: app.occurrence().unwrap(),
            event: event(app).id,
            slot,
            root,
            source,
        })
    }
    fn save(app: &mut App) -> ChordRevisionId {
        app.apply(Request::SaveChord {
            revision: app.editor.revision(),
            draft: draft(),
        })
        .unwrap();
        *app.editor.song().chords.keys().next_back().unwrap()
    }
    fn pitches(app: &App) -> Vec<u8> {
        let (id, phrase) = app.selected_phrase().unwrap();
        let song = app.isolated_phrase_song(id, phrase);
        let snapshot =
            Snapshot::compile(&song, app.editor.revision(), PlaybackBudget::default()).unwrap();
        let mut pitches: Vec<_> = Playback::new(Arc::new(snapshot))
            .window(Beat::integer(4))
            .unwrap()
            .into_iter()
            .filter_map(|event| match event.message {
                Message::NoteOn { pitch, .. } => Some(pitch),
                _ => None,
            })
            .collect();
        pitches.sort_unstable();
        pitches
    }

    #[test]
    fn native_preview_verifies_exact_pitches_without_mutating_song_or_history() {
        let mut app = fixture();
        let original = app.editor.song().clone();
        let revision = app.editor.revision();
        app.apply(Request::PreviewChord {
            revision,
            token: 7,
            draft: draft(),
        })
        .unwrap();
        assert_eq!(
            app.projection()["chordPreview"]["chord"]["verification"]["pitches"],
            json!([63, 72, 79, 84, 87])
        );
        assert_eq!(app.projection()["chordPreview"]["token"], 7);
        let mut invalid = draft();
        invalid.root = 126;
        app.apply(Request::PreviewChord {
            revision,
            token: 8,
            draft: invalid,
        })
        .unwrap();
        assert!(app.projection()["chordPreview"]["error"].is_string());
        app.apply(Request::PreviewChord {
            revision: revision + 1,
            token: 9,
            draft: draft(),
        })
        .unwrap();
        assert!(app.projection()["chordPreview"]["error"]
            .as_str()
            .unwrap()
            .contains("stale"));
        assert_eq!(app.editor.revision(), revision);
        assert_eq!(app.editor.song(), &original);
        app.apply(Request::Undo).unwrap();
        assert!(app.selected_phrase().unwrap().1.events.is_empty());
    }

    #[test]
    fn custom_edits_allocate_fresh_immutable_revisions_and_undo_individually() {
        let mut app = fixture();
        let first = save(&mut app);
        let old = app.editor.song().chords.get(&first).unwrap().clone();
        let mut edited = draft();
        edited.name = "Edited minor".into();
        edited.open = false;
        app.apply(Request::SaveChord {
            revision: app.editor.revision(),
            draft: edited,
        })
        .unwrap();
        let second = *app.editor.song().chords.keys().next_back().unwrap();
        assert_ne!(first, second);
        assert_eq!(app.editor.song().chords.get(&first), Some(&old));
        assert_eq!(app.editor.song().chords.len(), 2);
        let before = app.editor.song().clone();
        assert!(app
            .apply(Request::SaveChord {
                revision: 0,
                draft: draft()
            })
            .is_err());
        assert_eq!(app.editor.song(), &before);
        app.apply(Request::Undo).unwrap();
        assert_eq!(app.editor.song().chords.len(), 1);
        assert_eq!(app.editor.song().chords.get(&first), Some(&old));
    }

    #[test]
    fn default_apply_uses_shared_intervals_preserves_other_slots_and_undoes_atomically() {
        let mut app = fixture();
        let original = app.editor.song().clone();
        let old_event = event(&app).clone();
        let revision = app.editor.revision();
        let expected = app.default_chords().unwrap()[0].clone();
        assert_eq!(
            app.projection()["chordBank"]["defaults"][0]["name"],
            expected.name()
        );
        apply(&mut app, ChordSource::Default { index: 0 }, 51, 1).unwrap();
        assert_eq!(app.editor.revision(), revision + 1);
        assert_eq!(app.editor.song().chords.len(), 1);
        let id = match event(&app).fx[1] {
            Some(Fx::ChordRevision { revision }) => revision,
            _ => panic!("missing chord"),
        };
        assert_eq!(app.editor.song().chords.get(&id), Some(&expected));
        assert_eq!(event(&app).fx[0], old_event.fx[0]);
        assert_eq!(event(&app).fx[2], old_event.fx[2]);
        assert_eq!(event(&app).controls, old_event.controls);
        assert_eq!(event(&app).note.as_ref().unwrap().pitch, 51);
        let label = fx_label(&event(&app).fx[1], &app.editor.song().chords);
        assert_eq!(label, "M");
        app.apply(Request::Undo).unwrap();
        assert_eq!(app.editor.song(), &original);
    }

    #[test]
    fn saved_chord_recall_is_exact_and_explicit_fx_override_defaults_in_any_slot() {
        let mut app = fixture();
        let id = save(&mut app);
        let bank = app.editor.song().chords.clone();
        apply(&mut app, ChordSource::Saved { revision: id }, 60, 1).unwrap();
        assert_eq!(pitches(&app), vec![63, 72, 79, 84, 87]);
        assert_eq!(app.editor.song().chords, bank);
        for chord_slot in 0..3 {
            let phrase = app.selected_phrase().unwrap().0;
            let mut changed = event(&app).clone();
            let mut remaining = [
                Some(Fx::Voicing {
                    inversion: 0,
                    open: false,
                }),
                Some(Fx::Spread { voices: 0 }),
            ]
            .into_iter();
            changed.fx = std::array::from_fn(|slot| {
                if slot == chord_slot {
                    Some(Fx::ChordRevision { revision: id })
                } else {
                    remaining.next().unwrap()
                }
            });
            app.command(Command::PutEvent {
                phrase,
                event: changed,
            })
            .unwrap();
            assert_eq!(pitches(&app), vec![60, 63, 67], "chord slot {chord_slot}");
        }
    }

    #[test]
    fn owned_expression_blocks_polyphonic_chords_but_single_voice_updates_its_pitch() {
        let mut app = fixture();
        let phrase = app.selected_phrase().unwrap().0;
        let mut edited = event(&app).clone();
        edited.note.as_mut().unwrap().expression = vec![Expression {
            sequence: None,
            offset: Beat::new(1, 2).unwrap(),
            control: Control::PolyPressure {
                pitch: 60,
                value: 73,
            },
        }];
        app.command(Command::PutEvent {
            phrase,
            event: edited,
        })
        .unwrap();
        let before = app.editor.song().clone();
        let error = apply(&mut app, ChordSource::Default { index: 0 }, 51, 1).unwrap_err();
        assert!(error.to_string().contains("separate expression members"));
        assert_eq!(app.editor.song(), &before);
        let one = ChordDraft {
            name: "Single voice".into(),
            intervals: vec![0],
            root: 60,
            inversion: 0,
            open: false,
            spread: 0,
        };
        app.apply(Request::SaveChord {
            revision: app.editor.revision(),
            draft: one,
        })
        .unwrap();
        let id = *app.editor.song().chords.keys().next_back().unwrap();
        apply(&mut app, ChordSource::Saved { revision: id }, 51, 1).unwrap();
        assert_eq!(
            event(&app).note.as_ref().unwrap().expression[0].control,
            Control::PolyPressure {
                pitch: 51,
                value: 73
            }
        );
        assert_eq!(
            event(&app).controls,
            before.phrases[&phrase].events[0].controls
        );
    }

    #[test]
    fn invalid_apply_rejects_stale_missing_and_out_of_range_changes_atomically() {
        let mut app = fixture();
        let id = save(&mut app);
        let original = app.editor.song().clone();
        let revision = app.editor.revision();
        for (slot, root) in [(3, 60), (1, 128), (1, 126)] {
            assert!(apply(&mut app, ChordSource::Saved { revision: id }, root, slot).is_err());
        }
        assert!(apply(&mut app, ChordSource::Default { index: usize::MAX }, 60, 1).is_err());
        assert!(apply(
            &mut app,
            ChordSource::Saved {
                revision: ChordRevisionId::new(99999).unwrap()
            },
            60,
            1
        )
        .is_err());
        let occurrence = app.occurrence().unwrap();
        let event = event(&app).id;
        assert!(app
            .apply(Request::ApplyChord {
                revision: revision + 1,
                occurrence,
                event,
                slot: 1,
                root: 60,
                source: ChordSource::Saved { revision: id }
            })
            .is_err());
        assert!(app
            .apply(Request::ApplyChord {
                revision,
                occurrence,
                event: EventId::new(99998).unwrap(),
                slot: 1,
                root: 60,
                source: ChordSource::Saved { revision: id }
            })
            .is_err());
        let changed_occurrence = Occurrence {
            track: original.tracks[1].id,
            ..occurrence
        };
        assert!(app
            .apply(Request::ApplyChord {
                revision,
                occurrence: changed_occurrence,
                event,
                slot: 1,
                root: 60,
                source: ChordSource::Saved { revision: id }
            })
            .is_err());
        assert_eq!(app.editor.song(), &original);
        assert_eq!(app.editor.revision(), revision);
    }
}

#[cfg(test)]
mod file_dialog_tests {
    use super::*;

    #[test]
    fn correlated_file_requests_and_projection_leave_document_ownership_with_runtime() {
        let mut app = App::new();
        let original = app.editor.song().clone();
        assert!(app.projection()["fileDialog"].is_null());
        let projection = json!({
            "token": 19, "version": 2, "title": "Save Song", "mode": "save",
            "directory": "/Music", "parent": "/", "filename": "Song.crest",
            "entries": [], "loading": false, "error": null, "overwrite": null
        });
        app.file_dialog = projection.clone();
        for request in [
            json!({"kind":"file_browse","token":19,"directory":"/Music"}),
            json!({"kind":"file_choose","token":19,"path":"/Music/Song.crest","overwrite":false}),
            json!({"kind":"file_decision","token":19,"decision":"save"}),
            json!({"kind":"file_cancel","token":19}),
        ] {
            app.apply(serde_json::from_value(request).unwrap()).unwrap();
            assert_eq!(app.editor.song(), &original);
            assert_eq!(app.editor.revision(), 0);
            assert_eq!(app.projection()["fileDialog"], projection);
        }
        assert!(serde_json::from_value::<Request>(
            json!({"kind":"file_choose","token":19,"path":"x"})
        )
        .is_err());
        assert!(
            serde_json::from_value::<Request>(json!({"kind":"file_cancel","token":-1})).is_err()
        );
    }
}
