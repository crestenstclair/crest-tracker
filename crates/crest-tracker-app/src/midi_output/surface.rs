//! Latest controller display intent is independent from the musical queue.
//! The worker owns mode restoration, including partially failed configuration.
use super::*;
use crest_linnstrument::{Geometry, LedRenderer};

const REFRESH: Duration = Duration::from_millis(34);
const CELL_BUDGET: usize = 8;
const MODE_POLL: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SurfaceLease {
    endpoint: String,
    connection: u64,
    generation: u64,
}

struct Desired {
    lease: SurfaceLease,
    geometry: Geometry,
    frame: [[u8; 8]; 26],
}

pub(super) struct Mailbox {
    pub(super) generation: AtomicU64,
    desired: Mutex<Option<Desired>>,
}
impl Default for Mailbox {
    fn default() -> Self {
        Self {
            generation: AtomicU64::new(1),
            desired: Mutex::new(None),
        }
    }
}

impl MidiOutputService {
    /// Captures the exact connection. Reconnect requires explicit reacquisition.
    /// One controller can own this mailbox; a new lease restores the previous one.
    pub(crate) fn acquire_surface(&self, endpoint: &str) -> Result<SurfaceLease, MidiOutputError> {
        let connection = *self
            .shared
            .snapshot
            .try_lock()
            .map_err(|_| MidiOutputError::ConnectionBusy)?
            .connection_versions
            .get(endpoint)
            .ok_or(MidiOutputError::NotConnected)?;
        Ok(SurfaceLease {
            endpoint: endpoint.into(),
            connection,
            generation: self.shared.surface.generation.load(Ordering::Acquire),
        })
    }

    /// Coalesces complete LED intent. No cell/configuration batch can be partly
    /// admitted, and a full musical queue cannot block controller restoration.
    pub(crate) fn display_surface(
        &self,
        lease: &SurfaceLease,
        geometry: Geometry,
        frame: [[u8; 8]; 26],
    ) -> Result<(), MidiOutputError> {
        if self.shared.shutdown.load(Ordering::Acquire) {
            return Err(MidiOutputError::Closed);
        }
        if lease.generation != self.shared.surface.generation.load(Ordering::Acquire) {
            return Err(MidiOutputError::StaleEpoch);
        }
        if frame.iter().flatten().any(|color| *color > 11) {
            return Err(MidiOutputError::InvalidMessage);
        }
        let connection = self
            .shared
            .snapshot
            .try_lock()
            .map_err(|_| MidiOutputError::ConnectionBusy)?
            .connection_versions
            .get(&lease.endpoint)
            .copied();
        if connection != Some(lease.connection) {
            return Err(MidiOutputError::NotConnected);
        }
        *self
            .shared
            .surface
            .desired
            .try_lock()
            .map_err(|_| MidiOutputError::ConnectionBusy)? = Some(Desired {
            lease: lease.clone(),
            geometry,
            frame,
        });
        Ok(())
    }

    /// Out-of-band restoration; acknowledged even when the OS reports failure.
    /// Callers must check output errors as well as the settled generation.
    pub(crate) fn release_surface(&self) -> Result<u64, MidiOutputError> {
        self.shared
            .surface
            .generation
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |generation| {
                generation.checked_add(1)
            })
            .map(|old| old + 1)
            .map_err(|_| MidiOutputError::EpochExhausted)
    }
}

struct Active {
    desired: Desired,
    leds: LedRenderer,
    next_frame: Instant,
    next_query: Instant,
}

pub(super) struct Worker {
    generation: u64,
    active: Option<Active>,
    recovery: Option<SurfaceLease>,
}
impl Default for Worker {
    fn default() -> Self {
        Self {
            generation: 1,
            active: None,
            recovery: None,
        }
    }
}
impl Worker {
    pub(super) fn advance<S: Sink>(
        &mut self,
        shared: &Shared,
        destinations: &mut BTreeMap<String, Destination<S>>,
        now: Instant,
    ) {
        let generation = shared.surface.generation.load(Ordering::Acquire);
        if generation != self.generation {
            self.restore(shared, destinations);
            self.generation = generation;
            if let Ok(mut snapshot) = shared.snapshot.lock() {
                snapshot.settled_surface_generation = generation;
            }
        }
        let desired = shared
            .surface
            .desired
            .lock()
            .ok()
            .and_then(|mut slot| slot.take());
        if let Some(desired) = desired.filter(|d| d.lease.generation == generation) {
            self.adopt(shared, destinations, desired, now);
        }
        let Some(active) = &mut self.active else {
            return;
        };
        if now < active.next_frame {
            return;
        }
        active.next_frame = now + REFRESH;
        let lease = &active.desired.lease;
        let result = destinations
            .get_mut(&lease.endpoint)
            .filter(|d| d.version == lease.connection && !d.failed)
            .ok_or_else(|| "LinnStrument feedback connection changed".to_owned())
            .and_then(|destination| {
                let messages = active
                    .leds
                    .diff(&active.desired.frame, CELL_BUDGET)
                    .map_err(|e| e.to_string())?;
                send_messages(&mut destination.sink, &messages)?;
                if now >= active.next_query {
                    query_mode(&mut destination.sink)?;
                    active.next_query = now + MODE_POLL;
                }
                Ok(())
            });
        if let Err(error) = result {
            report(shared, "surface", Some(&lease.endpoint), error);
            if let Some(destination) = destinations.get_mut(&lease.endpoint) {
                destination.failed = true;
            }
            self.restore(shared, destinations);
            publish_connections(shared, destinations);
        }
    }

