//! Audition selections are playback projections, never document edits.
use super::*;
use crate::app::TransportScope;

#[derive(Clone, Copy, Debug)]
pub(super) enum PlaybackSelection {
    Song,
    Chain { track: TrackId, chain_use: UseId },
    Phrase(Occurrence),
}

impl PlaybackSelection {
    pub fn from_app(app: &App, scope: TransportScope) -> Result<Self, String> {
        match scope {
            TransportScope::Song => Ok(Self::Song),
            TransportScope::Phrase => app
                .occurrence()
                .map(Self::Phrase)
                .ok_or("Select a phrase to audition".into()),
            TransportScope::Chain => {
                let track = &app.editor.song().tracks[app.track];
                let usage = track
                    .slots
                    .get(app.slot)
                    .and_then(Option::as_ref)
                    .ok_or("Select a chain to audition")?;
                Ok(Self::Chain {
                    track: track.id,
                    chain_use: usage.id,
                })
            }
        }
    }

    pub fn snapshot(self, app: &App) -> Result<Snapshot, crest_tracker::playback::PlaybackError> {
        let mut song = app.editor.song().clone();
        match self {
            Self::Song => {}
            Self::Chain { track, chain_use } => scope_song(&mut song, track, chain_use, None)?,
            Self::Phrase(path) => {
                scope_song(&mut song, path.track, path.chain_use, Some(path.phrase_use))?
            }
        }
        Snapshot::compile(&song, app.editor.revision(), PlaybackBudget::default())
    }
}

fn scope_song(
    song: &mut Song,
    track: TrackId,
    chain_use: UseId,
    phrase_use: Option<UseId>,
) -> Result<(), DocumentError> {
    song.tracks.retain(|lane| lane.id == track);
    let track = song
        .tracks
        .first_mut()
        .ok_or(DocumentError::Invalid("audition track no longer exists"))?;
    track
        .slots
        .retain(|slot| slot.as_ref().is_some_and(|usage| usage.id == chain_use));
    let usage = track
        .slots
        .first_mut()
        .and_then(Option::as_mut)
        .ok_or(DocumentError::Invalid("audition chain no longer exists"))?;
    track.looped = false;
    // Audition is explicit even when the arrangement lane is muted.
    track.muted = false;
    usage.repeats = 1;
    if let Some(phrase_use) = phrase_use {
        let chain = song
            .chains
            .get_mut(&usage.chain)
            .ok_or(DocumentError::Invalid(
                "audition chain definition is missing",
            ))?;
        chain.phrases.retain(|usage| usage.id == phrase_use);
        let phrase = chain
            .phrases
            .first_mut()
            .ok_or(DocumentError::Invalid("audition phrase no longer exists"))?;
        phrase.repeats = 1;
    }
    Ok(())
}

impl Runtime {
    pub(super) fn start_at(
        &mut self,
        sound: &mut Sound<'_>,
        clock: HostClock,
        scope: TransportScope,
        at: Beat,
    ) -> Result<(), String> {
        let selection = PlaybackSelection::from_app(&self.app.borrow(), scope)?;
        if self.capture_preparing.is_some()
            || self.pending_open.is_some()
            || self.loading.is_some()
            || self.recovery_preparing.is_some()
            || self.adopting.is_some()
        {
            return Err("Wait for document or recording preparation before seeking".into());
        }
        self.play_from(sound, clock, selection, at, None)
    }

    /// Convert elapsed device frames through the rebuilt tempo map. Keeping an
    /// explicit offset avoids unsigned underflow when seeking past device uptime.
    pub(super) fn musical_frame(&self, frame: u64) -> Result<u64, String> {
        frame
            .saturating_sub(self.anchor)
            .checked_add(self.transport_offset_frames)
            .ok_or("Transport frame overflow".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_audition_preserves_arrangement_and_seek_rebuilds_tempo_without_stale_releases() {
        let mut app = App::new();
        app.apply(Request::Confirm).unwrap();
        app.apply(Request::Confirm).unwrap();
        let path = app.occurrence().unwrap();
        let mut song = app.editor.song().clone();
        let chain = song.tracks[0].slots[0].as_ref().unwrap().chain;
        let phrase = song.chains[&chain].phrases[0].phrase;
        let target = Target::Channel {
            channel: Channel::FIRST,
            endpoint: None,
        };
        let mut first = Event::note(
            EventId::new(song.next_identity().unwrap()).unwrap(),
            Beat::ZERO,
            target.clone(),
            Note::new(60, 100, Beat::new(1, 2).unwrap()).unwrap(),
        );
        first.fx[0] = Some(Fx::Tempo {
            tempo: crest_tracker::time::Tempo::new(60_000).unwrap(),
        });
        song.phrases.get_mut(&phrase).unwrap().events.push(first);
        let next = EventId::new(song.next_identity().unwrap()).unwrap();
        song.phrases
            .get_mut(&phrase)
            .unwrap()
            .events
            .push(Event::note(
                next,
                Beat::ONE,
                target,
                Note::new(64, 100, Beat::new(1, 4).unwrap()).unwrap(),
            ));
        song.tracks[0].looped = true;
        song.tracks[0].muted = true;
        app.editor = Editor::new(song.clone()).unwrap();
        let snapshot = PlaybackSelection::Phrase(path).snapshot(&app).unwrap();
        let mut playback = Playback::new(Arc::new(snapshot));
        let tempos = playback.seek_forward_tempos(Beat::ONE).unwrap();
        assert_eq!(
            tempos,
            vec![(Beat::ZERO, crest_tracker::time::Tempo::new(60_000).unwrap())]
        );
        let events = playback.window(Beat::integer(100)).unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event.message, Message::NoteOn { .. }))
                .count(),
            1
        );
        assert!(events.iter().all(|event| event.at >= Beat::ONE));
        assert!(events.iter().all(|event| event.owner.event == next));
        assert!(playback.finished());
        assert_eq!(app.editor.song(), &song);
        let chain_snapshot = PlaybackSelection::Chain {
            track: path.track,
            chain_use: path.chain_use,
        }
        .snapshot(&app)
        .unwrap();
        let mut chain_playback = Playback::new(Arc::new(chain_snapshot));
        assert_eq!(
            chain_playback
                .window(Beat::integer(100))
                .unwrap()
                .iter()
                .filter(|event| matches!(event.message, Message::NoteOn { .. }))
                .count(),
            2
        );
        assert!(chain_playback.finished());
    }
}
