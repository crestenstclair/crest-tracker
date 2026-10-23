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
    if usage.chain.special().is_none() {
        usage.repeats = 1;
    }
    if let Some(phrase_use) = phrase_use {
        let chain = song
            .chains
            .get_mut(&usage.chain.pattern()?)
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
    /// Resume an internal producer from its first undelivered beat. Everything
    /// before the horizon has already reached audio; everything after it still
    /// belongs to Playback, including releases, expression and send lifetimes.
    /// Move that remaining schedule together instead of bursting overdue notes
    /// or canceling voices. The audible result of a stall is a delay, not Stop.
    pub(super) fn recover_audio_deadline(
        &mut self,
        playback: &Playback,
        clock: HostClock,
    ) -> Result<(), String> {
        if self.sync.external()
            || playback.finished()
            || self.frame_at_beat(playback.horizon(), clock)? >= clock.audio.rendered_frames()
        {
            return Ok(());
        }
        let anchor = clock
            .audio
            .rendered_frames()
            .checked_add(u64::from(clock.sample_rate) / 20)
            .ok_or("Audio frame overflow")?;
        let offset = self
            .tempo
            .frame_at(playback.horizon(), clock.sample_rate)
            .map_err(|error| error.to_string())?;
        // Input timestamps captured against the old timeline cannot safely be
        // reinterpreted against the new anchor. Retain the take, but keep music
        // running. Ordinary playback does not enter this path.
        let capturing = !matches!(
            self.recorder.state(),
            CaptureState::Idle | CaptureState::Interrupted { .. }
        );
        self.interrupt_capture("Audio scheduling delayed; take retained; playback resumed")?;
        self.voices.recover_expression(&self.expression);
        self.anchor = anchor;
        self.transport_offset_frames = offset;
        self.playback_recoveries = self.playback_recoveries.saturating_add(1);
        self.app.borrow_mut().status = if capturing {
            "PLAYING · Timing recovered · Recording interrupted; take retained"
        } else {
            "PLAYING · Timing recovered"
        }
        .into();
        Ok(())
    }

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
    fn deadline_recovery_preserves_pending_events_tempo_and_retains_recording() {
        use crest_synth::real_time::AudioObservationSnapshot;
        use crest_tracker::time::Tempo;
        let mut runtime = super::super::tests::capture_fixture();
        let mut song = runtime.app.borrow().editor.song().clone();
        let id = EventId::new(song.next_identity().unwrap()).unwrap();
        song.phrases
            .values_mut()
            .next()
            .unwrap()
            .events
            .push(Event::note(
                id,
                Beat::ZERO,
                runtime.capture_target.clone().unwrap(),
                Note::new(60, 100, Beat::integer(3)).unwrap(),
            ));
        let snapshot = Snapshot::compile(&song, 0, PlaybackBudget::default()).unwrap();
        let mut playback = Playback::new(Arc::new(snapshot));
        playback.window(Beat::ONE).unwrap();
        let pending = playback.clone().window(Beat::integer(10)).unwrap();
        assert!(pending
            .iter()
            .any(|event| matches!(event.message, Message::NoteOff { .. })));
        runtime.tempo = TempoMap::new(Tempo::new(120_000).unwrap());
        runtime.tempo.insert(Beat::ONE, Tempo::new(60_000).unwrap());
        runtime
            .tempo
            .insert(Beat::integer(2), Tempo::new(180_000).unwrap());
        runtime.anchor = 100;
        let mut clock = super::super::tests::clock();
        clock.audio = AudioObservationSnapshot::from_mix(1, 1, 48_000, 0, 0, 0, Default::default());
        runtime.last_clock = Some(clock);
        runtime.recover_audio_deadline(&playback, clock).unwrap();
        assert_eq!(runtime.playback_recoveries, 1);
        assert!(matches!(
            runtime.recorder.state(),
            CaptureState::Interrupted { .. }
        ));
        assert!(runtime.app.borrow().status.contains("take retained"));
        assert_eq!(runtime.transport_beat(clock).unwrap(), Beat::ONE);
        assert_eq!(runtime.frame_at_beat(Beat::ONE, clock).unwrap(), 50_400);
        assert_eq!(
            runtime.frame_at_beat(Beat::integer(2), clock).unwrap(),
            98_400
        );
        assert_eq!(
            runtime.frame_at_beat(Beat::integer(3), clock).unwrap(),
            114_400
        );
        runtime.recover_audio_deadline(&playback, clock).unwrap();
        assert_eq!(
            runtime.playback_recoveries, 1,
            "no repeated recovery while ahead"
        );
        assert_eq!(playback.window(Beat::integer(10)).unwrap(), pending);
        clock.audio =
            AudioObservationSnapshot::from_mix(2, 2, 480_000, 0, 0, 0, Default::default());
        runtime.recover_audio_deadline(&playback, clock).unwrap();
        assert_eq!(
            runtime.playback_recoveries, 1,
            "finished playback must end normally"
        );
    }

    #[test]
    fn scoped_audition_preserves_arrangement_and_seek_rebuilds_tempo_without_stale_releases() {
        let mut app = App::new();
        app.apply(Request::Confirm).unwrap();
        app.apply(Request::Confirm).unwrap();
        let path = app.occurrence().unwrap();
        let mut song = app.editor.song().clone();
        let chain = song.tracks[0].slots[0]
            .as_ref()
            .unwrap()
            .chain
            .pattern()
            .unwrap();
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
