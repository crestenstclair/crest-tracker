//! Shared exact-time arrangement construction for SMF imports.
use super::*;
use crest_tracker::time::Division;

pub(crate) fn arrange(
    mut song: Song,
    parts: Vec<Part>,
    lanes: [Vec<Event>; 16],
    mut identity: u64,
    end: Beat,
) -> Result<Import, String> {
    // Each MIDI channel is one arrangement lane. Source parts still have distinct
    // instrument identities, including tracks reusing a channel in this export.
    // Four-bar Song rows keep the imported arrangement visible. Each placement
    // owns a one-phrase chain; splitting references never quantizes note timing.
    let bars = end.units(Beat::integer(4)).map_err(|e| e.to_string())?
        + u64::from(!end.numerator().is_multiple_of(end.denominator() * 4));
    // A tiny SMF can encode years of silence through large delta times. Bound
    // expansion before allocating empty phrases, independently of source size.
    const MAX_IMPORT_PLACEMENTS: u64 = 65_536;
    let active_lanes = lanes.iter().filter(|events| !events.is_empty()).count() as u64;
    if bars.div_ceil(4).saturating_mul(active_lanes) > MAX_IMPORT_PLACEMENTS {
        return Err("MIDI arrangement exceeds the 65,536-placement import budget".into());
    }
    for (lane, mut events) in lanes.into_iter().enumerate() {
        if events.is_empty() {
            continue;
        }
        events.sort_by_key(|event| (event.at, event.id));
        let names: Vec<_> = parts
            .iter()
            .filter(|part| usize::from(part.channel) == lane)
            .map(|part| part.name.as_str())
            .collect();
        song.tracks[lane].name = names.join(" + ");
        let mut events = events.into_iter().peekable();
        for bar in (0..bars).step_by(4) {
            let at = Beat::integer(bar * 4);
            let length = (bars - bar).min(4) * 4;
            let finish = Beat::integer(bar * 4 + length);
            let mut phrase = Phrase {
                name: format!(
                    "Channel {} · bars {}–{}",
                    lane + 1,
                    bar + 1,
                    bar + length / 4
                ),
                steps: (length * 4) as u32,
                division: Division::SIXTEENTH,
                form: if lane == 9 {
                    PhraseForm::Drum
                } else {
                    PhraseForm::PianoRoll
                },
                ..Phrase::default()
            };
            while events.peek().is_some_and(|event| event.at < finish) {
                let mut event = events.next().expect("peeked event");
                event.at = event.at.checked_sub(at).map_err(|e| e.to_string())?;
                phrase.events.push(event);
            }
            let id = PhraseId::new(identity).map_err(|e| e.to_string())?;
            identity += 1;
            let name = phrase.name.clone();
            song.phrases.insert(id, phrase);
            let usage = PhraseUse {
                id: UseId::new(identity).map_err(|e| e.to_string())?,
                phrase: id,
                repeats: 1,
            };
            identity += 1;
            let chain_id = ChainId::new(identity).map_err(|e| e.to_string())?;
            identity += 1;
            song.chains.insert(
                chain_id,
                Chain {
                    name,
                    phrases: vec![usage],
                },
            );
            song.tracks[lane].slots.push(Some(ChainUse {
                id: UseId::new(identity).map_err(|e| e.to_string())?,
                chain: (chain_id).into(),
                repeats: 1,
            }));
            identity += 1;
        }
        if events.next().is_some() {
            return Err("Event exceeds imported arrangement".into());
        }
    }
    song.validate().map_err(|e| e.to_string())?;
    Ok(Import {
        song,
        parts,
        diagnostics: Diagnostics::default(),
        end,
    })
}
