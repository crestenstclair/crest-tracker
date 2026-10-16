//! An original small arrangement that exposes the editable phrase forms.
use super::*;
use crest_tracker::time::Division;

pub(crate) fn arrangement(song: &Song, targets: [Target; 5]) -> Result<Command, DocumentError> {
    if !song.chains.is_empty() || !song.phrases.is_empty() {
        return Err(DocumentError::Invalid("demo requires an empty Song"));
    }
    let mut next = song.next_identity()?;
    let mut identity = || -> Result<u64, DocumentError> {
        let id = next;
        next = next
            .checked_add(1)
            .ok_or(DocumentError::Invalid("identity exhausted"))?;
        Ok(id)
    };
    let mut phrases = Vec::new();
    let mut lead = Phrase {
        name: "Window Light · A".into(),
        steps: 32,
        form: PhraseForm::PianoRoll,
        ..Phrase::default()
    };
    for (index, pitch) in [
        64, 67, 71, 69, 67, 62, 64, 60, 64, 67, 72, 71, 69, 67, 64, 62,
    ]
    .into_iter()
    .enumerate()
    {
        let at = Beat::new(index as u64, 2)?.checked_add(if index % 4 == 1 {
            Beat::new(1, 24)?
        } else {
            Beat::ZERO
        })?;
        lead.events.push(Event::note(
            EventId::new(identity()?)?,
            at,
            targets[0].clone(),
            Note::new(
                pitch,
                if index % 4 == 0 { 94 } else { 76 },
                Beat::new(3, 8)?,
            )?,
        ));
    }
    phrases.push(lead);
    let mut answer = phrases[0].clone();
    answer.name = "Window Light · B".into();
    for event in &mut answer.events {
        event.id = EventId::new(identity()?)?;
        if let Some(note) = &mut event.note {
            note.pitch = match note.pitch {
                72 => 74,
                71 => 72,
                62 => 64,
                pitch => pitch,
            };
        }
    }
    phrases.push(answer);
    let mut bass = Phrase {
        name: "Roots · C Am F G".into(),
        steps: 32,
        ..Phrase::default()
    };
    for (index, pitch) in [36, 33, 41, 43].into_iter().enumerate() {
        for (offset, velocity) in [(Beat::ZERO, 88), (Beat::new(3, 2)?, 65)] {
            bass.events.push(Event::note(
                EventId::new(identity()?)?,
                Beat::integer(index as u64 * 2).checked_add(offset)?,
                targets[1].clone(),
                Note::new(pitch, velocity, Beat::new(3, 8)?)?,
            ));
        }
    }
    phrases.push(bass);
    let mut harmony = Phrase {
        name: "Sevenths · editable chord FX".into(),
        steps: 32,
        ..Phrase::default()
    };
    for (index, (pitch, intervals)) in [
        (48, vec![0, 4, 7, 11]),
        (45, vec![0, 3, 7, 10]),
        (53, vec![0, 4, 7, 11]),
        (55, vec![0, 4, 7, 10]),
    ]
    .into_iter()
    .enumerate()
    {
        let mut event = Event::note(
            EventId::new(identity()?)?,
            Beat::integer(index as u64 * 2),
            targets[2].clone(),
            Note::new(pitch, 48, Beat::new(7, 4)?)?,
        );
        event.fx[0] = Some(Fx::Chord { intervals });
        harmony.events.push(event);
    }
    phrases.push(harmony);
    phrases.push(Phrase {
        name: "Five against eight".into(),
        steps: 8,
        division: Division::new(8, 1, 1)?,
        form: PhraseForm::Euclidean {
            pulses: 5,
            rotation: 0,
            pitch: 79,
            velocity: 39,
            gate: Beat::new(1, 5)?,
            target: targets[3].clone(),
        },
        ..Phrase::default()
    });
    let mut rhythm = Phrase {
        name: "Pulse · Drum lanes".into(),
        steps: 16,
        form: PhraseForm::Drum,
        ..Phrase::default()
    };
    for step in [0, 4, 8, 10, 12, 14] {
        rhythm.events.push(Event::note(
            EventId::new(identity()?)?,
            Beat::new(step, 4)?,
            targets[4].clone(),
            Note::new(
                if step % 4 == 0 { 48 } else { 72 },
                if step % 8 == 0 { 75 } else { 45 },
                Beat::new(1, 16)?,
            )?,
        ));
    }
    phrases.push(rhythm);
    let ids = phrases
        .iter()
        .map(|_| PhraseId::new(identity()?))
        .collect::<Result<Vec<_>, DocumentError>>()?;
    let mut commands = vec![
        Command::RenameSong {
            name: "Window Light".into(),
        },
        Command::SetTempo {
            tempo: Tempo::new(108_000)?,
        },
    ];
    for (id, phrase) in ids.iter().copied().zip(phrases) {
        commands.push(Command::PutPhrase { id, phrase });
    }
    for (index, (name, phrase_indices)) in [
        ("Melody", vec![0, 1, 0, 1]),
        ("Bass", vec![2, 2]),
        ("Harmony", vec![3, 3]),
        ("Euclidean", vec![4, 4, 4, 4]),
        ("Pulse", vec![5, 5, 5, 5]),
    ]
    .into_iter()
    .enumerate()
    {
        let chain = ChainId::new(identity()?)?;
        let phrases = phrase_indices
            .into_iter()
            .map(|i| {
                Ok(PhraseUse {
                    id: UseId::new(identity()?)?,
                    phrase: ids[i],
                    repeats: 1,
                })
            })
            .collect::<Result<Vec<_>, DocumentError>>()?;
        commands.push(Command::PutChain {
            id: chain,
            chain: Chain {
                name: name.into(),
                phrases,
            },
        });
        let mut track = song
            .tracks
            .get(index)
            .cloned()
            .ok_or(DocumentError::Invalid("demo needs five tracks"))?;
        track.name = name.into();
        track.looped = true;
        track.muted = false;
        track.slots = vec![Some(ChainUse {
            id: UseId::new(identity()?)?,
            chain,
            repeats: 1,
        })];
        commands.push(Command::PutTrack { track });
    }
    Ok(Command::Batch { commands })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn demo_is_one_undoable_multitrack_arrangement_with_shared_phrases_and_raw_timing() {
        let mut editor = Editor::new(Song::default()).unwrap();
        let before = editor.song().clone();
        let target = Target::Channel {
            channel: Channel::new(1).unwrap(),
            endpoint: None,
        };
        editor
            .apply(
                0,
                arrangement(editor.song(), std::array::from_fn(|_| target.clone())).unwrap(),
            )
            .unwrap();
        let snapshot =
            Snapshot::compile(editor.song(), editor.revision(), PlaybackBudget::default()).unwrap();
        let mut playback = Playback::new(Arc::new(snapshot));
        let events = playback.window(Beat::integer(32)).unwrap();
        let tracks: std::collections::BTreeSet<_> = events
            .iter()
            .filter(|e| matches!(e.message, Message::NoteOn { .. }))
            .map(|e| e.owner.track)
            .collect();
        assert_eq!(tracks.len(), 5);
        assert!(events.iter().any(|e| e.at == Beat::new(13, 24).unwrap()));
        assert_eq!(editor.song().phrases.len(), 6);
        let serialized = serde_json::to_vec(editor.song()).unwrap();
        assert_eq!(
            serde_json::from_slice::<Song>(&serialized).unwrap(),
            *editor.song()
        );
        editor.undo().unwrap();
        assert_eq!(editor.song(), &before);
    }
}
