//! Performance checkpoints share the existing durable recovery worker. The
//! library closes only the copied take; the native input engine keeps running.
use super::*;

pub(in crate::synth) struct RecoveryCapture {
    pub slot: usize,
    pub baseline: Document,
}

pub(in crate::synth) struct PendingRecord {
    slot: usize,
    prepared: std::sync::mpsc::Receiver<Result<Document, String>>,
}

impl Runtime {
    pub(in crate::synth) fn prepare_performance_recording(
        &self,
        sound: &Sound<'_>,
        slot: usize,
    ) -> Result<PendingRecord, String> {
        if self.saving
            || self.recovery_preparing.is_some()
            || self.performance_preparing.is_some()
            || self.resource_candidate.is_some()
            || self.adopting.is_some()
            || self.library_read.is_some()
            || self.publishing.is_some()
        {
            return Err("Finish the current save or recovery operation before recording".into());
        }
        if self.recovery_available.is_some()
            || self.recovery_load_error.is_some()
            || self.recovery_loading.is_some()
        {
            return Err("Review the interrupted recording in Settings first".into());
        }
        let synth = serde_json::to_value(sound.capture_saved_session().map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        let mut baseline = self.capture_document(synth)?;
        let (send, prepared) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let result = resources::prepare_save(&mut baseline, None).map(|()| baseline);
            let _ = send.send(result);
        });
        Ok(PendingRecord { slot, prepared })
    }

    /// Admit asset identities before arming. Failed or canceled preparation
    /// leaves the previous recovery checkpoint and committed clips untouched.
    pub(in crate::synth) fn poll_performance_recording(&mut self) -> Result<(), String> {
        let Some(result) = self
            .performance_preparing
            .as_ref()
            .and_then(|pending| poll_once(&pending.prepared))
        else {
            return Ok(());
        };
        let pending = self.performance_preparing.take().expect("pending checked");
        let baseline = result.map_err(|error| format!("Recording setup failed: {error}"))?;
        let live = self
            .performance_live
            .as_mut()
            .ok_or("Performance input stopped during recording setup")?;
        if live.fault.is_some() {
            return Err("Performance input failed during recording setup".into());
        }
        live.engine
            .arm_recording(pending.slot)
            .map_err(|e| e.to_string())?;
        self.resources = baseline.resources.clone();
        self.performance_recovery = Some(RecoveryCapture {
            slot: pending.slot,
            baseline,
        });
        self.app.borrow_mut().status = format!(
            "ARMED · Clip {} · Assets verified · Play the first note to record",
            pending.slot + 1
        );
        Ok(())
    }

    pub(in crate::synth) fn checkpoint_performance(&mut self) -> Result<(), String> {
        let Some(capture) = &self.performance_recovery else {
            return Ok(());
        };
        let mut document = capture.baseline.clone();
        document.performance = Some(match &self.performance_live {
            Some(live) => {
                live.checkpoint(self.last_clock.map(|clock| clock.audio.rendered_frames()))?
            }
            None => self
                .performance
                .as_ref()
                .ok_or("Performance recovery engine unavailable")?
                .checkpoint_document()
                .map_err(|e| e.to_string())?,
        });
        let snapshot = RecoverySnapshot::performance(document, capture.slot)?;
        self.checkpoint_generation = self.recovery_worker.submit(snapshot)?;
        self.checkpoint_dirty = false;
        Ok(())
    }

    /// Finish, discard and failed input all checkpoint the complete committed
    /// library state. Discarding a later take retains earlier unsaved clips.
    pub(in crate::synth) fn finish_performance_recovery(&mut self) -> Result<(), String> {
        if self.performance_recovery.is_none() {
            return Ok(());
        }
        if self.performance_document()? == self.saved_performance {
            return self.clear_recovery();
        }
        self.checkpoint_dirty = true;
        self.checkpoint_performance()
    }

    /// Called only after the composite document and its sound graph activate.
    pub(in crate::synth) fn restore_performance_recovery(
        &mut self,
        snapshot: &RecoverySnapshot,
        baseline: &Document,
    ) -> Result<bool, String> {
        let Some(slot) = snapshot.performance_slot() else {
            return Ok(false);
        };
        snapshot.verify_adoption(baseline, &self.app.borrow().editor)?;
        self.performance_recovery = Some(RecoveryCapture {
            slot,
            baseline: baseline.clone(),
        });
        self.saved_performance = None;
        self.saved_sound = None;
        self.recovery_available = None;
        let mut app = self.app.borrow_mut();
        app.performance_dirty = true;
        app.sound_dirty = true;
        app.status = "RECOVERED PERFORMANCE · Stopped · Save the project to keep it".into();
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crest_performance_core::{BeatTime, Command, Gesture};
    use std::time::{Duration, Instant};

    fn wait_for_write(runtime: &mut Runtime) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(result) = runtime.recovery_worker.poll() {
                let completed = result.unwrap();
                if completed.generation == runtime.checkpoint_generation {
                    return;
                }
            }
            assert!(Instant::now() < deadline, "checkpoint write did not finish");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn failed_asset_preparation_retains_the_previous_recovery_checkpoint() {
        let root = std::env::temp_dir().join(format!(
            "crest-performance-setup-failure-{}",
            uuid::Uuid::new_v4()
        ));
        let app = Rc::new(RefCell::new(App::new()));
        let mut runtime = Runtime::with_data_directory(app, root.clone()).unwrap();
        let baseline = example_document().unwrap();
        runtime.performance = Some(
            Performance::new(
                baseline.performance.clone().unwrap(),
                Geometry::Pads200,
                Limits::default(),
            )
            .unwrap(),
        );
        runtime.performance_recovery = Some(RecoveryCapture {
            slot: 0,
            baseline: baseline.clone(),
        });
        runtime.checkpoint_performance().unwrap();
        wait_for_write(&mut runtime);
        let (send, prepared) = std::sync::mpsc::sync_channel(1);
        runtime.performance_preparing = Some(PendingRecord { slot: 1, prepared });
        send.send(Err("Sample asset missing".into())).unwrap();
        assert_eq!(
            runtime.poll_performance_recording().unwrap_err(),
            "Recording setup failed: Sample asset missing"
        );
        assert!(runtime.performance_preparing.is_none());
        assert_eq!(runtime.performance_recovery.as_ref().unwrap().slot, 0);
        assert_eq!(
            serde_json::to_value(RecoveryWorker::load(&root).unwrap().unwrap().baseline()).unwrap(),
            serde_json::to_value(baseline).unwrap()
        );
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn durable_performance_prefix_and_discard_preserve_preceding_unsaved_clips() {
        let root = std::env::temp_dir().join(format!(
            "crest-performance-checkpoint-{}",
            uuid::Uuid::new_v4()
        ));
        let app = Rc::new(RefCell::new(App::new()));
        let mut runtime = Runtime::with_data_directory(app, root.clone()).unwrap();
        let baseline = example_document().unwrap();
        let document = baseline.performance.clone().unwrap();
        runtime.saved_performance = Some(document.clone());
        runtime.performance =
            Some(Performance::new(document, Geometry::Pads200, Limits::default()).unwrap());
        runtime.performance_recovery = Some(RecoveryCapture {
            slot: 1,
            baseline: baseline.clone(),
        });
        let engine = runtime.performance.as_mut().unwrap();
        engine.command(Command::BeginRecord { slot: 1 }).unwrap();
        engine
            .accept_midi(Beat::ZERO, &[0x91, 60, 100, 0x92, 64, 90, 0x93, 67, 80])
            .unwrap();
        engine
            .accept_midi(Beat::ONE, &[0xe1, 0, 65, 0xb2, 74, 110, 0xd3, 90])
            .unwrap();
        runtime.checkpoint_performance().unwrap();
        wait_for_write(&mut runtime);
        let recovered = RecoveryWorker::load(&root).unwrap().unwrap();
        let prefix = &recovered
            .baseline()
            .performance
            .as_ref()
            .unwrap()
            .session
            .slots[1]
            .clip
            .as_ref()
            .unwrap();
        assert_eq!(prefix.events.len(), 9);
        assert_eq!(prefix.length, BeatTime(960_001));
        assert!(
            matches!(prefix.events[3].gesture, Gesture::Expression { expression } if expression.pitch == 750_000)
        );
        assert!(
            matches!(prefix.events[4].gesture, Gesture::Expression { expression } if expression.timbre == 56_762)
        );
        assert!(
            matches!(prefix.events[5].gesture, Gesture::Expression { expression } if expression.pressure == 46_442)
        );
        assert!(
            runtime.performance_document().is_err(),
            "Checkpoint must leave the take active"
        );
        let engine = runtime.performance.as_mut().unwrap();
        engine.advance(Beat::integer(2)).unwrap();
        engine.command(Command::FinishRecord).unwrap();
        engine.command(Command::Stop).unwrap();
        runtime.finish_performance_recovery().unwrap();
        wait_for_write(&mut runtime);
        let finished = runtime.performance_document().unwrap();
        let mut next_baseline = baseline;
        next_baseline.performance = finished.clone();
        runtime.performance_recovery = Some(RecoveryCapture {
            slot: 2,
            baseline: next_baseline,
        });
        let engine = runtime.performance.as_mut().unwrap();
        engine.command(Command::BeginRecord { slot: 2 }).unwrap();
        engine
            .accept_midi(Beat::integer(2), &[0x91, 72, 100])
            .unwrap();
        engine.command(Command::CancelRecord).unwrap();
        engine.command(Command::Stop).unwrap();
        runtime.finish_performance_recovery().unwrap();
        wait_for_write(&mut runtime);
        let retained = RecoveryWorker::load(&root).unwrap().unwrap();
        assert_eq!(retained.baseline().performance, finished);
        assert!(runtime.performance_recovery.is_some());
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }
}
