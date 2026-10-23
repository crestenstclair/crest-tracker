//! Bounded, identity-independent performance of a linear imported Song.
use super::*;
use std::collections::BTreeSet;

pub(super) const MAX_PLACEMENTS: usize = 65_536;
const MAX_EVENTS: usize = 1_000_000;

pub(crate) struct Timeline {
    pub lanes: [Vec<Event>; 16],
    pub sounding: BTreeSet<InstrumentId>,
}

impl Timeline {
    /// Ignore controls addressed only to never-sounding patches. The importer
    /// already snapshots those channel controls onto every real patch at creation.
    /// Tempo is global, so retain it even when its original target was unused.
    pub fn read(song: &Song) -> Result<Self, String> {
        song.validate().map_err(|e| e.to_string())?;
        let mut lanes = std::array::from_fn(|_| vec![]);
        let mut budget = ExpansionBudget::default();
        for (index, track) in song.tracks.iter().enumerate() {
            lanes[index] = expand_track(song, track, &mut budget)?;
        }
        let sounding: BTreeSet<_> = lanes
            .iter()
            .flatten()
            .filter_map(sounding_instrument)
            .collect();
        let target = lanes
            .iter()
            .flatten()
            .find(|e| e.note.is_some())
            .ok_or("MIDI performance contains no notes")?
            .target
            .clone();
        for lane in &mut lanes {
            lane.retain_mut(|event| retain_sounding_event(event, &sounding, &target));
            // Stable sorting preserves same-tick control order across occurrences.
            lane.sort_by_key(|event| event.at);
            normalize_ids(lane);
        }
        Ok(Self { lanes, sounding })
    }

    pub fn note_count(&self) -> usize {
        self.lanes
            .iter()
            .flatten()
            .filter(|e| e.note.is_some())
            .count()
    }
}

fn sounding_instrument(event: &Event) -> Option<InstrumentId> {
    match (&event.target, &event.note) {
        (Target::Instrument { id, .. }, Some(_)) => Some(*id),
        _ => None,
    }
}

pub(super) fn retain_sounding_event(
    event: &mut Event,
    sounding: &BTreeSet<InstrumentId>,
    fallback: &Target,
) -> bool {
    let Target::Instrument { id, .. } = event.target else {
        return true;
    };
    if sounding.contains(&id) {
        return true;
    }
    if event
        .fx
        .iter()
        .flatten()
        .any(|fx| matches!(fx, Fx::Tempo { .. }))
    {
        event.controls.clear();
        event.target = fallback.clone();
        return true;
    }
    false
}

pub(super) fn normalize_ids(events: &mut [Event]) {
    for (index, event) in events.iter_mut().enumerate() {
        event.id = EventId::new(index as u64 + 1).expect("bounded nonzero event identity");
    }
}

#[derive(Default)]
struct ExpansionBudget {
    placements: usize,
    events: usize,
}
impl ExpansionBudget {
    fn admit(&mut self, events: usize) -> Result<(), String> {
        self.placements += 1;
        self.events = self.events.saturating_add(events);
        if self.placements > MAX_PLACEMENTS || self.events > MAX_EVENTS {
            return Err("MIDI performance exceeds the bounded expansion budget".into());
        }
        Ok(())
    }
}

fn expand_track(
    song: &Song,
    track: &Track,
    budget: &mut ExpansionBudget,
) -> Result<Vec<Event>, String> {
    if track.looped || track.muted {
        return Err("Import organization requires unmuted, finite tracks".into());
    }
    let mut at = Beat::ZERO;
    let mut events = vec![];
    for usage in track.slots.iter().flatten() {
        expand_chain(song, usage, &mut at, &mut events, budget)?;
    }
    Ok(events)
}

fn expand_chain(
    song: &Song,
    usage: &ChainUse,
    at: &mut Beat,
    events: &mut Vec<Event>,
    budget: &mut ExpansionBudget,
) -> Result<(), String> {
    let id = usage
        .chain
        .pattern()
        .map_err(|_| "Import organization cannot rewrite Rest/Kill placements")?;
    if usage.repeats as usize > MAX_PLACEMENTS || song.chains[&id].phrases.is_empty() {
        return Err("MIDI chain repeats exceed the expansion budget".into());
    }
    for _ in 0..usage.repeats {
        expand_phrases(song, &song.chains[&id], at, events, budget)?;
    }
    Ok(())
}

fn expand_phrases(
    song: &Song,
    chain: &Chain,
    at: &mut Beat,
    events: &mut Vec<Event>,
    budget: &mut ExpansionBudget,
) -> Result<(), String> {
    for usage in &chain.phrases {
        expand_phrase(
            &song.phrases[&usage.phrase],
            usage.repeats,
            at,
            events,
            budget,
        )?;
    }
    Ok(())
}

fn expand_phrase(
    phrase: &Phrase,
    repeats: u32,
    at: &mut Beat,
    output: &mut Vec<Event>,
    budget: &mut ExpansionBudget,
) -> Result<(), String> {
    if !matches!(
        phrase.form,
        PhraseForm::Tracker | PhraseForm::PianoRoll | PhraseForm::Drum
    ) {
        return Err("Import organization cannot rewrite generated phrases".into());
    }
    let mut events = phrase.events.clone();
    events.sort_by_key(|e| (e.at, e.id));
    for _ in 0..repeats {
        budget.admit(events.len())?;
        append_events(&events, *at, output)?;
        *at = at
            .checked_add(phrase.length().map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn append_events(events: &[Event], at: Beat, output: &mut Vec<Event>) -> Result<(), String> {
    for event in events {
        if event
            .fx
            .iter()
            .flatten()
            .any(|fx| !matches!(fx, Fx::Tempo { .. }))
        {
            return Err(
                "Import organization accepts only literal MIDI events and tempo commands".into(),
            );
        }
        let mut event = event.clone();
        event.at = at.checked_add(event.at).map_err(|e| e.to_string())?;
        output.push(event);
    }
    Ok(())
}
