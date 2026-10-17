//! Retrospective capture is a committed clip edit; monitoring and playback keep
//! their clocks and owned voices. The library owns buffer bounds and truncation.
use super::*;
use crest_performance_core::Command;

impl Runtime {
    pub(in crate::synth) fn require_performance_change(&self) -> Result<(), String> {
        if self.pending_open.is_some()
            || self.loading.is_some()
            || self.capture_baseline.is_some()
            || self.performance_preparing.is_some()
        {
            return Err("Finish the current document or take operation first".into());
        }
        if self
            .performance_live
            .as_ref()
            .is_some_and(|live| live.fault.is_some())
        {
            return Err("Finish or discard the interrupted input first".into());
        }
        Ok(())
    }

    pub(in crate::synth) fn capture_recent_performance(
        &mut self,
        sound: &Sound<'_>,
        clock: HostClock,
        slot: usize,
    ) -> Result<(), String> {
        self.require_performance_change()?;
        self.performance_live
            .as_mut()
            .ok_or("Enable LinnStrument MPE input before playing the phrase to capture")?
            .capture_recent(slot, sound, clock, self.app.borrow().editor.song().tempo)?;
        self.app.borrow_mut().performance_editor = None;
        self.performance_edit_committed(
            "CAPTURED · Last 4 beats · Live input continues · Undo restores previous clip",
        )
    }

    pub(in crate::synth) fn undo_performance_clip_change(&mut self) -> Result<(), String> {
        self.require_performance_change()?;
        let engine = self
            .performance_live
            .as_mut()
            .map(|live| &mut live.engine)
            .or(self.performance.as_mut())
            .ok_or("Performance session unavailable")?;
        engine.command(Command::Undo).map_err(|e| e.to_string())?;
        self.app.borrow_mut().performance_editor = None;
        self.performance_edit_committed(
            "CLIP CHANGE UNDONE · Previous clip restored · Live input continues",
        )
    }
}
