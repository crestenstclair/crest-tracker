//! Exact musical labels and source context for the native grid projection.
use super::*;
use crest_tracker::editing::shared::{self, Definition};
use crest_tracker::time::Division;

pub(super) const PAGE_ROWS: usize = 64;
type Grid = (String, String, Vec<String>, GridRows);

/// Presentation window only. Absolute source rows and document limits stay intact.
#[derive(Default)]
pub(super) struct GridRows {
    pub start: usize,
    pub total: usize,
    pub values: Vec<Vec<String>>,
}
impl GridRows {
    fn new(selected: usize, total: usize, row: impl FnMut(usize) -> Vec<String>) -> Self {
        let start = selected.min(total.saturating_sub(1)) / PAGE_ROWS * PAGE_ROWS;
        let end = start.saturating_add(PAGE_ROWS).min(total);
        Self {
            start,
            total,
            values: (start..end).map(row).collect(),
        }
    }
}

impl App {
    pub(super) fn grid(&self) -> Grid {
        let song = self.editor.song();
        match self.view {
            View::Expanded => (
                "Expanded tracker".into(),
                format!(
                    "{} SELECTED CHAINS · SOURCE EVENTS · {} BAR LIMIT",
                    self.timeline_selection().len(),
                    self.preview_bars
                ),
                vec![],
                GridRows::default(),
            ),
            View::Minimap => (
                song.name.clone(),
                format!(
                    "{} BPM · {} · {} TRACKS",
                    song.tempo.bpm(),
                    song.key.name(),
                    song.tracks.len()
                ),
                vec![],
                GridRows::default(),
            ),
            View::Phrase => self.phrase_grid(),
            View::Chain => self.chain_grid(),
            _ => self.song_grid(),
        }
    }

    fn phrase_grid(&self) -> Grid {
        let Some((_, phrase)) = self.selected_phrase() else {
            return (
                "NO PHRASE".into(),
                "Choose a phrase from its chain".into(),
                vec![],
                GridRows::default(),
            );
        };
        let track = &self.editor.song().tracks[self.track];
        (
            phrase.name.clone(),
            format!(
                "TRACK {:02} · {} · {} · {} STEPS · {}",
                self.track + 1,
                track.name,
                form_label(&phrase.form),
                phrase.steps,
                division_label(phrase.division)
            ),
            ["NOTE", "INSTRUMENT", "VELOCITY", "FX 1", "FX 2", "FX 3"]
                .map(str::to_owned)
                .to_vec(),
            GridRows::new(self.row, phrase.steps as usize, |row| {
                self.phrase_row(phrase, row as u32)
            }),
        )
    }

    fn chain_grid(&self) -> Grid {
        let Some((id, chain)) = self.selected_chain() else {
            return (
                "NO CHAIN".into(),
                "Create a chain in Song".into(),
                vec![],
                GridRows::default(),
            );
        };
        let song = self.editor.song();
        let track = &song.tracks[self.track];
        let references = shared::references(song, Definition::Chain(id)).len();
        let rows = GridRows::new(self.row, chain.phrases.len() + 1, |row| {
            chain.phrases.get(row).map_or_else(
                || vec!["+ NEW PHRASE".into(), "—".into(), "—".into(), "—".into()],
                |usage| chain_row(song, usage),
            )
        });
        (
            chain.name.clone(),
            format!(
                "TRACK {:02} · {} · USED {} {}",
                self.track + 1,
                track.name,
                references,
                if references == 1 { "TIME" } else { "TIMES" }
            ),
            ["PHRASE", "REPEATS", "LENGTH", "DURATION"]
                .map(str::to_owned)
                .to_vec(),
            rows,
        )
    }

    fn song_grid(&self) -> Grid {
        let song = self.editor.song();
        let columns = song
            .tracks
            .iter()
            .enumerate()
            .map(|(i, track)| format!("{:02} / {}", i + 1, track.name))
            .collect();
        let count = song
            .tracks
            .iter()
            .map(|track| track.slots.len() + 1)
            .max()
            .unwrap_or(0)
            .max(12);
        let rows = GridRows::new(self.row, count, |row| song_row(song, row));
        (
            song.name.clone(),
            format!(
                "{} BPM · {} · {} TRACKS",
                song.tempo.bpm(),
                song.key.name(),
                song.tracks.len()
            ),
            columns,
            rows,
        )
    }

