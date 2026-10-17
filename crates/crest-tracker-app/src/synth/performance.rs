//! Tracker owns project lifecycle and sound bindings around the released library.
mod live;
mod output;
mod playback;
mod surface;
use super::*;
use crest_linnstrument::Geometry;
use crest_performance_core::Limits;
use crest_tracker::performance::Performance;
pub(super) use live::Live;
pub(super) use playback::Playback;
pub(super) use surface::Surface;

pub(super) struct PendingOpen {
    pub document: Document,
    pub path: Option<PathBuf>,
    pub performance: Option<Performance>,
}
impl PendingOpen {
    /// All library validation and allocation precedes sound graph publication.
    fn prepare(document: Document, path: Option<PathBuf>) -> Result<Self, String> {
        let document = document.into_current()?;
        let performance = document
            .performance
            .clone()
            .map(|state| {
                Performance::new(state, Geometry::Pads200, Limits::default())
                    .map_err(|e| e.to_string())
            })
            .transpose()?;
        Ok(Self {
            document,
            path,
            performance,
        })
    }
}

impl Runtime {
    pub(super) fn enable_performance_input(
        &mut self,
        sound: &mut Sound<'_>,
        clock: HostClock,
    ) -> Result<(), String> {
        if self.performance_surface.is_some() {
            return Err(
                "Return the LinnStrument to Play and wait for its acknowledgement first".into(),
            );
        }
        if self.performance_live.is_some() {
            return Ok(());
        }
        if self.capture_baseline.is_some()
            || self.pending_open.is_some()
            || self.loading.is_some()
            || self.sync.external()
        {
            return Err(
                "Finish the current document/take operation and select the local clock first"
                    .into(),
            );
        }
        let revision = sound
            .active_midi_input_identity()
            .ok_or("Select the LinnStrument MIDI input in Settings first")?
            .revision()
            .value();
        let overflow = sound
            .midi_activity()
            .map_or(0, |input| input.overflow_epoch());
        let document = self
            .performance_document()?
            .ok_or("Open a performance session first")?;
        let live = Live::new(
            document,
            &self.app.borrow(),
            sound,
            clock,
            revision,
            overflow,
        )?;
        sound.release_default_input().map_err(|e| e.to_string())?;
        sound.cancel_monitor_schedule().map_err(|e| e.to_string())?;
        self.performance_live = Some(live);
        self.performance = None;
        self.app.borrow_mut().status =
            "LINNSTRUMENT · MPE input enabled · 50 ms monitor lead".into();
        Ok(())
    }

    pub(super) fn record_performance(
        &mut self,
        sound: &mut Sound<'_>,
        clock: HostClock,
        slot: usize,
    ) -> Result<(), String> {
        self.enable_performance_input(sound, clock)?;
        let live = self
            .performance_live
            .as_mut()
            .ok_or("Performance input unavailable")?;
        if live.fault.is_some() {
            return Err("Finish or discard the interrupted take first".into());
        }
        live.engine.arm_recording(slot).map_err(|e| e.to_string())?;
        self.app.borrow_mut().status =
            format!("ARMED · Clip {} · Play the first note to record", slot + 1);
        Ok(())
    }

    pub(super) fn finish_performance_input(
        &mut self,
        sound: &mut Sound<'_>,
        discard: bool,
    ) -> Result<(), String> {
        let Some(mut live) = self.performance_live.take() else {
            return Ok(());
        };
        let canceled = sound.cancel_monitor_schedule().map_err(|e| e.to_string());
        let frame = if live.fault.is_some() {
            None
        } else {
            self.last_clock.map(|clock| clock.audio.rendered_frames())
        };
        if let Err(error) = live.finish(frame, discard) {
            live.fault = Some(error.clone());
            self.performance_live = Some(live);
            return Err(error);
        }
        self.performance = Some(live.engine);
        self.app.borrow_mut().performance_dirty =
            self.performance_document()? != self.saved_performance;
        self.app.borrow_mut().status = if discard {
            "TAKE DISCARDED"
        } else {
            "INPUT STOPPED · Take retained"
        }
        .into();
        canceled.map(|_| ())
    }

