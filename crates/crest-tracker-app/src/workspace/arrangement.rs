//! Arrangement operations retain occurrence identities and use document undo.
use super::*;
use crate::app::TransportScope;
use crest_tracker::composition::{Chain, ChainId, ChainRef, ChainUse, SpecialChain, UseId};

pub(super) fn rows(app: &App) -> Vec<WorkspaceRow> {
    let track = &app.editor.song().tracks[app.track];
    let mut rows = vec![
        row("arr_track_name", "Track name", &track.name),
        row("arr_mute", "Track mute", track.muted.to_string()),
        row("arr_loop", "Track loop", track.looped.to_string()),
    ];
    if let Some(usage) = track.slots.get(app.slot).and_then(Option::as_ref) {
        rows.push(row(
            "arr_chain_repeat",
            if usage.chain.special().is_some() {
                "Measures"
            } else {
                "Chain occurrence repeats"
            },
            usage.repeats.to_string(),
        ));
    }
    if let Some((_, chain)) = app.selected_chain() {
        rows.push(row("arr_chain_name", "Chain name", &chain.name));
        if let Some(usage) = chain.phrases.get(app.chain_row) {
            rows.push(row(
                "arr_phrase_repeat",
                "Phrase occurrence repeats",
                usage.repeats.to_string(),
            ));
        }
    }
    rows.extend(commands(&[
        ("arr_move_up", "Move selected occurrence earlier"),
        ("arr_move_down", "Move selected occurrence later"),
        ("arr_remove", "Remove occurrence"),
        ("arr_unique", "Make selected phrase occurrence unique"),
        ("audition_song", "Play Song from start"),
        ("audition_chain", "Play selected Chain"),
        ("audition_phrase", "Play selected Phrase"),
        ("back", "Back"),
    ]));
    rows
}

pub(super) fn slot_rows() -> Vec<WorkspaceRow> {
    commands(&[
        ("slot_kill", "Kill"),
        ("slot_rest", "Rest"),
        ("slot_chain", "Chain"),
    ])
}

fn choose_slot(app: &mut App, id: &str) {
    if id == "slot_chain" && app.selected_chain().is_some() {
        open(app, None);
        return;
    }
    let song = app.editor.song();
    let next = match song.next_identity() {
        Ok(next) => next,
        Err(error) => {
            app.status = error.to_string();
            return;
        }
    };
    let mut commands = Vec::new();
    let chain = match id {
        "slot_rest" => ChainRef::Special(SpecialChain::Rest),
        "slot_kill" => ChainRef::Special(SpecialChain::Kill),
        "slot_chain" => {
            if let Some(chain) = song.chains.keys().next().copied() {
                app.requests.push(Request::AssignChain {
                    track: song.tracks[app.track].id,
                    slot: app.slot,
                    chain,
                });
                open(app, None);
                return;
            }
            let id = ChainId::new(next).expect("fresh identity");
            commands.push(Command::PutChain {
                id,
                chain: Chain {
                    name: format!("{:02}", song.chains.len()),
                    phrases: vec![],
                },
            });
            ChainRef::Pattern(id)
        }
        _ => return,
    };
    let mut track = song.tracks[app.track].clone();
    let previous = track.slots.get(app.slot).and_then(Option::as_ref);
    if previous.is_some_and(|usage| usage.chain == chain) {
        open(app, None);
        return;
    }
    let repeats = previous
        .filter(|usage| usage.chain.special().is_some() && chain.special().is_some())
        .map_or(1, |usage| usage.repeats);
    let Some(identity) = next
        .checked_add(commands.len() as u64)
        .and_then(|id| UseId::new(id).ok())
    else {
        app.status = "identity range exhausted".into();
        return;
    };
    if let Err(error) = track.ensure_slot(app.slot) {
        app.status = error.to_string();
        return;
    }
    track.slots[app.slot] = Some(ChainUse {
        id: identity,
        chain,
        repeats,
    });
    commands.push(Command::PutTrack { track });
    app.requests.push(Request::Edit {
        revision: app.editor.revision(),
        command: Box::new(Command::Batch { commands }),
    });
    open(app, None);
}

pub(super) fn adjust(app: &mut App, field: &str, delta: i32) {
    let mut track = app.editor.song().tracks[app.track].clone();
    let command = match field {
        "arr_mute" => {
            track.muted = !track.muted;
            Command::PutTrack { track }
        }
        "arr_loop" => {
            track.looped = !track.looped;
            Command::PutTrack { track }
        }
        "arr_chain_repeat" => {
            let Some(usage) = track.slots.get_mut(app.slot).and_then(Option::as_mut) else {
                return;
            };
            usage.repeats = usage.repeats.saturating_add_signed(delta).max(1);
            Command::PutTrack { track }
        }
        "arr_phrase_repeat" => {
            let Some((id, chain)) = app.selected_chain() else {
                return;
            };
            let mut chain = chain.clone();
            let Some(usage) = chain.phrases.get_mut(app.chain_row) else {
                return;
            };
            usage.repeats = usage.repeats.saturating_add_signed(delta).max(1);
            Command::PutChain { id, chain }
        }
        _ => return,
    };
    app.requests.push(Request::Edit {
        revision: app.editor.revision(),
        command: Box::new(command),
    });
}

pub(super) fn activate(app: &mut App, id: &str) -> bool {
    if id.starts_with("slot_") {
        choose_slot(app, id);
        return true;
    }
    if let Some(scope) = match id {
        "audition_song" => Some(TransportScope::Song),
        "audition_chain" => Some(TransportScope::Chain),
        "audition_phrase" => Some(TransportScope::Phrase),
        _ => None,
    } {
        app.requests.push(Request::StartAt {
            scope,
            at: Beat::ZERO,
        });
        return true;
    }
    if !id.starts_with("arr_") {
        return false;
    }
    match id {
        "arr_track_name" => text_entry(
            app,
            id,
            "Track name",
            &app.editor.song().tracks[app.track].name.clone(),
        ),
        "arr_chain_name" => {
            if let Some((_, chain)) = app.selected_chain() {
                let name = chain.name.clone();
                text_entry(app, id, "Chain name", &name);
            }
        }
        "arr_unique" => {
            if let Some(occurrence) = app.occurrence() {
                app.requests.push(Request::Edit {
                    revision: app.editor.revision(),
                    command: Box::new(Command::MakeUnique { occurrence }),
                });
            }
        }
        "arr_remove" | "arr_move_up" | "arr_move_down" => reorder(app, id),
        _ => adjust(app, id, 1),
    }
    true
}
fn reorder(app: &mut App, id: &str) {
    let command = if app.view == View::Song || app.view == View::Minimap {
        let mut track = app.editor.song().tracks[app.track].clone();
        if !move_or_remove(&mut track.slots, app.slot, id) {
            return;
        }
        Command::PutTrack { track }
    } else {
        let Some((id_chain, chain)) = app.selected_chain() else {
            return;
        };
        let mut chain = chain.clone();
        if !move_or_remove(&mut chain.phrases, app.chain_row, id) {
            return;
        }
        Command::PutChain {
            id: id_chain,
            chain,
        }
    };
    app.requests.push(Request::Edit {
        revision: app.editor.revision(),
        command: Box::new(command),
    });
}
fn move_or_remove<T>(items: &mut Vec<T>, index: usize, id: &str) -> bool {
    if index >= items.len() {
        return false;
    }
    if id == "arr_remove" {
        items.remove(index);
    } else {
        let next = if id == "arr_move_up" {
            index.saturating_sub(1)
        } else {
            (index + 1).min(items.len() - 1)
        };
        items.swap(index, next);
    }
    true
}