    fn phrase_row(&self, phrase: &Phrase, row: u32) -> Vec<String> {
        let at = phrase
            .division
            .beat_length()
            .checked_scale(u64::from(row), 1)
            .expect("validated phrase");
        let Some(event) = phrase.events.iter().find(|v| v.at == at) else {
            if let Some((pitch, velocity, target)) = self
                .preview_cache
                .borrow()
                .as_ref()
                .and_then(|cache| cache.notes.get(&u64::from(row)))
            {
                if matches!(
                    phrase.form,
                    PhraseForm::Euclidean { .. } | PhraseForm::Random { .. }
                ) {
                    return vec![
                        phrase
                            .key
                            .as_ref()
                            .unwrap_or(&self.editor.song().key)
                            .pitch_name(Pitch::new(*pitch).expect("compiled MIDI pitch")),
                        self.target_label(target),
                        format!("{}%", u32::from(*velocity) * 100 / 127),
                        "—".into(),
                        "—".into(),
                        "—".into(),
                    ];
                }
            }
            return vec!["—".into(); 6];
        };
        self.event_cells(phrase, event)
    }
    pub(super) fn event_cells(&self, phrase: &Phrase, event: &Event) -> Vec<String> {
        let pitch = event
            .note
            .as_ref()
            .and_then(|n| Pitch::new(n.pitch).ok())
            .map(|p| {
                let key = phrase.key.as_ref().unwrap_or(&self.editor.song().key);
                if self.pitch_intervals {
                    format!(
                        "{} · oct {}",
                        key.interval_name(p),
                        i16::from(p.midi()) / 12 - 1
                    )
                } else {
                    key.pitch_name(p)
                }
            })
            .unwrap_or("—".into());
        let target = self.target_label(&event.target);
        let mut cells = vec![
            pitch,
            target,
            event
                .note
                .as_ref()
                .map(|n| format!("{}%", u32::from(n.velocity) * 100 / 127))
                .unwrap_or("—".into()),
        ];
        cells.extend(
            event
                .fx
                .iter()
                .map(|fx| fx_label(fx, &self.editor.song().chords)),
        );
        cells
    }
    fn target_label(&self, target: &Target) -> String {
        match target {
            Target::Instrument { id, channel } => format!(
                "{:02} / {} · CH {:02}",
                id.get(),
                self.editor
                    .song()
                    .instruments
                    .get(id)
                    .map(|v| v.name.clone())
                    .unwrap_or("UNRESOLVED".into()),
                channel.get()
            ),
            Target::Channel {
                channel,
                endpoint: None,
            } => format!("INTERNAL · CH {:02}", channel.get()),
            Target::Channel {
                channel,
                endpoint: Some(endpoint),
            } => format!("EXTERNAL / {endpoint} · CH {:02}", channel.get()),
        }
    }
}

fn song_row(song: &Song, row: usize) -> Vec<String> {
    song.tracks
        .iter()
        .map(|track| {
            track
                .slots
                .get(row)
                .and_then(Option::as_ref)
                .map(|slot| format!("{:02}", slot.chain.get()))
                .unwrap_or("—".into())
        })
        .collect()
}

fn chain_row(song: &Song, usage: &PhraseUse) -> Vec<String> {
    let phrase = &song.phrases[&usage.phrase];
    let duration = phrase
        .length()
        .and_then(|length| length.checked_scale(u64::from(usage.repeats), 1));
    // Jump FX can alter or prevent completion. Never promise a fixed duration
    // from source length when the runtime owns conditional flow.
    let conditional = phrase.events.iter().any(event_has_jump);
    let duration = if conditional {
        "CONDITIONAL · JUMP FX".into()
    } else {
        duration
            .map(|beats| format!("{beats} BEATS"))
            .unwrap_or("EXCEEDS TIME RANGE".into())
    };
    vec![
        format!("{:02} / {}", usage.phrase.get(), phrase.name),
        usage.repeats.to_string(),
        format!("{} × {}", phrase.steps, division_label(phrase.division)),
        duration,
    ]
}

fn event_has_jump(event: &Event) -> bool {
    event
        .fx
        .iter()
        .flatten()
        .any(|fx| matches!(fx, Fx::Jump { .. }))
}

fn division_label(division: Division) -> String {
    let denominator = division.denominator();
    match division.tuplet() {
        (1, 1) => format!("1/{denominator}"),
        (3, 2) => format!("1/{denominator}T"),
        (notes, time) => format!("1/{denominator} ({notes}:{time})"),
    }
}

