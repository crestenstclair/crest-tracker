//! Small, evidence-based musical summaries for the planner, not raw MIDI dumps.
use super::*;
use serde_json::{json, Value};

pub(super) fn inspect(import: &Import, timeline: &Timeline, bars: &Bars) -> Result<Value, String> {
    let tracks = timeline
        .lanes
        .iter()
        .enumerate()
        .filter(|(_, events)| !events.is_empty())
        .map(|(lane, events)| analyze_track(lane, events, bars))
        .collect::<Result<Vec<_>, _>>()?;
    let meters: Vec<_> = bars
        .0
        .iter()
        .enumerate()
        .filter(|(index, bar)| *index == 0 || bar.meter != bars.0[index - 1].meter)
        .map(|(index, bar)| json!({"start_bar": index + 1, "meter": bar.meter}))
        .collect();
    Ok(json!({
        "total_bars": bars.0.len(), "meters": meters, "tracks": tracks,
        "pattern_meaning": "Bar patterns compare note pitches, relative onsets and lengths. Velocity and controls are excluded from this musical summary only. Saved phrases reuse only EXACT complete events, including velocity and controls. Pattern 0 means no note attacks, not necessarily silence: held notes and automation can continue.",
        "unused_source_patches": import.parts.len() - timeline.sounding.len(),
    }))
}

fn analyze_track(lane: usize, events: &[Event], bars: &Bars) -> Result<Value, String> {
    let mut patterns = vec![json!({"id": 0, "notes": 0, "pitch_min": null, "pitch_max": null})];
    let mut identities: BTreeMap<Vec<u8>, usize> = BTreeMap::new();
    let mut sequence = vec![];
    let mut index = 0;
    for bar in &bars.0 {
        let end = index + events[index..].partition_point(|event| event.at < bar.end);
        let (key, summary) = note_pattern(&events[index..end], bar)?;
        let pattern = if key.is_empty() {
            0
        } else {
            *identities.entry(key).or_insert_with(|| {
                let id = patterns.len();
                let mut summary = summary;
                summary["id"] = json!(id);
                patterns.push(summary);
                id
            })
        };
        sequence.push(pattern);
        index = end;
    }
    Ok(json!({"channel": lane + 1, "bar_patterns": sequence, "patterns": patterns}))
}

fn note_pattern(events: &[Event], bar: &Bar) -> Result<(Vec<u8>, Value), String> {
    let notes: Vec<_> = events
        .iter()
        .filter_map(|e| e.note.as_ref().map(|note| (e, note)))
        .collect();
    if notes.is_empty() {
        return Ok((vec![], Value::Null));
    }
    let shape = notes
        .iter()
        .map(|(event, note)| {
            event
                .at
                .checked_sub(bar.at)
                .map(|at| (at, note.pitch, note.duration))
                .map_err(|e| e.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let key = serde_json::to_vec(&(bar.meter, shape)).map_err(|e| e.to_string())?;
    Ok((
        key,
        json!({
            "notes": notes.len(),
            "pitch_min": notes.iter().map(|(_, n)| n.pitch).min(),
            "pitch_max": notes.iter().map(|(_, n)| n.pitch).max(),
        }),
    ))
}