    fn adopt<S: Sink>(
        &mut self,
        shared: &Shared,
        destinations: &mut BTreeMap<String, Destination<S>>,
        desired: Desired,
        now: Instant,
    ) {
        if let Some(active) = &mut self.active {
            if active.desired.lease == desired.lease && active.desired.geometry == desired.geometry
            {
                active.desired.frame = desired.frame;
                return;
            }
        }
        if !self.restore(shared, destinations) {
            return;
        }
        let lease = &desired.lease;
        let result = destinations
            .get_mut(&lease.endpoint)
            .filter(|d| d.version == lease.connection && !d.failed)
            .ok_or_else(|| "LinnStrument feedback connection changed".to_owned())
            .and_then(Destination::enter_surface);
        if let Err(error) = result {
            report(shared, "surface", Some(&lease.endpoint), error);
            publish_connections(shared, destinations);
            return;
        }
        self.recovery = Some(desired.lease.clone());
        let leds = LedRenderer::new(desired.geometry);
        self.active = Some(Active {
            desired,
            leds,
            next_frame: now,
            next_query: now + MODE_POLL,
        });
    }

    pub(super) fn retire<S: Sink>(
        &mut self,
        shared: &Shared,
        destinations: &mut BTreeMap<String, Destination<S>>,
        endpoint: &str,
    ) {
        if self
            .recovery
            .as_ref()
            .is_some_and(|lease| lease.endpoint == endpoint)
        {
            self.restore(shared, destinations);
            self.recovery = None;
        }
    }

    pub(super) fn restore<S: Sink>(
        &mut self,
        shared: &Shared,
        destinations: &mut BTreeMap<String, Destination<S>>,
    ) -> bool {
        self.active = None;
        let Some(lease) = &self.recovery else {
            return true;
        };
        let mut restored = true;
        if let Some(destination) = destinations
            .get_mut(&lease.endpoint)
            .filter(|d| d.version == lease.connection)
        {
            destination.surface_owned = true;
            if let Err(error) = destination.restore_surface() {
                report(shared, "surface_restore", Some(&lease.endpoint), error);
                restored = false;
            }
        }
        publish_connections(shared, destinations);
        restored
    }
}

impl<S: Sink> Destination<S> {
    fn enter_surface(&mut self) -> Result<(), String> {
        if self.transport_active
            || self.channels.iter().any(|c| c.touched)
            || (0..16).any(|channel| self.mpe.reserves(channel))
        {
            return Err(
                "Stop musical output to this endpoint before claiming LinnStrument feedback".into(),
            );
        }
        // Record recovery ownership before the first byte reaches the port.
        self.surface_owned = true;
        let result = send_messages(&mut self.sink, &crest_linnstrument::user_mode(true))
            .and_then(|()| query_mode(&mut self.sink));
        if result.is_err() {
            let _ = self.restore_surface();
            self.failed = true;
        }
        result
    }

    pub(super) fn restore_surface(&mut self) -> Result<(), String> {
        if !self.surface_owned {
            return Ok(());
        }
        // Complete the sequence even if an earlier send failed. A later retry
        // starts with a fresh NRPN selection rather than reusing partial state.
        let mut error = None;
        for message in crest_linnstrument::user_mode(false) {
            if let Err(e) = self.sink.send(message.as_bytes()) {
                error.get_or_insert(e);
            }
        }
        if let Err(e) = query_mode(&mut self.sink) {
            error.get_or_insert(e);
        }
        if let Some(error) = error {
            self.failed = true;
            Err(error)
        } else {
            self.surface_owned = false;
            Ok(())
        }
    }

