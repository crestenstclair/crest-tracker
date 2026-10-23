//! A deterministic arrangement for the independent-track event scene.
use super::*;

pub(crate) fn arrangement(song: &Song, target: Target) -> Result<Command, DocumentError> {
    if !song.chains.is_empty() || !song.phrases.is_empty() || song.tracks.len() < 2 {
        return Err(DocumentError::Invalid(
            "cursor demo requires an empty Song with two tracks",
        ));
    }
    let mut next = song.next_identity()?;
    let mut identity = || {
        let id = next;
        next = next
            .checked_add(1)
            .ok_or(DocumentError::Invalid("identity exhausted"))?;
        Ok::<_, DocumentError>(id)
    };
    let mut commands = vec![
        Command::RenameSong {
            name: "Independent track cursors".into(),
        },
        Command::SetTempo {
            tempo: Tempo::new(120_000)?,
        },
    ];
    let short = PhraseId::new(identity()?)?;
    let spacer = PhraseId::new(identity()?)?;
    for (id, steps, root) in [(short, 10, 60), (spacer, 5, 48)] {
        let mut phrase = Phrase {
            name: if id == short { "00" } else { "01" }.into(),
            steps,
            ..Phrase::default()
        };
        for step in 0..steps {
            phrase.events.push(Event::note(
                EventId::new(identity()?)?,
                Beat::new(u64::from(step), 4)?,
                target.clone(),
                Note::new(
                    root + [0, 4, 7, 9, 12][step as usize % 5],
                    70,
                    Beat::new(1, 8)?,
                )?,
            ));
        }
        commands.push(Command::PutPhrase { id, phrase });
    }
    let short_chain = ChainId::new(identity()?)?;
    let long_chain = ChainId::new(identity()?)?;
    for (id, name, sources) in [
        (short_chain, "10 rows", vec![(short, 1)]),
        (
            long_chain,
            "100 rows",
            vec![(spacer, 1), (short, 9), (spacer, 1)],
        ),
    ] {
        let phrases = sources
            .into_iter()
            .map(|(phrase, repeats)| {
                Ok(PhraseUse {
                    id: UseId::new(identity()?)?,
                    phrase,
                    repeats,
                })
            })
            .collect::<Result<_, DocumentError>>()?;
        commands.push(Command::PutChain {
            id,
            chain: Chain {
                name: name.into(),
                phrases,
            },
        });
    }
    for (index, count, chain, name) in [
        (0, 10, short_chain, "10 × 10"),
        (1, 1, long_chain, "1 × 100 · offset 5"),
    ] {
        let mut track = song.tracks[index].clone();
        track.name = name.into();
        track.looped = true;
        track.slots = (0..count)
            .map(|_| {
                Ok(Some(ChainUse {
                    id: UseId::new(identity()?)?,
                    chain: chain.into(),
                    repeats: 1,
                }))
            })
            .collect::<Result<_, DocumentError>>()?;
        commands.push(Command::PutTrack { track });
    }
    Ok(Command::Batch { commands })
}
