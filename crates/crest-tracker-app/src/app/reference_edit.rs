//! Held Edit selects existing definitions or creates the trailing new definition.
//! Changes use the same guarded, undoable command path as every other edit.
use super::*;

fn adjacent<T: Copy + PartialEq>(choices: &[T], current: Option<T>, delta: i32) -> Option<T> {
    let index = current
        .and_then(|id| choices.iter().position(|choice| *choice == id))
        .map_or_else(
            || {
                if delta < 0 {
                    choices.len().saturating_sub(1)
                } else {
                    0
                }
            },
            |index| {
                index
                    .saturating_add_signed(delta as isize)
                    .min(choices.len())
            },
        );
    choices.get(index).copied()
}

impl App {
    pub(super) fn adjust_chain_reference(&mut self, delta: i32) -> Result<(), DocumentError> {
        let choices: Vec<_> = self.editor.song().chains.keys().copied().collect();
        let current = self.selected_chain().map(|(id, _)| id);
        let mut track = self.editor.song().tracks[self.track].clone();
        let index = current.and_then(|id| choices.iter().position(|choice| *choice == id));
        let wanted = index.map_or(0, |index| index.saturating_add_signed(delta as isize));
        if let Some(chain) = choices.get(wanted).copied() {
            if current == Some(chain) {
                return Ok(());
            }
            return self.assign_chain(track.id, self.slot, chain);
        }
        // Coarse gestures create the intermediate catalog slots atomically,
        // so 00 + ten is displayed as 10 even in a new Song.
        let count = wanted.saturating_sub(choices.len()) + 1;
        if count > 10 {
            return Err(DocumentError::Invalid(
                "reference gesture exceeds ten slots",
            ));
        }
        let next = self.editor.song().next_identity()?;
        let mut commands = Vec::new();
        for offset in 0..count {
            commands.push(Command::PutChain {
                id: ChainId::new(fresh_identity(next, offset as u64)?)?,
                chain: Chain {
                    name: format!("Chain {:02}", choices.len() + offset),
                    phrases: vec![],
                },
            });
        }
        let usage = Some(ChainUse {
            id: UseId::new(fresh_identity(next, count as u64)?)?,
            chain: ChainId::new(fresh_identity(next, count as u64 - 1)?)?,
            repeats: 1,
        });
        track.ensure_slot(self.slot)?;
        track.slots[self.slot] = usage;
        commands.push(Command::PutTrack { track });
        self.command(Command::Batch { commands })
    }

    pub(super) fn adjust_phrase_reference(&mut self, delta: i32) -> Result<(), DocumentError> {
        let (chain_id, chain) = self
            .selected_chain()
            .ok_or(DocumentError::Invalid("select a chain first"))?;
        let mut chain = chain.clone();
        let row = self.row;
        let current = chain.phrases.get(row).map(|usage| usage.phrase);
        let choices: Vec<_> = self.editor.song().phrases.keys().copied().collect();
        if let Some(phrase) = adjacent(&choices, current, delta) {
            if current == Some(phrase) {
                return Ok(());
            }
            return self.assign_phrase(row, phrase);
        }

        let next = self.editor.song().next_identity()?;
        let phrase = PhraseId::new(next)?;
        if let Some(usage) = chain.phrases.get_mut(row) {
            usage.phrase = phrase;
        } else {
            chain.phrases.push(PhraseUse {
                id: UseId::new(fresh_identity(next, 1)?)?,
                phrase,
                repeats: 1,
            });
        }
        self.command(Command::Batch {
            commands: vec![
                Command::PutPhrase {
                    id: phrase,
                    phrase: Phrase {
                        name: format!("{:02}", self.editor.song().phrases.len()),
                        ..Phrase::default()
                    },
                },
                Command::PutChain {
                    id: chain_id,
                    chain,
                },
            ],
        })?;
        self.chain_row = row;
        self.row = row;
        Ok(())
    }
}