    pub(super) fn retire(&mut self) -> Result<(), String> {
        let surface = self.restore_surface();
        let music = self.cleanup();
        surface.and(music)
    }
}

fn send_messages(
    sink: &mut impl Sink,
    messages: &[crest_linnstrument::MidiMessage],
) -> Result<(), String> {
    for message in messages {
        sink.send(message.as_bytes())?;
    }
    Ok(())
}

fn query_mode(sink: &mut impl Sink) -> Result<(), String> {
    // Stock firmware NRPN 299 queries a parameter; 245 is User Firmware Mode.
    // Query also produces a response when setting an already-active mode.
    for bytes in [[0xb0, 99, 2], [0xb0, 98, 43], [0xb0, 6, 1], [0xb0, 38, 117]] {
        sink.send(&bytes)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ON: [[u8; 3]; 4] = [[0xb0, 99, 1], [0xb0, 98, 117], [0xb0, 6, 0], [0xb0, 38, 1]];
    const OFF: [[u8; 3]; 4] = [[0xb0, 99, 1], [0xb0, 98, 117], [0xb0, 6, 0], [0xb0, 38, 0]];

    #[test]
    fn feedback_coalesces_complete_cells_and_stop_keeps_mode_but_release_bypasses_full_queue() {
        let (mut service, mut driver) = MidiOutputService::recording_for_test("feedback", 1);
        let lease = service.acquire_surface("feedback").unwrap();
        let mut frame = [[0; 8]; 26];
        frame[1][0] = 5;
        let now = Instant::now();
        service.discover().unwrap(); // Fill the same command queue used by notes.
        assert!(matches!(
            service.discover(),
            Err(MidiOutputError::QueueFull)
        ));
        service
            .display_surface(&lease, Geometry::Pads200, frame)
            .unwrap();
        driver.flush_due(now).unwrap();
        assert_eq!(&driver.packets()[..4], &ON);
        assert_eq!(driver.packets().len(), 8 + 8 * 3);
        frame[1][0] = 3;
        service
            .display_surface(&lease, Geometry::Pads200, frame)
            .unwrap();
        frame[1][0] = 5;
        service
            .display_surface(&lease, Geometry::Pads200, frame)
            .unwrap();
        driver.flush_due(now + Duration::from_millis(10)).unwrap();
        assert_eq!(driver.packets().len(), 32); // Render rate is independent of control ticks.
        service.cancel().unwrap();
        driver.flush_due(now + REFRESH).unwrap();
        assert_eq!(
            &driver.packets()[32..35],
            &[[0xb0, 20, 1], [0xb0, 21, 0], [0xb0, 22, 5]]
        );
        assert!(!driver.packets().windows(4).any(|bytes| bytes == OFF));
        service.discover().unwrap();
        let generation = service.release_surface().unwrap();
        driver.flush_due(now + REFRESH * 2).unwrap();
        let packets = driver.packets();
        assert_eq!(&packets[packets.len() - 8..packets.len() - 4], &OFF);
        assert_eq!(service.snapshot().settled_surface_generation, generation);
        assert!(matches!(
            service.display_surface(&lease, Geometry::Pads200, frame),
            Err(MidiOutputError::StaleEpoch)
        ));
        // Explicit recovery retries resend a fresh off/query transaction.
        service.release_surface().unwrap();
        driver.flush_due(now + REFRESH * 3).unwrap();
        assert_eq!(driver.packets().len(), packets.len() + 8);
    }

    #[test]
    fn release_discards_unpublished_mode_and_disconnect_never_reuses_a_stale_frame() {
        let (mut service, mut driver) = MidiOutputService::recording_for_test("feedback", 2);
        let lease = service.acquire_surface("feedback").unwrap();
        service
            .display_surface(&lease, Geometry::Pads128, [[0; 8]; 26])
            .unwrap();
        service.release_surface().unwrap();
        let now = Instant::now();
        driver.flush_due(now).unwrap();
        assert!(driver.packets().is_empty());
        let lease = service.acquire_surface("feedback").unwrap();
        service
            .display_surface(&lease, Geometry::Pads128, [[0; 8]; 26])
            .unwrap();
        driver.flush_due(now).unwrap();
        service
            .display_surface(&lease, Geometry::Pads128, [[1; 8]; 26])
            .unwrap();
        service.disconnect("feedback").unwrap();
        driver.flush_due(now + REFRESH).unwrap();
        assert_eq!(
            driver
                .packets()
                .windows(4)
                .filter(|bytes| *bytes == ON)
                .count(),
            1
        );
        assert!(driver.packets().windows(4).any(|bytes| bytes == OFF));
        assert!(service.snapshot().connected.is_empty());
        assert!(matches!(
            service.display_surface(&lease, Geometry::Pads128, [[0; 8]; 26]),
            Err(MidiOutputError::NotConnected)
        ));
    }

    #[derive(Default)]
    struct FailingSink {
        calls: usize,
        fail_at: Option<usize>,
        packets: Vec<Vec<u8>>,
    }
    impl Sink for FailingSink {
        fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
            self.calls += 1;
            if self.fail_at == Some(self.calls) {
                return Err("injected write failure".into());
            }
            self.packets.push(bytes.to_vec());
            Ok(())
        }
    }

    #[test]
    fn partial_mode_failure_restores_from_fresh_selection_and_reserves_no_music() {
        for fail_at in 1..=8 {
            let mut destination = Destination::new(
                FailingSink {
                    fail_at: Some(fail_at),
                    ..Default::default()
                },
                1,
            );
            assert!(destination.enter_surface().is_err());
            assert!(destination.failed);
            assert!(!destination.surface_owned);
            assert!(destination
                .sink
                .packets
                .windows(4)
                .any(|bytes| bytes == OFF));
        }
        let mut destination = Destination::new(FailingSink::default(), 1);
        destination.enter_surface().unwrap();
        assert!(destination
            .send_voice(Packet::parse(&[0x90, 60, 100]).unwrap(), None)
            .is_err());
        destination.cleanup().unwrap();
        assert!(destination.surface_owned); // Musical cancellation does not change device mode.
        destination.retire().unwrap();
        assert!(!destination.surface_owned);
    }

    #[test]
    fn obsolete_connection_cannot_receive_mode_or_led_commands() {
        let (mut service, _driver) = MidiOutputService::recording_for_test("feedback", 2);
        let lease = service.acquire_surface("feedback").unwrap();
        service
            .display_surface(&lease, Geometry::Pads200, [[0; 8]; 26])
            .unwrap();
        let mut destinations = BTreeMap::from([(
            "feedback".into(),
            Destination::new(FailingSink::default(), 2),
        )]);
        Worker::default().advance(&service.shared, &mut destinations, Instant::now());
        assert!(destinations["feedback"].sink.packets.is_empty());
        assert_eq!(service.snapshot().error_count, 1);
    }

    #[test]
    fn idle_feedback_queries_firmware_and_128_pad_repaint_stays_in_bounds() {
        let (service, mut driver) = MidiOutputService::recording_for_test("feedback", 2);
        let lease = service.acquire_surface("feedback").unwrap();
        service
            .display_surface(&lease, Geometry::Pads128, [[5; 8]; 26])
            .unwrap();
        let now = Instant::now();
        for frame in 0..17 {
            driver.flush_due(now + REFRESH * frame).unwrap();
        }
        let packets = driver.packets();
        assert_eq!(packets.len(), 8 + 17 * 8 * 3);
        assert!(packets.iter().filter(|p| p[1] == 20).all(|p| p[2] < 17));
        driver.flush_due(now + MODE_POLL).unwrap();
        let after = driver.packets();
        assert_eq!(
            &after[packets.len()..],
            &[[0xb0, 99, 2], [0xb0, 98, 43], [0xb0, 6, 1], [0xb0, 38, 117]]
        );
    }

    #[test]
    fn partial_led_failure_restores_mode_and_publishes_failed_connection() {
        let (mut service, _driver) = MidiOutputService::recording_for_test("feedback", 2);
        let lease = service.acquire_surface("feedback").unwrap();
        service
            .display_surface(&lease, Geometry::Pads200, [[0; 8]; 26])
            .unwrap();
        let sink = FailingSink {
            fail_at: Some(10),
            ..Default::default()
        };
        let mut destinations = BTreeMap::from([("feedback".into(), Destination::new(sink, 1))]);
        let mut worker = Worker::default();
        let now = Instant::now();
        worker.advance(&service.shared, &mut destinations, now);
        let destination = &destinations["feedback"];
        assert!(destination.failed);
        assert!(!destination.surface_owned);
        assert!(destination
            .sink
            .packets
            .windows(4)
            .any(|bytes| bytes == OFF));
        let count = destination.sink.packets.len();
        worker.advance(&service.shared, &mut destinations, now + REFRESH);
        assert_eq!(destinations["feedback"].sink.packets.len(), count);
        assert!(service.snapshot().connected.is_empty());
    }
}
