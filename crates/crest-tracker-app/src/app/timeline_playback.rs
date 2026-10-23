//! Map delivered source positions onto arrangement previews without guessing
//! musical time from a lane's phrase-generation counter.
use super::*;
use crest_tracker::playback::AudiblePlayhead;

impl App {
    pub(super) fn timeline_playheads(&self) -> Value {
        if !self.playing || !matches!(self.view, View::Expanded | View::Minimap) {
            return json!([]);
        }
        let Ok(timeline) = self.timeline() else {
            return json!([]);
        };
        let heads: Vec<_> = timeline
            .lanes
            .iter()
            .filter_map(|lane| {
                let head = self
                    .play_heads
                    .iter()
                    .find(|head| head.occurrence.track == lane.track)?;
                // Pending edits can change structure before the audible snapshot
                // adopts it. Never paint the old cursor onto newly edited music.
                if head.revision != timeline.revision {
                    return None;
                }
                let arrangement_time = self.view == View::Minimap
                    && matches!(self.playback_scope, TransportScope::Song)
                    && self.playback_loop != Some(head.occurrence);
                project_head(lane, head, &timeline, arrangement_time)
            })
            .collect();
        json!(heads)
    }
}

fn project_head(
    lane: &TimelineLane,
    head: &AudiblePlayhead,
    timeline: &Timeline,
    arrangement_time: bool,
) -> Option<Value> {
    let span = lane.spans.iter().find(|span| {
        span.occurrence == head.occurrence
            && span.phrase == head.phrase
            && span.repeat == head.phrase_repeat
            && span.chain_repeat == head.chain_repeat
            && (!arrangement_time
                || span.conditional
                || (span.start <= head.at && head.at < span.end))
    })?;
    let at = span.start.checked_add(head.local_beat).ok()?;
    if at < timeline.start || at >= timeline.end || at >= span.end {
        return None;
    }
    let row = span.rows.iter().rev().find(|row| row.at <= at);
    Some(
        json!({"occurrence":head.occurrence,"start":span.start,"at":at,
        "rowAt":row.map(|row| row.at),"row":head.row,"phrase":head.phrase,
        "phraseRepeat":head.phrase_repeat,"chainRepeat":head.chain_repeat,
        "conditional":span.conditional,"sourceTime":!arrangement_time}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::demo;

    fn app() -> App {
        let mut app = App::new();
        let target = Target::Channel {
            channel: Channel::FIRST,
            endpoint: None,
        };
        let command =
            demo::arrangement(app.editor.song(), std::array::from_fn(|_| target.clone())).unwrap();
        app.command(command).unwrap();
        app.playing = true;
        app
    }
    fn advance(app: &mut App, at: Beat) {
        let snapshot = Snapshot::compile(
            app.editor.song(),
            app.editor.revision(),
            PlaybackBudget::default(),
        )
        .unwrap();
        let mut playback = Playback::new(Arc::new(snapshot));
        playback.window(at.checked_add(Beat::ONE).unwrap()).unwrap();
        app.play_heads = playback.playheads_at(at).unwrap().unwrap();
    }
    #[test]
    fn minimap_keeps_later_phrases_and_independent_track_loops_visible() {
        let mut app = app();
        app.view = View::Minimap;
        advance(&mut app, Beat::integer(9));
        let heads = app.timeline_playheads();
        assert_eq!(heads.as_array().unwrap().len(), 5);
        assert!(heads
            .as_array()
            .unwrap()
            .iter()
            .all(|head| head["at"] == json!(Beat::integer(9))));
        assert!(app.play_heads.iter().any(|head| head.cycle > 0));
        advance(&mut app, Beat::integer(37));
        let heads = app.timeline_playheads();
        assert_eq!(heads.as_array().unwrap().len(), 5);
        assert!(heads
            .as_array()
            .unwrap()
            .iter()
            .all(|head| head["at"] == json!(Beat::integer(37))));
    }
    #[test]
    fn expanded_uses_repeat_identity_and_retains_each_lanes_local_rows() {
        let mut app = app();
        let usage = app.editor.song().tracks[1].slots[0]
            .as_ref()
            .unwrap()
            .clone();
        let mut song = app.editor.song().clone();
        song.chains
            .get_mut(&usage.chain.pattern().unwrap())
            .unwrap()
            .phrases[0]
            .repeats = 2;
        song.tracks[1].slots[0].as_mut().unwrap().repeats = 2;
        app.editor = Editor::new(song).unwrap();
        app.expanded_selection = app.editor.song().tracks[..2]
            .iter()
            .map(|track| ChainSelection {
                track: track.id,
                chain_use: track.slots[0].as_ref().unwrap().id,
            })
            .collect();
        app.view = View::Expanded;
        advance(&mut app, Beat::integer(9));
        let heads = app.timeline_playheads();
        assert_eq!(heads.as_array().unwrap().len(), 2);
        assert_eq!(heads[1]["phraseRepeat"], 1);
        assert_eq!(heads[1]["start"], json!(Beat::integer(8)));
        assert_eq!(heads[1]["rowAt"], json!(Beat::integer(9)));
        advance(&mut app, Beat::integer(25));
        let heads = app.timeline_playheads();
        assert_eq!(heads[1]["chainRepeat"], 1);
        assert_eq!(heads[1]["phraseRepeat"], 0);
    }
    #[test]
    fn audition_and_record_loop_use_source_time_and_pending_revisions_hide() {
        let mut app = app();
        app.view = View::Minimap;
        advance(&mut app, Beat::integer(9));
        let head = &mut app.play_heads[0];
        let occurrence = head.occurrence;
        head.at = Beat::ZERO;
        app.playback_loop = Some(occurrence);
        let heads = app.timeline_playheads();
        assert_eq!(heads[0]["sourceTime"], true);
        assert_eq!(heads[0]["at"], json!(Beat::integer(9)));
        assert_eq!(heads[1]["sourceTime"], false);
        app.playback_loop = None;
        app.playback_scope = TransportScope::Phrase;
        assert_eq!(app.timeline_playheads()[0]["sourceTime"], true);
        app.play_heads[0].revision += 1;
        assert_eq!(app.timeline_playheads().as_array().unwrap().len(), 4);
        app.playing = false;
        assert_eq!(app.timeline_playheads(), json!([]));
    }
}