    pub(super) fn performance_midi_input(
        &mut self,
        event: crest_synth::control::PhysicalMidiEvent,
        clock: HostClock,
    ) -> bool {
        let Some(live) = &mut self.performance_live else {
            return false;
        };
        self.last_clock = Some(clock);
        let armed = live.engine.armed_slot();
        if let Err(error) = live.accept(event, clock) {
            live.fault = Some(error.clone());
            self.app.borrow_mut().status = error;
        }
        if live.recording() {
            let mut app = self.app.borrow_mut();
            app.performance_dirty = true;
            if let Some(slot) = armed.filter(|_| live.fault.is_none()) {
                app.status = format!("RECORDING · Clip {} · Source timing retained", slot + 1);
            }
        }
        true
    }

    pub(super) fn audition_performance(
        &mut self,
        sound: &mut Sound<'_>,
        clock: HostClock,
        slot: usize,
    ) -> Result<(), String> {
        if self.capture_baseline.is_some() || self.pending_open.is_some() || self.loading.is_some()
        {
            return Err(
                "Finish the current recording or document operation before auditioning".into(),
            );
        }
        if self.sync.external() {
            return Err("Performance audition requires the local host clock".into());
        }
        let document = self
            .performance_document()?
            .ok_or("Open a Song containing a performance session first")?;
        let mut playback = Playback::new(document, slot, &self.app.borrow(), sound, clock)?;
        self.stop(sound)?;
        if let Err(error) = playback.advance(sound, clock, &mut self.voices) {
            let _ = self.stop(sound);
            return Err(error);
        }
        self.performance_playback = Some(playback);
        self.app.borrow_mut().playing = true;
        self.app.borrow_mut().status =
            format!("PERFORMANCE · Auditioning clip {} · Stop to end", slot + 1);
        Ok(())
    }

    pub(super) fn advance_performance(
        &mut self,
        sound: &mut Sound<'_>,
        clock: HostClock,
    ) -> Result<(), String> {
        if let Some(live) = &mut self.performance_live {
            if let Some(error) = &live.fault {
                return Err(error.clone());
            }
            if let Err(error) = live.validate_source(sound).and_then(|()| {
                live.advance(
                    sound,
                    clock,
                    self.app.borrow().editor.song().tempo,
                    &mut self.voices,
                )
            }) {
                live.fault = Some(error.clone());
                return Err(error);
            }
        }
        if let Some(playback) = &mut self.performance_playback {
            if playback.tempo_at_start != self.app.borrow().editor.song().tempo {
                return Err("Tempo changed; restart performance audition at the new tempo".into());
            }
            playback.advance(sound, clock, &mut self.voices)?;
        }
        Ok(())
    }
    pub(super) fn project_performance(&self) {
        self.app.borrow_mut().performance = serde_json::json!({
            "session":self.performance_live.as_ref().map(|live| live.engine.snapshot()).or_else(|| self.performance.as_ref().map(Performance::snapshot)),
            "input":self.performance_live.is_some(),
            "surface":self.performance_surface.as_ref().map(|s| serde_json::json!({"endpoint": s.endpoint, "pads":s.pads(), "status":s.status()})),
            "armed_slot":self.performance_live.as_ref().and_then(|live| live.engine.armed_slot()),
            "input_fault":self.performance_live.as_ref().and_then(|live| live.fault.as_deref()),
            "auditioning":self.performance_playback.is_some(),
            "playing_slot":self.performance_playback.as_ref().map(|playback| playback.slot),
        });
    }
    pub(super) fn performance_document(
        &self,
    ) -> Result<Option<crest_tracker::performance::Document>, String> {
        self.performance_live
            .as_ref()
            .map(|live| &live.engine)
            .or(self.performance.as_ref())
            .map(Performance::document)
            .transpose()
            .map_err(|e| e.to_string())
    }

    /// One composite capture path for normal save, sound import and recovery.
    pub(super) fn capture_document(&self, synth: Value) -> Result<Document, String> {
        let app = self.app.borrow();
        let mut document = Document::new(app.editor.song().clone(), synth, app.mappings.clone());
        document.performance = self.performance_document()?;
        document.pinned_sounds = self.pinned_sounds.clone();
        document.resources = self.resources.clone();
        document.validate()?;
        Ok(document)
    }

