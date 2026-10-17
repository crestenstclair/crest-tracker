//! Independent clip owners share one chronologically merged native schedule.
//! All work here runs on the control thread; the audio callback sees only events.
use super::*;
use crest_performance_core::SlotState;
use crest_synth::real_time::VoiceGroupId;

struct Player {
    playback: Playback,
    group: VoiceGroupId,
}

#[derive(Default)]
pub(in crate::synth) struct Bank {
    players: BTreeMap<usize, Player>,
    // Preserve the publication fence after stopping the furthest-ahead player.
    horizon: u64,
}
impl Bank {
    pub fn contains(&self, slot: usize) -> bool {
        self.players.contains_key(&slot)
    }
    pub fn is_empty(&self) -> bool {
        self.players.is_empty()
    }
    pub fn horizon(&self) -> u64 {
        self.horizon
    }
    #[cfg(test)]
    pub fn get(&self, slot: usize) -> &Playback {
        &self.players.get(&slot).expect("playing slot").playback
    }
    pub fn states(&self) -> Vec<(usize, SlotState)> {
        self.players
            .values()
            .map(|p| p.playback.slot_state())
            .collect()
    }
    pub fn insert(&mut self, playback: Playback, sound: &mut Sound<'_>) -> Result<(), String> {
        if self.contains(playback.slot) {
            return Err("Clip is already playing".into());
        }
        let group = sound.create_voice_group().map_err(|e| e.to_string())?;
        self.players
            .insert(playback.slot, Player { playback, group });
        Ok(())
    }
    /// Cancellation invalidates published lookahead and held voices together.
    /// No Note Off reconstruction or restart of unrelated players is necessary.
    pub fn stop_slot(&mut self, slot: usize, sound: &mut Sound<'_>) -> Result<(), String> {
        if let Some(player) = self.players.get(&slot) {
            sound
                .cancel_voice_group(player.group)
                .map_err(|e| e.to_string())?;
            self.players.remove(&slot);
        }
        Ok(())
    }
    pub fn advance(
        &mut self,
        sound: &mut Sound<'_>,
        clock: HostClock,
        ids: &mut ScheduledVoices,
        tempo: crest_tracker::time::Tempo,
    ) -> Result<(), String> {
        let mut events = Vec::new();
        for (&slot, player) in &mut self.players {
            if player.playback.tempo_at_start != tempo {
                return Err("Tempo changed; restart performance audition at the new tempo".into());
            }
            events.extend(
                player
                    .playback
                    .prepare_window(sound, clock)?
                    .into_iter()
                    .map(|(frame, event)| (frame, slot, event)),
            );
            self.horizon = self.horizon.max(player.playback.horizon());
        }
        // Stable ties retain each library stream's ordering, then slot order.
        events.sort_by_key(|(frame, _, _)| *frame);
        for (frame, slot, event) in events {
            let player = self
                .players
                .get_mut(&slot)
                .ok_or("Playback owner disappeared")?;
            player
                .playback
                .destinations
                .schedule(sound, ids, frame, event, Some(player.group))?;
        }
        Ok(())
    }
}
