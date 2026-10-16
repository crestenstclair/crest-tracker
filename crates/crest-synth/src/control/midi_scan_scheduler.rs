/// Portable physical MIDI discovery cadence.
pub const MIDI_SCAN_INTERVAL_MICROS: u64 = 1_000_000;

/// Control-thread scheduler for startup, Settings-entry, and steady scans.
///
/// The scheduler owns no clock and performs no work itself. Callers provide a
/// monotonic timestamp and the reducer's current in-flight fact. Due requests
/// are coalesced until the current scan completes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MidiScanScheduler {
    startup_requested: bool,
    settings_was_open: bool,
    request_pending: bool,
    last_request_micros: Option<u64>,
}

impl MidiScanScheduler {
    /// Returns true exactly when control should dispatch
    /// `AppEvent::MidiInputScanStarted` on this tick.
    pub fn tick(&mut self, now_micros: u64, settings_open: bool, scan_in_flight: bool) -> bool {
        let entered_settings = settings_open && !self.settings_was_open;
        self.settings_was_open = settings_open;
        let periodic_due = self
            .last_request_micros
            .is_some_and(|last| now_micros.saturating_sub(last) >= MIDI_SCAN_INTERVAL_MICROS);
        let due =
            !self.startup_requested || entered_settings || periodic_due || self.request_pending;

        if !due {
            return false;
        }
        if scan_in_flight {
            self.request_pending = true;
            return false;
        }

        self.startup_requested = true;
        self.request_pending = false;
        self.last_request_micros = Some(now_micros);
        true
    }

    pub const fn request_pending(self) -> bool {
        self.request_pending
    }

    pub const fn last_request_micros(self) -> Option<u64> {
        self.last_request_micros
    }
}

#[cfg(test)]
mod tests {
    use super::{MidiScanScheduler, MIDI_SCAN_INTERVAL_MICROS};

    #[test]
    fn deterministic_clock_coalesces_startup_settings_and_periodic_scans() {
        let mut scheduler = MidiScanScheduler::default();

        assert!(scheduler.tick(0, false, false), "startup scan");
        assert!(!scheduler.tick(1, false, true));
        assert!(
            !scheduler.tick(10, true, true),
            "entry coalesces while busy"
        );
        assert!(scheduler.request_pending());
        assert!(scheduler.tick(20, true, false), "coalesced entry scan");
        assert!(!scheduler.tick(21, true, false), "open is edge-triggered");
        assert!(!scheduler.tick(20 + MIDI_SCAN_INTERVAL_MICROS - 1, true, false));
        assert!(scheduler.tick(20 + MIDI_SCAN_INTERVAL_MICROS, true, false));
        assert!(!scheduler.tick(20 + 2 * MIDI_SCAN_INTERVAL_MICROS, false, true));
        assert!(scheduler.request_pending());
        assert!(scheduler.tick(20 + 2 * MIDI_SCAN_INTERVAL_MICROS + 1, false, false));
    }
}