fn form_label(form: &PhraseForm) -> &'static str {
    match form {
        PhraseForm::Tracker => "TRACKER",
        PhraseForm::PianoRoll => "PIANO ROLL",
        PhraseForm::Drum => "DRUM",
        PhraseForm::Euclidean { .. } => "EUCLIDEAN",
        PhraseForm::Random { .. } => "RANDOM",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_keep_identity_channel_and_endpoint_distinct() {
        let mut app = App::new();
        let mut song = app.editor.song().clone();
        for id in [17, 18] {
            song.instruments.insert(
                InstrumentId::new(id).unwrap(),
                InstrumentUse {
                    name: "Bass".into(),
                    resource: None,
                },
            );
        }
        app.editor = Editor::new(song).unwrap();
        let target = |id, channel| Target::Instrument {
            id: InstrumentId::new(id).unwrap(),
            channel: Channel::new(channel).unwrap(),
        };
        assert_eq!(app.target_label(&target(17, 1)), "17 / Bass · CH 01");
        assert_ne!(
            app.target_label(&target(17, 1)),
            app.target_label(&target(18, 1))
        );
        assert_ne!(
            app.target_label(&target(17, 1)),
            app.target_label(&target(17, 2))
        );
        let external = Target::Channel {
            channel: Channel::FIRST,
            endpoint: Some("Port A".into()),
        };
        let internal = Target::Channel {
            channel: Channel::FIRST,
            endpoint: None,
        };
        assert_eq!(app.target_label(&external), "EXTERNAL / Port A · CH 01");
        assert_eq!(app.target_label(&internal), "INTERNAL · CH 01");
    }

    #[test]
    fn placement_duration_retains_exact_tuplets_and_repeats_without_editing_music() {
        let mut app = App::new();
        app.apply(Request::Confirm).unwrap();
        let (id, phrase) = app.selected_phrase().unwrap();
        let mut phrase = phrase.clone();
        phrase.name = "Triplet".into();
        phrase.steps = 5;
        phrase.division = Division::new(16, 3, 2).unwrap();
        app.command(Command::PutPhrase { id, phrase }).unwrap();
        let (chain_id, chain) = app.selected_chain().unwrap();
        let mut chain = chain.clone();
        chain.phrases[0].repeats = 3;
        app.command(Command::PutChain {
            id: chain_id,
            chain,
        })
        .unwrap();
        let before = app.editor.song().clone();
        let projection = app.projection();
        assert_eq!(
            projection["columns"],
            json!(["PHRASE", "REPEATS", "LENGTH", "DURATION"])
        );
        assert_eq!(
            projection["rows"][0],
            json!([
                format!("{:02} / Triplet", id.get()),
                "3",
                "5 × 1/16T",
                "5/2 BEATS"
            ])
        );
        assert!(projection["subtitle"]
            .as_str()
            .unwrap()
            .contains("USED 1 TIME"));
        app.apply(Request::Focus { row: 0, column: 3 }).unwrap();
        assert_eq!(
            app.column, 1,
            "derived length/duration do not add editable columns"
        );
        app.apply(Request::Confirm).unwrap();
        let projection = app.projection();
        assert!(projection["subtitle"]
            .as_str()
            .unwrap()
            .contains("TRACKER · 5 STEPS · 1/16T"));
        assert_eq!(projection["phraseKey"], app.editor.song().key.name());
        assert_eq!(app.editor.song(), &before);
    }

    #[test]
    fn jump_placement_never_claims_a_fixed_duration() {
        let mut app = App::new();
        app.apply(Request::Confirm).unwrap();
        let (id, phrase) = app.selected_phrase().unwrap();
        let mut phrase = phrase.clone();
        phrase.events.push(Event {
            id: EventId::new(app.editor.song().next_identity().unwrap()).unwrap(),
            at: Beat::ZERO,
            target: Target::Channel {
                channel: Channel::new(1).unwrap(),
                endpoint: None,
            },
            note: None,
            controls: vec![],
            fx: [Some(Fx::Jump { step: 0 }), None, None],
        });
        app.command(Command::PutPhrase { id, phrase }).unwrap();
        assert_eq!(app.projection()["rows"][0][3], "CONDITIONAL · JUMP FX");
    }
}
