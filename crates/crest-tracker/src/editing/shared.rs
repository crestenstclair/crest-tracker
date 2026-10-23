//! Shared-definition topology and atomic edits to one occurrence.
use super::*;
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum Definition {
    Phrase(PhraseId),
    Chain(ChainId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Location {
    pub track: TrackId,
    pub chain_use: UseId,
    pub phrase_use: Option<UseId>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Reference {
    pub location: Location,
    pub track_name: String,
    pub slot: usize,
    pub phrase_row: Option<usize>,
}

/// Count actual arrangement uses, including uses through shared parent chains.
pub fn references(song: &Song, definition: Definition) -> Vec<Reference> {
    song.tracks
        .iter()
        .flat_map(|track| track_references(song, track, definition))
        .collect()
}

fn track_references(song: &Song, track: &Track, definition: Definition) -> Vec<Reference> {
    track
        .slots
        .iter()
        .enumerate()
        .flat_map(|(slot, usage)| {
            usage
                .as_ref()
                .map(|usage| slot_references(song, track, slot, usage, definition))
                .unwrap_or_default()
        })
        .collect()
}

fn slot_references(
    song: &Song,
    track: &Track,
    slot: usize,
    usage: &ChainUse,
    definition: Definition,
) -> Vec<Reference> {
    let reference = |phrase_use, phrase_row| Reference {
        location: Location {
            track: track.id,
            chain_use: usage.id,
            phrase_use,
        },
        track_name: track.name.clone(),
        slot,
        phrase_row,
    };
    let Ok(chain_id) = usage.chain.pattern() else {
        return Vec::new();
    };
    match definition {
        Definition::Chain(id) if id == chain_id => vec![reference(None, None)],
        Definition::Phrase(id) => song.chains[&chain_id]
            .phrases
            .iter()
            .enumerate()
            .filter(|(_, usage)| usage.phrase == id)
            .map(|(row, usage)| reference(Some(usage.id), Some(row)))
            .collect(),
        _ => Vec::new(),
    }
}

/// Existing definitions changed by a local editing command. New resources do
/// not need shared-edit consent, but structural commands cannot be localized.
pub fn edited_definitions(song: &Song, command: &Command) -> Option<BTreeSet<Definition>> {
    let definition = match command {
        Command::PutPhrase { id, .. } if song.phrases.contains_key(id) => {
            Some(Definition::Phrase(*id))
        }
        Command::PutChain { id, .. } if song.chains.contains_key(id) => {
            Some(Definition::Chain(*id))
        }
        Command::PutEvent { phrase, .. }
        | Command::DeleteEvents { phrase, .. }
        | Command::Quantize { phrase, .. }
        | Command::ResizeNotes { phrase, .. }
        | Command::ResizePhrase { phrase, .. }
        | Command::RetimeDivision { phrase, .. } => Some(Definition::Phrase(*phrase)),
        Command::PutChord { .. } | Command::PutPhrase { .. } | Command::PutChain { .. } => None,
        Command::Batch { commands } => {
            let mut definitions = BTreeSet::new();
            for command in commands {
                definitions.extend(edited_definitions(song, command)?);
            }
            return Some(definitions);
        }
        _ => return None,
    };
    Some(definition.into_iter().collect())
}

/// Mixed batches still require shared-edit consent even when another operation
/// prevents localizing that batch to one occurrence.
pub fn affected_definitions(song: &Song, command: &Command) -> BTreeSet<Definition> {
    match command {
        Command::Batch { commands } => commands
            .iter()
            .flat_map(|command| affected_definitions(song, command))
            .collect(),
        _ => edited_definitions(song, command).unwrap_or_default(),
    }
}

pub fn definition_at(song: &Song, location: Location) -> Result<Definition, DocumentError> {
    let usage = song
        .tracks
        .iter()
        .find(|track| track.id == location.track)
        .and_then(|track| {
            track
                .slots
                .iter()
                .flatten()
                .find(|usage| usage.id == location.chain_use)
        })
        .ok_or(DocumentError::Invalid(
            "shared source occurrence no longer exists",
        ))?;
    match location.phrase_use {
        None => Ok(Definition::Chain(usage.chain.pattern()?)),
        Some(id) => song.chains[&usage.chain.pattern()?]
            .phrases
            .iter()
            .find(|usage| usage.id == id)
            .map(|usage| Definition::Phrase(usage.phrase))
            .ok_or(DocumentError::Invalid(
                "shared phrase occurrence no longer exists",
            )),
    }
}

pub(super) fn edit_unique(
    song: &mut Song,
    location: Location,
    command: Command,
) -> Result<(), DocumentError> {
    let definition = definition_at(song, location)?;
    if edited_definitions(song, &command) != Some([definition].into_iter().collect()) {
        return Err(DocumentError::Invalid(
            "local edit must change only the selected shared definition",
        ));
    }
    let original = song.clone();
    super::apply_command(song, command)?;
    song.validate()?;
    match definition {
        Definition::Phrase(id) => {
            super::make_unique(
                song,
                Occurrence {
                    track: location.track,
                    chain_use: location.chain_use,
                    phrase_use: location.phrase_use.expect("phrase location"),
                },
            )?;
            song.phrases.insert(id, original.phrases[&id].clone());
        }
        Definition::Chain(id) => {
            let mut chain = song.chains[&id].clone();
            let mut next = song.next_identity()?;
            for usage in &mut chain.phrases {
                usage.id = UseId::new(super::allocate(&mut next)?)?;
            }
            let private = ChainId::new(super::allocate(&mut next)?)?;
            chain.name.push_str(" copy");
            song.chains.insert(private, chain);
            song.chains.insert(id, original.chains[&id].clone());
            let usage = song
                .tracks
                .iter_mut()
                .find(|track| track.id == location.track)
                .and_then(|track| {
                    track
                        .slots
                        .iter_mut()
                        .flatten()
                        .find(|usage| usage.id == location.chain_use)
                })
                .ok_or(DocumentError::Invalid("local chain occurrence disappeared"))?;
            usage.chain = private.into();
        }
    }
    Ok(())
}
