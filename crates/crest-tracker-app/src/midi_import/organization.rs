//! Musical layout policy for imported MIDI. Plans choose boundaries and names;
//! exact event content remains owned by the deterministic parser.
mod analysis;
#[cfg(test)]
mod tests;
use super::timeline::{normalize_ids, Timeline, MAX_PLACEMENTS};
use super::*;
use crest_tracker::time::Division;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArrangementPlan {
    pub chain_bars: u32,
    pub tracks: Vec<TrackPlan>,
    pub sections: Vec<SectionPlan>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TrackPlan {
    /// One-based source MIDI channel, also the arrangement lane.
    pub channel: u8,
    pub name: String,
    pub phrase_bars: u32,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SectionPlan {
    /// One-based bar number in the source meter map. Sections run to the next
    /// section or the song end. Every bar belongs to exactly one section.
    pub start_bar: u32,
    pub name: String,
    pub reason: String,
}

struct Bar {
    at: Beat,
    end: Beat,
    meter: Meter,
}
struct Bars(Vec<Bar>);
impl Bars {
    fn read(import: &Import) -> Result<Self, String> {
        let mut at = Beat::ZERO;
        let mut bars = vec![];
        let mut changes = import.song.meters.iter().peekable();
        let mut meter = Meter::COMMON;
        while at < import.end {
            if bars.len() >= 16_384 {
                return Err("MIDI arrangement exceeds the 16,384-bar import budget".into());
            }
            while changes.peek().is_some_and(|change| change.at <= at) {
                meter = changes.next().unwrap().meter;
            }
            let end = at
                .checked_add(meter.bar_length())
                .map_err(|e| e.to_string())?;
            bars.push(Bar { at, end, meter });
            at = end;
        }
        Ok(Self(bars))
    }

    fn segment_end(&self, start: usize, limit: usize) -> usize {
        (start + 1..limit)
            .find(|index| self.0[*index].meter != self.0[start].meter)
            .unwrap_or(limit)
    }
}

pub(crate) fn default_plan(import: &Import) -> Result<ArrangementPlan, String> {
    let timeline = Timeline::read(&import.song)?;
    let bars = Bars::read(import)?;
    let tracks = timeline
        .lanes
        .iter()
        .enumerate()
        .filter(|(_, events)| !events.is_empty())
        .map(|(channel, _)| TrackPlan {
            channel: channel as u8 + 1,
            name: if channel == 9 {
                "Drums".into()
            } else {
                format!("Channel {}", channel + 1)
            },
            phrase_bars: if channel == 9 { 1 } else { 2 },
        })
        .collect();
    let section_bars = if bars.0.len() > 8192 { 32 } else { 16 };
    let sections = (0..bars.0.len())
        .step_by(section_bars)
        .enumerate()
        .map(|(index, bar)| SectionPlan {
            start_bar: bar as u32 + 1,
            name: format!("Section {:02}", index + 1),
            reason: format!("Conservative {section_bars}-bar grouping; review musical boundaries"),
        })
        .collect();
    Ok(ArrangementPlan {
        chain_bars: 8,
        tracks,
        sections,
    })
}

pub(crate) fn inspect(import: &Import) -> Result<serde_json::Value, String> {
    analysis::inspect(import, &Timeline::read(&import.song)?, &Bars::read(import)?)
}

fn validate_plan(plan: &ArrangementPlan, timeline: &Timeline, bars: &Bars) -> Result<(), String> {
    if ![4, 8].contains(&plan.chain_bars) {
        return Err("Chain length must be 4 or 8 bars".into());
    }
    let expected: BTreeSet<_> = timeline
        .lanes
        .iter()
        .enumerate()
        .filter(|(_, e)| !e.is_empty())
        .map(|(i, _)| i as u8 + 1)
        .collect();
    let supplied: BTreeSet<_> = plan.tracks.iter().map(|t| t.channel).collect();
    if expected != supplied || supplied.len() != plan.tracks.len() {
        return Err("Arrangement must name every active source channel exactly once".into());
    }
    for track in &plan.tracks {
        valid_label(&track.name)?;
        if ![1, 2, 4].contains(&track.phrase_bars) {
            return Err("Phrase length must be 1, 2 or 4 bars".into());
        }
    }
    if plan.sections.is_empty() || plan.sections.len() > 512 || plan.sections[0].start_bar != 1 {
        return Err("Provide 1–512 ordered sections starting at bar 1".into());
    }
    let mut previous = 0;
    for section in &plan.sections {
        valid_label(&section.name)?;
        if section.reason.trim().is_empty() || section.reason.len() > 2048 {
            return Err("Each section needs a concise evidence-based reason".into());
        }
        if section.start_bar <= previous || section.start_bar as usize > bars.0.len() {
            return Err("Section starts must increase and lie inside the source bar map".into());
        }
        previous = section.start_bar;
    }
    Ok(())
}

pub(crate) fn valid_label(name: &str) -> Result<(), String> {
    if name.trim().is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
        return Err("Import labels require 1–80 characters without control characters".into());
    }
    Ok(())
}

pub(crate) fn organize(mut import: Import, plan: &ArrangementPlan) -> Result<Import, String> {
    let timeline = Timeline::read(&import.song)?;
    let bars = Bars::read(&import)?;
    validate_plan(plan, &timeline, &bars)?;
    import
        .parts
        .retain(|part| timeline.sounding.contains(&part.instrument));
    import
        .song
        .instruments
        .retain(|id, _| timeline.sounding.contains(id));
    import.song.phrases.clear();
    import.song.chains.clear();
    for track in &mut import.song.tracks {
        track.slots.clear();
    }
    let mut builder = Builder::new(import.song);
    for track in &plan.tracks {
        builder.track(
            track,
            plan,
            &bars,
            &timeline.lanes[usize::from(track.channel - 1)],
        )?;
    }
    builder.song.validate().map_err(|e| e.to_string())?;
    if Timeline::read(&builder.song)?.lanes != timeline.lanes {
        return Err("Arrangement changed the expanded MIDI performance".into());
    }
    import.song = builder.song;
    Ok(import)
}

struct Builder {
    song: Song,
    next: u64,
    phrases: BTreeMap<Vec<u8>, PhraseId>,
    chains: BTreeMap<Vec<PhraseId>, ChainId>,
    placements: usize,
}
impl Builder {
    fn new(song: Song) -> Self {
        let next = song
            .instruments
            .keys()
            .map(|id| id.get())
            .max()
            .unwrap_or(16)
            + 1;
        Self {
            song,
            next,
            phrases: BTreeMap::new(),
            chains: BTreeMap::new(),
            placements: 0,
        }
    }
    fn identity(&mut self) -> u64 {
        let id = self.next;
        self.next += 1;
        id
    }

    fn track(
        &mut self,
        track: &TrackPlan,
        plan: &ArrangementPlan,
        bars: &Bars,
        events: &[Event],
    ) -> Result<(), String> {
        self.song.tracks[usize::from(track.channel - 1)].name = track.name.clone();
        let mut lane = Lane {
            plan: track,
            events: events.iter().peekable(),
            motifs: 0,
        };
        for (index, section) in plan.sections.iter().enumerate() {
            let end = plan
                .sections
                .get(index + 1)
                .map_or(bars.0.len(), |s| s.start_bar as usize - 1);
            self.section(
                &mut lane,
                section,
                plan.chain_bars as usize,
                bars,
                section.start_bar as usize - 1..end,
            )?;
        }
        if lane.events.next().is_some() {
            return Err("Arrangement omitted source events".into());
        }
        Ok(())
    }

    fn section(
        &mut self,
        lane: &mut Lane<'_>,
        section: &SectionPlan,
        chain_bars: usize,
        bars: &Bars,
        range: std::ops::Range<usize>,
    ) -> Result<(), String> {
        let mut start = range.start;
        while start < range.end {
            let end = bars.segment_end(start, (start + chain_bars).min(range.end));
            let phrases = self.chain_phrases(lane, bars, start..end)?;
            let name = format!("{} · {}", section.name, lane.plan.name);
            let chain = self.intern_chain(phrases, name)?;
            let id = UseId::new(self.identity()).map_err(|e| e.to_string())?;
            self.song.tracks[usize::from(lane.plan.channel - 1)]
                .slots
                .push(Some(ChainUse {
                    id,
                    chain: chain.into(),
                    repeats: 1,
                }));
            start = end;
        }
        Ok(())
    }

    fn chain_phrases(
        &mut self,
        lane: &mut Lane<'_>,
        bars: &Bars,
        range: std::ops::Range<usize>,
    ) -> Result<Vec<PhraseId>, String> {
        let mut result = vec![];
        for start in (range.start..range.end).step_by(lane.plan.phrase_bars as usize) {
            self.placements += 1;
            if self.placements > MAX_PLACEMENTS {
                return Err("Arrangement exceeds the 65,536-placement budget".into());
            }
            let end = (start + lane.plan.phrase_bars as usize).min(range.end);
            let phrase = lane.take_phrase(bars, start..end)?;
            result.push(self.intern_phrase(phrase, lane)?);
        }
        Ok(result)
    }

    fn intern_phrase(
        &mut self,
        mut phrase: Phrase,
        lane: &mut Lane<'_>,
    ) -> Result<PhraseId, String> {
        let key = serde_json::to_vec(&phrase).map_err(|e| e.to_string())?;
        if let Some(id) = self.phrases.get(&key) {
            return Ok(*id);
        }
        if phrase.events.is_empty() {
            phrase.name = format!("Rest · {} steps", phrase.steps);
        } else {
            lane.motifs += 1;
            phrase.name = format!("{} {:02}", lane.plan.name, lane.motifs);
        }
        let id = PhraseId::new(self.identity()).map_err(|e| e.to_string())?;
        self.phrases.insert(key, id);
        self.song.phrases.insert(id, phrase);
        Ok(id)
    }

    fn intern_chain(&mut self, phrases: Vec<PhraseId>, name: String) -> Result<ChainId, String> {
        if let Some(id) = self.chains.get(&phrases) {
            return Ok(*id);
        }
        // Silent chains can be shared across melodic tracks. A role from the
        // first track would misleadingly label rests in every later track.
        let name = if phrases
            .iter()
            .all(|id| self.song.phrases[id].events.is_empty())
        {
            "Rest".into()
        } else {
            name
        };
        let mut uses = vec![];
        for phrase in &phrases {
            uses.push(PhraseUse {
                id: UseId::new(self.identity()).map_err(|e| e.to_string())?,
                phrase: *phrase,
                repeats: 1,
            });
        }
        let id = ChainId::new(self.identity()).map_err(|e| e.to_string())?;
        self.chains.insert(phrases, id);
        self.song.chains.insert(
            id,
            Chain {
                name,
                phrases: uses,
            },
        );
        Ok(id)
    }
}

/// Sequential source consumption for one channel. A note is placed by attack
/// time; its release and expression lifetime may extend beyond the phrase.
struct Lane<'a> {
    plan: &'a TrackPlan,
    events: std::iter::Peekable<std::slice::Iter<'a, Event>>,
    motifs: usize,
}
impl Lane<'_> {
    fn take_phrase(
        &mut self,
        bars: &Bars,
        range: std::ops::Range<usize>,
    ) -> Result<Phrase, String> {
        let start = &bars.0[range.start];
        let end = bars.0[range.end - 1].end;
        let division = Division::new(16, (start.meter.denominator() / 16).max(1), 1)
            .map_err(|e| e.to_string())?;
        let length = end.checked_sub(start.at).map_err(|e| e.to_string())?;
        let steps = u32::try_from(
            length
                .units(division.beat_length())
                .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let mut phrase = Phrase {
            name: String::new(),
            steps,
            division,
            form: if self.plan.channel == 10 {
                PhraseForm::Drum
            } else {
                PhraseForm::PianoRoll
            },
            ..Phrase::default()
        };
        while self.events.peek().is_some_and(|e| e.at < end) {
            let mut event = self.events.next().expect("peeked source event").clone();
            event.at = event.at.checked_sub(start.at).map_err(|e| e.to_string())?;
            phrase.events.push(event);
        }
        normalize_ids(&mut phrase.events);
        Ok(phrase)
    }
}

pub(crate) fn statistics(song: &Song) -> Result<serde_json::Value, String> {
    let mut chain_placements = 0_u64;
    let mut phrase_placements = 0_u64;
    for usage in song
        .tracks
        .iter()
        .flat_map(|track| track.slots.iter().flatten())
    {
        chain_placements += u64::from(usage.repeats);
        let chain = &song.chains[&usage.chain.pattern().expect("organized pattern")];
        let phrase_count: u64 = chain.phrases.iter().map(|p| u64::from(p.repeats)).sum();
        phrase_placements += phrase_count * u64::from(usage.repeats);
    }
    Ok(serde_json::json!({
        "unique_phrases": song.phrases.len(), "phrase_placements": phrase_placements,
        "reused_phrase_placements": phrase_placements.saturating_sub(song.phrases.len() as u64),
        "unique_chains": song.chains.len(), "chain_placements": chain_placements,
        "reused_chain_placements": chain_placements.saturating_sub(song.chains.len() as u64),
        "multi_phrase_chains": song.chains.values().filter(|c| c.phrases.len() > 1).count(),
        "rest_phrases": song.phrases.values().filter(|p| p.events.is_empty()).count(),
    }))
}