    pub(super) fn stage_document(
        &mut self,
        sound: &mut Sound<'_>,
        document: Document,
        path: Option<PathBuf>,
    ) -> Result<(), String> {
        let pending = PendingOpen::prepare(document, path)?;
        let required = pending
            .document
            .installed_patches
            .values()
            .map(|id| PatchId::new(*id).map_err(|e| e.to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        sound
            .request_restore_json_requiring_patches(
                serde_json::to_vec(&pending.document.synth).map_err(|e| e.to_string())?,
                required,
            )
            .map_err(|e| e.to_string())?;
        self.pending_open = Some(pending);
        Ok(())
    }
}

pub(super) fn example_document() -> Result<Document, String> {
    let mut document = super::demo::document()?;
    let instrument = *document
        .song
        .instruments
        .keys()
        .next()
        .ok_or("Example has no instrument")?;
    document.song.name = "Expressive performance example".into();
    let session: crest_performance_core::Session =
        serde_json::from_str(include_str!("../../../../assets/performance-phrase.json"))
            .map_err(|e| e.to_string())?;
    document.performance = Some(crest_tracker::performance::Document {
        version: 1,
        session: session.migrate().map_err(|e| e.to_string())?,
        routes: vec![crest_tracker::performance::Route {
            destination: crest_performance_core::DestinationId(1),
            instrument,
        }],
    });
    document.validate()?;
    Ok(document)
}

/// The released gesture fixture can drive the installed Sample engine through
/// the same MIDI boundary. Its catalog owns the default asset and preparation.
pub(super) fn sample_example_document() -> Result<Document, String> {
    use crest_synth::adapter::sample_capability::{
        SAMPLE_CAPABILITY_ID, SAMPLE_LOOP_FORWARD_CHOICE_ID, SAMPLE_LOOP_MODE_PARAMETER_ID,
    };
    use crest_synth::synth::{
        CapabilityId, CapabilityRegistry, DescriptorDefaultConfigFactory, ParameterId,
        ParameterValue,
    };
    let providers = crest_synth::adapter::production_instruments::production_instrument_providers()
        .map_err(|e| e.to_string())?;
    let registry = CapabilityRegistry::new(
        providers
            .iter()
            .map(|provider| provider.descriptor())
            .collect(),
    )
    .map_err(|e| e.to_string())?;
    let factory = DescriptorDefaultConfigFactory::new(registry, providers);
    let capability = CapabilityId::new(SAMPLE_CAPABILITY_ID).map_err(|e| e.to_string())?;
    let descriptor = factory
        .registry()
        .descriptor(&capability)
        .ok_or("Sample engine is unavailable")?;
    let config = factory
        .create(&capability)
        .map_err(|e| e.to_string())?
        .with_scalar_value(
            descriptor,
            &ParameterId::new(SAMPLE_LOOP_MODE_PARAMETER_ID).map_err(|e| e.to_string())?,
            ParameterValue::Choice(SAMPLE_LOOP_FORWARD_CHOICE_ID.into()),
        )
        .map_err(|e| e.to_string())?;
    let mut document = example_document()?;
    document.song.name = "Expressive Sample performance example".into();
    let instrument = document
        .performance
        .as_ref()
        .ok_or("Example has no performance")?
        .routes[0]
        .instrument;
    let patch_id = document
        .installed_patches
        .get(&instrument)
        .ok_or("Example has no sound binding")?;
    let patch = document.synth["patches"]
        .as_array_mut()
        .ok_or("Example has no Patches")?
        .iter_mut()
        .find(|patch| patch["id"].as_u64() == Some(u64::from(*patch_id)))
        .ok_or("Example sound binding is missing")?;
    patch["instrument"] = serde_json::to_value(config).map_err(|e| e.to_string())?;
    patch["name"] = "Expressive Sample".into();
    let song_instrument = document
        .song
        .instruments
        .get_mut(&instrument)
        .ok_or("Example instrument is missing")?;
    song_instrument.name = "Expressive Sample".into();
    song_instrument.resource = None;
    document.validate()?;
    Ok(document)
}

#[cfg(test)]
mod persistence_tests {
    use super::*;
    use crest_performance_core::{
        self as library, BeatTime, Command, DestinationId, Engine, Session,
    };
    use crest_performance_testkit::{expressive_phrase, CaptureSink};
    use crest_tracker::performance::Route;

    fn document() -> Document {
        let mut engine =
            Engine::new(library::API_VERSION, Session::default(), Limits::default()).unwrap();
        engine.command(Command::BeginRecord { slot: 0 }).unwrap();
        expressive_phrase(
            &mut engine,
            &mut CaptureSink::default(),
            &mut crest_linnstrument::MpeOutput::new(
                DestinationId(1),
                crest_linnstrument::MpeZone::lower(),
            ),
        )
        .unwrap();
        engine.advance(BeatTime(library::TICKS_PER_BEAT)).unwrap();
        engine.command(Command::FinishRecord).unwrap();
        let id = InstrumentId::new(1).unwrap();
        let mut song = Song::default();
        song.instruments.insert(
            id,
            InstrumentUse {
                name: "Expressive patch".into(),
                resource: None,
            },
        );
        let mut document = Document::new(
            song,
            super::super::demo::document().unwrap().synth,
            [(id, 7)].into(),
        );
        document.performance = Some(crest_tracker::performance::Document {
            version: 1,
            session: engine.save_session().unwrap(),
            routes: vec![Route {
                destination: DestinationId(1),
                instrument: id,
            }],
        });
        document
    }

    #[test]
    fn project_worker_roundtrip_retains_released_expression_and_exact_note_times() {
        let mut document = document();
        resources::prepare_save(&mut document, None).unwrap();
        let root =
            std::env::temp_dir().join(format!("crest-performance-save-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("phrase.crest-song");
        let saver = SaveWorker::new();
        saver
            .submit(path.clone(), document.clone(), SaveMode::Normal)
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Some(result) = saver.poll() {
                result.unwrap().result.unwrap();
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "save worker timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let restored = Document::read(&path).unwrap();
        assert_eq!(
            serde_json::to_value(&restored).unwrap(),
            serde_json::to_value(&document).unwrap()
        );
        let prepared = PendingOpen::prepare(restored, Some(path)).unwrap();
        let mut performance = prepared.performance.unwrap();
        performance
            .command(Command::Launch {
                slot: 0,
                quantum: BeatTime(0),
            })
            .unwrap();
        performance
            .advance(Beat::new(740_014, library::TICKS_PER_BEAT).unwrap())
            .unwrap();
        let events: Vec<_> = performance.drain_output().collect();
        assert_eq!(events.len(), 9);
        assert_eq!(events[3].at, BeatTime(240_001));
        assert!(
            matches!(events[3].gesture, library::Gesture::Expression { expression } if expression.pitch == 750_000)
        );
        assert!(
            matches!(events[4].gesture, library::Gesture::Expression { expression } if expression.timbre == 56_762)
        );
        assert!(
            matches!(events[5].gesture, library::Gesture::Expression { expression } if expression.pressure == 46_442)
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_performance_is_rejected_before_replacement_can_be_staged() {
        let mut empty = document();
        let session = &mut empty.performance.as_mut().unwrap().session;
        session.tracks.clear();
        session.slots.clear();
        empty.performance.as_mut().unwrap().routes.clear();
        assert!(matches!(PendingOpen::prepare(empty, None), Err(e) if e.contains("input track")));
        let mut absent = document();
        absent.performance.as_mut().unwrap().routes[0].instrument = InstrumentId::new(99).unwrap();
        assert!(
            matches!(PendingOpen::prepare(absent, None), Err(e) if e.contains("absent Tracker instrument"))
        );
        let mut future = document();
        future.performance.as_mut().unwrap().session.version = 99;
        assert!(PendingOpen::prepare(future, None).is_err());
    }

    #[test]
    fn legacy_song_migrates_in_memory_and_old_versions_cannot_hide_performance() {
        let mut legacy = document();
        legacy.version = 1;
        assert!(legacy.clone().into_current().is_err());
        legacy.performance = None;
        let original = serde_json::to_value(&legacy).unwrap();
        let current = legacy.into_current().unwrap();
        assert_eq!(current.version, 2);
        assert!(current.performance.is_none());
        let mut restored = serde_json::to_value(&current).unwrap();
        restored["version"] = 1.into();
        assert_eq!(restored, original);
        assert_eq!(
            serde_json::to_value(current.clone().into_current().unwrap()).unwrap(),
            serde_json::to_value(current).unwrap()
        );
    }
}
