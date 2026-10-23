use super::*;

pub(super) const PREVIEW_EVENT_LIMIT: usize = 8192;

/// Locate a bar by meter segments, without scanning every preceding bar.
pub(super) fn bar_at(song: &Song, position: Beat) -> Result<BarMarker, DocumentError> {
    let mut number = 1_u64;
    for (index, change) in song.meters.iter().enumerate() {
        let next = song.meters.get(index + 1).map(|change| change.at);
        let until = next.unwrap_or(position).min(position);
        let bars = until
            .checked_sub(change.at)?
            .units(change.meter.bar_length())?;
        if next.is_none_or(|next| position < next) {
            return Ok(BarMarker {
                at: change
                    .at
                    .checked_add(change.meter.bar_length().checked_scale(bars, 1)?)?,
                number: u32::try_from(
                    number
                        .checked_add(bars)
                        .ok_or(DocumentError::Invalid("bar number overflow"))?,
                )
                .map_err(|_| DocumentError::Invalid("bar number overflow"))?,
                meter: change.meter,
            });
        }
        number = number
            .checked_add(bars)
            .ok_or(DocumentError::Invalid("bar number overflow"))?;
    }
    Err(DocumentError::Invalid("meter map is empty"))
}

pub(super) fn linear_chain_length(
    song: &Song,
    chain: &Chain,
) -> Result<Option<Beat>, DocumentError> {
    let mut length = Beat::ZERO;
    for usage in &chain.phrases {
        let phrase = &song.phrases[&usage.phrase];
        if conditional_phrase(phrase) {
            return Ok(None);
        }
        length = length.checked_add(
            phrase
                .length()?
                .checked_scale(u64::from(usage.repeats), 1)?,
        )?;
    }
    Ok((!length.is_zero()).then_some(length))
}

fn conditional_phrase(phrase: &Phrase) -> bool {
    phrase.events.len() > PREVIEW_EVENT_LIMIT
        || phrase
            .events
            .iter()
            .flat_map(|event| event.fx.iter().flatten())
            .any(|fx| matches!(fx, Fx::Jump { .. }))
}

pub(super) fn linear_track_length(
    song: &Song,
    track: &Track,
) -> Result<Option<Beat>, DocumentError> {
    let mut length = Beat::ZERO;
    for usage in track.slots.iter().flatten() {
        if usage.chain.special().is_some() {
            return Ok(None);
        }
        let Some(chain) = linear_chain_length(song, &song.chains[&usage.chain.pattern()?])? else {
            return Ok(None);
        };
        length = length.checked_add(chain.checked_scale(u64::from(usage.repeats), 1)?)?;
    }
    Ok((!length.is_zero()).then_some(length))
}

pub(super) fn skip_repeats(
    at: &mut Beat,
    start: Beat,
    length: Beat,
    repeats: u32,
) -> Result<u32, DocumentError> {
    if *at >= start {
        return Ok(0);
    }
    let skip = start
        .checked_sub(*at)?
        .units(length)?
        .min(u64::from(repeats));
    *at = at.checked_add(length.checked_scale(skip, 1)?)?;
    Ok(skip as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crest_tracker::time::Meter;
    #[test]
    fn later_windows_skip_large_repeat_counts_and_follow_meter_segments() {
        let mut app = App::new();
        app.apply(Request::Confirm).unwrap();
        app.apply(Request::Confirm).unwrap();
        let mut song = app.editor.song().clone();
        song.tracks[0].looped = true;
        song.meters = vec![
            MeterChange {
                at: Beat::ZERO,
                meter: Meter::new(3, 4).unwrap(),
            },
            MeterChange {
                at: Beat::integer(6),
                meter: Meter::new(5, 8).unwrap(),
            },
        ];
        app.editor = Editor::new(song.clone()).unwrap();
        app.apply(Request::View {
            view: View::Minimap,
        })
        .unwrap();
        app.apply(Request::PreviewBars { bars: 8 }).unwrap();
        app.apply(Request::PreviewStart {
            at: Beat::integer(100_006),
        })
        .unwrap();
        let timeline = app.timeline().unwrap();
        assert_eq!(timeline.start, Beat::integer(100_006));
        assert_eq!(timeline.end, Beat::integer(100_026));
        assert_eq!(timeline.bars[0].number, 40_003);
        assert!(!timeline.lanes[0].spans.is_empty());
        assert!(timeline.lanes[0]
            .spans
            .iter()
            .all(|span| span.end > timeline.start));
        assert!(timeline.lanes[0].spans.len() < 10);
        assert_eq!(app.editor.song(), &song);
    }
}
