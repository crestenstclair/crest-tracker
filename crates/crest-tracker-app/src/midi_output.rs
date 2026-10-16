//! Worker-owned external MIDI delivery. An endpoint/channel used here belongs
//! to this host for its playback lifetime: MIDI 1.0 cannot isolate other clients
//! sharing that channel. Cancellation cleans only channels this host touched.
//! Deadlines use monotonic host time; timing estimates belong to the host clock
//! model. This worker is not a hard-RT service.
//! Ordinary tagged voices and declared MPE output use separate ownership paths.
//! MPE batches reserve zones and validate all retained master states before wire.

mod mpe;
pub use mpe::{MpeOutputAction, MpeOutputProfile, MpeOwner, MpeSource};

use midir::{MidiOutput, MidiOutputConnection};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU64;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_millis(2);

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MidiOutputEndpoint {
    /// Exact opaque midir port identity. Never a display name or list index.
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MidiOutputFailure {
    pub operation: String,
    pub endpoint_id: Option<String>,
    pub message: String,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MidiOutputSnapshot {
    pub endpoints: Vec<MidiOutputEndpoint>,
    pub connected: Vec<String>,
    pub last_error: Option<MidiOutputFailure>,
    pub error_count: u64,
    pub pending: usize,
    pub requested_epoch: u64,
    /// Cancellation cleanup has completed through this epoch, possibly with a
    /// visible backend failure. Check error_count as well as this acknowledgement.
    pub settled_epoch: u64,
    pub requested_monitor_generation: u64,
    pub settled_monitor_generation: u64,
    pub running: bool,
    #[serde(skip)]
    connection_versions: BTreeMap<String, u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum MidiOutputError {
    #[error("MIDI output queue capacity must be positive")]
    InvalidCapacity,
    #[error("MIDI output queue is full")]
    QueueFull,
    #[error("MIDI connection state is being updated; retry on the next control tick")]
    ConnectionBusy,
    #[error("exact MIDI endpoint is not connected")]
    NotConnected,
    #[error("MIDI output worker is closed")]
    Closed,
    #[error("MIDI output epoch is stale")]
    StaleEpoch,
    #[error("MIDI output epoch exhausted")]
    EpochExhausted,
    #[error("expected one complete MIDI 1.0 channel message")]
    InvalidMessage,
    #[error(
        "expected Start, Continue, Stop, Timing Clock or a complete 14-bit Song Position Pointer"
    )]
    InvalidTransportMessage,
    #[error("MIDI voice identity must be nonzero")]
    InvalidVoice,
    #[error("invalid MPE output batch: {0}")]
    InvalidMpe(String),
    #[error("cannot start MIDI output worker: {0}")]
    ThreadStart(String),
    #[cfg(test)]
    #[error("MIDI output worker panicked")]
    WorkerPanicked,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Packet {
    bytes: [u8; 3],
    len: usize,
}

impl Packet {
    fn parse(bytes: &[u8]) -> Result<Self, MidiOutputError> {
        let status = *bytes.first().ok_or(MidiOutputError::InvalidMessage)?;
        let len = match status >> 4 {
            0x8..=0xb | 0xe => 3,
            0xc | 0xd => 2,
            _ => return Err(MidiOutputError::InvalidMessage),
        };
        if bytes.len() != len || bytes[1..].iter().any(|byte| *byte > 127) {
            return Err(MidiOutputError::InvalidMessage);
        }
        let mut packet = Self { bytes: [0; 3], len };
        packet.bytes[..len].copy_from_slice(bytes);
        Ok(packet)
    }

    fn parse_transport(bytes: &[u8]) -> Result<Self, MidiOutputError> {
        let len = match bytes {
            [0xf8 | 0xfa | 0xfb | 0xfc] => 1,
            [0xf2, lsb, msb] if *lsb < 128 && *msb < 128 => 3,
            _ => return Err(MidiOutputError::InvalidTransportMessage),
        };
        let mut packet = Self { bytes: [0; 3], len };
        packet.bytes[..len].copy_from_slice(bytes);
        Ok(packet)
    }

    fn is_transport(self) -> bool {
        self.bytes[0] >= 0xf0
    }

    fn bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

struct Scheduled {
    endpoint: String,
    packet: Packet,
    deadline: Instant,
    epoch: u64,
    connection_version: u64,
    voice: Option<NonZeroU64>,
    mpe: Option<ScheduledMpe>,
}

struct ScheduledMpe {
    profile: MpeOutputProfile,
    actions: Vec<MpeOutputAction>,
    monitor_generation: u64,
    cost: usize,
}

impl Scheduled {
    fn cost(&self) -> usize {
        self.mpe.as_ref().map_or(1, |batch| batch.cost)
    }

    fn discard_monitor(&mut self, generation: u64) -> usize {
        let Some(batch) = &mut self.mpe else {
            return 0;
        };
        if batch.monitor_generation == generation {
            return 0;
        }
        let previous = batch.cost;
        batch
            .actions
            .retain(|action| action.owner().source != MpeSource::Monitor);
        batch.cost = mpe::batch_cost(&batch.actions).expect("subset of a bounded batch");
        batch.monitor_generation = generation;
        previous - batch.cost
    }
}

enum Command {
    Discover,
    Connect(String),
    Disconnect(String),
    Enqueue(Scheduled),
}

struct Shared {
    epoch: AtomicU64,
    monitor_generation: AtomicU64,
    pending: AtomicUsize,
    shutdown: AtomicBool,
    snapshot: Mutex<MidiOutputSnapshot>,
}

/// Control-side API. All normal methods are nonblocking; snapshot uses try_lock
/// and returns its last complete copy if the worker is publishing. Production
/// close waits for cancellation acknowledgement before dropping the service;
/// Drop itself requests asynchronous worker shutdown without blocking callbacks.
pub struct MidiOutputService {
    sender: SyncSender<Command>,
    shared: Arc<Shared>,
    snapshot: MidiOutputSnapshot,
    capacity: usize,
    worker: Option<JoinHandle<()>>,
}

impl MidiOutputService {
    pub fn new(capacity: usize) -> Result<Self, MidiOutputError> {
        if capacity == 0 {
            return Err(MidiOutputError::InvalidCapacity);
        }
        let (sender, receiver) = mpsc::sync_channel(capacity);
        let snapshot = MidiOutputSnapshot {
            requested_epoch: 1,
            settled_epoch: 1,
            requested_monitor_generation: 1,
            settled_monitor_generation: 1,
            running: true,
            ..Default::default()
        };
        let shared = Arc::new(Shared {
            epoch: AtomicU64::new(1),
            monitor_generation: AtomicU64::new(1),
            pending: AtomicUsize::new(0),
            shutdown: AtomicBool::new(false),
            snapshot: Mutex::new(snapshot.clone()),
        });
        let worker_shared = Arc::clone(&shared);
        let worker = thread::Builder::new()
            .name("crest-tracker-midi-output".into())
            .spawn(move || worker_main(receiver, worker_shared, capacity))
            .map_err(|error| MidiOutputError::ThreadStart(error.to_string()))?;
        Ok(Self {
            sender,
            shared,
            snapshot,
            capacity,
            worker: Some(worker),
        })
    }

    pub fn discover(&self) -> Result<(), MidiOutputError> {
        self.command(Command::Discover)
    }

    pub fn connect(&self, endpoint_id: &str) -> Result<(), MidiOutputError> {
        self.command(Command::Connect(endpoint_id.to_owned()))
    }

    pub fn disconnect(&self, endpoint_id: &str) -> Result<(), MidiOutputError> {
        self.command(Command::Disconnect(endpoint_id.to_owned()))
    }

    pub fn epoch(&self) -> u64 {
        self.shared.epoch.load(Ordering::Acquire)
    }

    pub fn enqueue(
        &self,
        endpoint_id: &str,
        bytes: &[u8],
        deadline: Instant,
        epoch: u64,
    ) -> Result<(), MidiOutputError> {
        self.enqueue_packet(endpoint_id, Packet::parse(bytes)?, deadline, epoch, None)
    }

    /// Explicit clock/transport output; ordinary enqueue remains channel-only.
    /// Uses the same deadline, connection revision, capacity and epoch rules.
    /// Cancellation sends Stop only to destinations that received clock/start.
    pub fn send_transport(
        &self,
        endpoint_id: &str,
        bytes: &[u8],
        deadline: Instant,
        epoch: u64,
    ) -> Result<(), MidiOutputError> {
        self.enqueue_packet(
            endpoint_id,
            Packet::parse_transport(bytes)?,
            deadline,
            epoch,
            None,
        )
    }

    /// Schedules one owned note or expression event. A same-pitch attack releases
    /// the previous tagged owner before retriggering; its later release is ignored.
    /// Identity must remain stable through the note lifetime. Delivery rejects
    /// expression that MIDI 1.0 cannot isolate, reported through snapshot errors.
    pub fn enqueue_voice(
        &self,
        endpoint_id: &str,
        bytes: &[u8],
        deadline: Instant,
        epoch: u64,
        voice: u64,
    ) -> Result<(), MidiOutputError> {
        let voice = NonZeroU64::new(voice).ok_or(MidiOutputError::InvalidVoice)?;
        self.enqueue_packet(
            endpoint_id,
            Packet::parse(bytes)?,
            deadline,
            epoch,
            Some(voice),
        )
    }

    fn enqueue_packet(
        &self,
        endpoint_id: &str,
        packet: Packet,
        deadline: Instant,
        epoch: u64,
        voice: Option<NonZeroU64>,
    ) -> Result<(), MidiOutputError> {
        self.enqueue_scheduled(endpoint_id, packet, deadline, epoch, voice, None)
    }

    /// The caller groups every action at an exact musical timestamp and endpoint
    /// into one batch. Independent batches cannot form a master-state transaction.
    pub fn enqueue_mpe_batch(
        &self,
        profile: &MpeOutputProfile,
        actions: Vec<MpeOutputAction>,
        deadline: Instant,
        epoch: u64,
    ) -> Result<(), MidiOutputError> {
        profile.validate().map_err(MidiOutputError::InvalidMpe)?;
        let cost = mpe::batch_cost(&actions).ok_or(MidiOutputError::QueueFull)?;
        if cost > self.capacity {
            return Err(MidiOutputError::QueueFull);
        }
        if actions.is_empty() {
            return Err(MidiOutputError::InvalidMpe("empty batch".into()));
        }
        for action in &actions {
            action.validate().map_err(MidiOutputError::InvalidMpe)?;
        }
        let batch = ScheduledMpe {
            profile: profile.clone(),
            actions,
            cost,
            monitor_generation: self.shared.monitor_generation.load(Ordering::Acquire),
        };
        self.enqueue_scheduled(
            &profile.endpoint,
            Packet {
                bytes: [0; 3],
                len: 0,
            },
            deadline,
            epoch,
            None,
            Some(batch),
        )
    }

    fn enqueue_scheduled(
        &self,
        endpoint_id: &str,
        packet: Packet,
        deadline: Instant,
        epoch: u64,
        voice: Option<NonZeroU64>,
        mpe: Option<ScheduledMpe>,
    ) -> Result<(), MidiOutputError> {
        if epoch != self.epoch() {
            return Err(MidiOutputError::StaleEpoch);
        }
        let connection_version = *self
            .shared
            .snapshot
            .try_lock()
            .map_err(|_| MidiOutputError::ConnectionBusy)?
            .connection_versions
            .get(endpoint_id)
            .ok_or(MidiOutputError::NotConnected)?;
        let cost = mpe.as_ref().map_or(1, |batch| batch.cost);
        self.shared
            .pending
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |pending| {
                pending
                    .checked_add(cost)
                    .filter(|sum| *sum <= self.capacity)
            })
            .map_err(|_| MidiOutputError::QueueFull)?;
        let result = self.command(Command::Enqueue(Scheduled {
            endpoint: endpoint_id.to_owned(),
            packet,
            deadline,
            epoch,
            connection_version,
            voice,
            mpe,
        }));
        if result.is_err() {
            self.shared.pending.fetch_sub(cost, Ordering::AcqRel);
        }
        result
    }

    /// Invalidates pending output even if the command queue is full. Cleanup is
    /// asynchronous and acknowledged in snapshot.settled_epoch.
    pub fn cancel(&self) -> Result<u64, MidiOutputError> {
        self.shared
            .epoch
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |epoch| {
                epoch.checked_add(1)
            })
            .map(|old| old + 1)
            .map_err(|_| MidiOutputError::EpochExhausted)
    }

    /// Invalidates queued monitor MPE and releases only monitor-owned zones.
    /// Generation changes bypass a full command queue, like global cancellation.
    pub fn cancel_monitor_mpe(&self) -> Result<(), MidiOutputError> {
        self.shared
            .monitor_generation
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |generation| {
                generation.checked_add(1)
            })
            .map(|_| ())
            .map_err(|_| MidiOutputError::EpochExhausted)
    }

    pub fn snapshot(&mut self) -> MidiOutputSnapshot {
        if let Ok(mut snapshot) = self.shared.snapshot.try_lock() {
            if snapshot.running && self.worker.as_ref().is_some_and(JoinHandle::is_finished) {
                snapshot.running = false;
                snapshot.error_count = snapshot.error_count.saturating_add(1);
                snapshot.last_error = Some(MidiOutputFailure {
                    operation: "worker".into(),
                    endpoint_id: None,
                    message: "MIDI output worker stopped unexpectedly".into(),
                });
            }
            self.snapshot = snapshot.clone();
        }
        self.snapshot.pending = self.shared.pending.load(Ordering::Acquire);
        self.snapshot.requested_epoch = self.epoch();
        self.snapshot.requested_monitor_generation =
            self.shared.monitor_generation.load(Ordering::Acquire);
        self.snapshot.clone()
    }

    /// Deterministic worker join for backend integration tests. Production close
    /// uses the cancellation acknowledgement and nonblocking Drop path.
    #[cfg(test)]
    pub fn shutdown(mut self) -> Result<(), MidiOutputError> {
        self.shared.shutdown.store(true, Ordering::Release);
        self.worker
            .take()
            .unwrap()
            .join()
            .map_err(|_| MidiOutputError::WorkerPanicked)
    }

    fn command(&self, command: Command) -> Result<(), MidiOutputError> {
        if self.shared.shutdown.load(Ordering::Acquire) {
            return Err(MidiOutputError::Closed);
        }
        self.sender.try_send(command).map_err(|error| match error {
            TrySendError::Full(_) => MidiOutputError::QueueFull,
            TrySendError::Disconnected(_) => MidiOutputError::Closed,
        })
    }
}

impl Drop for MidiOutputService {
    fn drop(&mut self) {
        // Never block a window/control callback on a native backend. Production
        // close has already waited for its cancellation acknowledgement.
        self.shared.shutdown.store(true, Ordering::Release);
    }
}

trait Sink {
    fn send(&mut self, bytes: &[u8]) -> Result<(), String>;
}

impl Sink for MidiOutputConnection {
    fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
        MidiOutputConnection::send(self, bytes).map_err(|error| error.to_string())
    }
}

#[derive(Clone, Copy)]
struct ChannelOwnership {
    notes: [u16; 128],
    voices: [Option<NonZeroU64>; 128],
    sustain: bool,
    touched: bool,
}

impl ChannelOwnership {
    const EMPTY: Self = Self {
        notes: [0; 128],
        voices: [None; 128],
        sustain: false,
        touched: false,
    };

    fn require_voice_expression(&self, packet: Packet, voice: NonZeroU64) -> Result<(), String> {
        let [status, key, _] = packet.bytes;
        if self.sustain {
            return Err("MIDI 1.0 cannot isolate tagged expression while sustain is active".into());
        }
        if status >> 4 == 0xa {
            return if self.voices[usize::from(key)] == Some(voice)
                && self.notes[usize::from(key)] == 1
            {
                Ok(())
            } else {
                Err("tagged poly pressure has no isolated current note owner".into())
            };
        }
        if !(matches!(status >> 4, 0xd | 0xe) || status >> 4 == 0xb && matches!(key, 7 | 11 | 74)) {
            return Err("tagged MIDI expression is unsupported; no channel-wide fallback".into());
        }
        if !self.voices.contains(&Some(voice)) {
            return Err("tagged expression has no current note owner".into());
        }
        if self
            .notes
            .iter()
            .map(|count| usize::from(*count))
            .sum::<usize>()
            != 1
        {
            return Err(
                "MIDI 1.0 cannot isolate tagged expression on an overlapping channel".into(),
            );
        }
        Ok(())
    }
}

struct Destination<S: Sink> {
    sink: S,
    channels: [ChannelOwnership; 16],
    failed: bool,
    version: u64,
    transport_active: bool,
    mpe: mpe::Allocator,
    mpe_recovery_notes: BTreeSet<(u8, u8)>,
    mpe_recovery_channels: BTreeSet<u8>,
}

impl<S: Sink> Destination<S> {
    fn new(sink: S, version: u64) -> Self {
        Self {
            sink,
            channels: [ChannelOwnership::EMPTY; 16],
            failed: false,
            version,
            transport_active: false,
            mpe: mpe::Allocator::default(),
            mpe_recovery_notes: BTreeSet::new(),
            mpe_recovery_channels: BTreeSet::new(),
        }
    }

    fn send_voice(&mut self, packet: Packet, voice: Option<NonZeroU64>) -> Result<(), String> {
        self.require_unreserved(packet)?;
        let Some(voice) = voice else {
            return self.send(packet);
        };
        if packet.is_transport() {
            return Err("transport packets cannot carry a note voice identity".into());
        }
        if self.failed {
            return Err("connection failed; explicit reconnect required".into());
        }
        let [status, key, value] = packet.bytes;
        let channel = usize::from(status & 15);
        let note = usize::from(key);
        match status >> 4 {
            9 if value > 0 => self.attack_voice(packet, voice),
            8 | 9 => {
                if self.channels[channel].voices[note] != Some(voice) {
                    return Ok(());
                }
                if self.channels[channel].notes[note] != 1 {
                    return Err("MIDI 1.0 cannot isolate a tagged release from untagged same-pitch ownership".into());
                }
                self.send(packet)
            }
            _ => {
                self.channels[channel].require_voice_expression(packet, voice)?;
                self.send(packet)
            }
        }
    }

    fn attack_voice(&mut self, packet: Packet, voice: NonZeroU64) -> Result<(), String> {
        let [status, key, _] = packet.bytes;
        let channel = usize::from(status & 15);
        let note = usize::from(key);
        let ownership = &self.channels[channel];
        if ownership
            .voices
            .iter()
            .enumerate()
            .any(|(pitch, owner)| pitch != note && *owner == Some(voice))
        {
            return Err("MIDI voice identity already owns a different note on this channel".into());
        }
        if ownership.notes[note] > u16::from(ownership.voices[note].is_some()) {
            return Err(
                "MIDI 1.0 cannot isolate a tagged attack from untagged same-pitch ownership".into(),
            );
        }
        if ownership.voices[note].is_some() {
            self.send(Packet {
                bytes: [0x80 | (status & 15), key, 0],
                len: 3,
            })?;
        }
        // Retain the new identity even if the backend partially sends its attack.
        self.channels[channel].voices[note] = Some(voice);
        self.send(packet)
    }

    fn send(&mut self, packet: Packet) -> Result<(), String> {
        if packet.is_transport() {
            return self.send_transport(packet);
        }
        if self.failed {
            return Err("connection failed; explicit reconnect required".into());
        }
        self.require_unreserved(packet)?;
        let [status, key, value] = packet.bytes;
        let channel = &mut self.channels[usize::from(status & 15)];
        if status >> 4 == 9 && value > 0 && channel.notes[usize::from(key)] == u16::MAX {
            return Err("owned overlapping note count exhausted".into());
        }
        // Retain ownership on a failed attack so later cleanup can still attempt
        // a release when a backend reported failure after partial delivery.
        channel.touched = true;
        if status >> 4 == 9 && value > 0 {
            channel.notes[usize::from(key)] += 1;
        }
        if let Err(error) = self.sink.send(packet.bytes()) {
            self.failed = true;
            return Err(error);
        }
        match status >> 4 {
            8 | 9 if status >> 4 == 8 || value == 0 => {
                let note = usize::from(key);
                channel.notes[note] = channel.notes[note].saturating_sub(1);
                if channel.notes[note] == 0 {
                    channel.voices[note] = None;
                }
            }
            11 if matches!(key, 120 | 123) => {
                channel.notes.fill(0);
                channel.voices.fill(None);
            }
            11 if key == 64 => channel.sustain = value >= 64,
            11 if key == 121 => channel.sustain = false,
            _ => {}
        }
        Ok(())
    }

    fn send_transport(&mut self, packet: Packet) -> Result<(), String> {
        if self.failed {
            return Err("connection failed; explicit reconnect required".into());
        }
        if matches!(packet.bytes[0], 0xf8 | 0xfa | 0xfb) {
            // A failed native call may have partially delivered the packet.
            self.transport_active = true;
        }
        if let Err(error) = self.sink.send(packet.bytes()) {
            self.failed = true;
            return Err(error);
        }
        if packet.bytes[0] == 0xfc {
            self.transport_active = false;
        }
        Ok(())
    }

    fn cleanup(&mut self) -> Result<(), String> {
        let mut first_error = None;
        if let Err(error) = self.cleanup_mpe() {
            first_error = Some(error);
        }
        if self.transport_active {
            match self.sink.send(&[0xfc]) {
                Ok(()) => self.transport_active = false,
                Err(error) => first_error = Some(error),
            }
        }
        for (channel, ownership) in self.channels.iter_mut().enumerate() {
            if !ownership.touched {
                continue;
            }
            let mut send = |packet: &[u8]| {
                if let Err(error) = self.sink.send(packet) {
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
            };
            for (note, count) in ownership.notes.iter().enumerate() {
                for _ in 0..*count {
                    send(&[0x80 | channel as u8, note as u8, 0]);
                }
            }
            send(&[0xb0 | channel as u8, 64, 0]);
            send(&[0xe0 | channel as u8, 0, 64]);
        }
        if let Some(error) = first_error {
            self.failed = true;
            Err(error)
        } else {
            self.channels.fill(ChannelOwnership::EMPTY);
            Ok(())
        }
    }

    fn require_unreserved(&self, packet: Packet) -> Result<(), String> {
        if !packet.is_transport() && self.mpe.reserves(packet.bytes[0] & 15) {
            return Err("ordinary MIDI collides with a reserved MPE zone".into());
        }
        Ok(())
    }

    fn send_mpe(
        &mut self,
        profile: &MpeOutputProfile,
        actions: &[MpeOutputAction],
    ) -> Result<(), String> {
        if self.failed {
            return Err("connection failed; explicit reconnect required".into());
        }
        let plan = self.mpe.plan(profile, actions, &self.channels)?;
        self.send_mpe_plan(plan)
    }

    fn send_mpe_plan(&mut self, plan: mpe::Plan) -> Result<(), String> {
        self.mpe_recovery_notes.extend(plan.notes);
        self.mpe_recovery_channels.extend(plan.channels);
        for packet in plan.packets {
            if let Err(error) = self.sink.send(packet.bytes()) {
                self.failed = true;
                return Err(error);
            }
        }
        self.mpe = plan.next;
        self.mpe_recovery_notes.clear();
        self.mpe_recovery_channels.clear();
        Ok(())
    }

    fn cancel_monitor_mpe(&mut self) -> Result<(), String> {
        if self.failed {
            return Err("connection failed; monitor ownership retained for cleanup".into());
        }
        if let Some(plan) = self.mpe.cancel_plan(MpeSource::Monitor, &self.channels)? {
            self.send_mpe_plan(plan)?;
        }
        Ok(())
    }

    fn cleanup_mpe(&mut self) -> Result<(), String> {
        let (notes, channels) = self.mpe.cleanup_ownership();
        self.mpe_recovery_notes.extend(notes);
        self.mpe_recovery_channels.extend(channels);
        let mut first_error = None;
        let mut send = |bytes: &[u8]| {
            if let Err(error) = self.sink.send(bytes) {
                first_error.get_or_insert(error);
            }
        };
        for (channel, note) in &self.mpe_recovery_notes {
            send(&[0x80 | channel, *note, 0]);
        }
        for channel in &self.mpe_recovery_channels {
            send(&[0xb0 | channel, 64, 0]);
            send(&[0xb0 | channel, 120, 0]);
            send(&[0xe0 | channel, 0, 64]);
            send(&[0xd0 | channel, 0]);
        }
        if let Some(error) = first_error {
            self.failed = true;
            return Err(error);
        }
        self.mpe = mpe::Allocator::default();
        self.mpe_recovery_notes.clear();
        self.mpe_recovery_channels.clear();
        Ok(())
    }
}

fn insert_ordered(queue: &mut Vec<Scheduled>, event: Scheduled) {
    let position = queue.partition_point(|existing| existing.deadline <= event.deadline);
    queue.insert(position, event);
}

fn deliver<S: Sink>(
    destinations: &mut BTreeMap<String, Destination<S>>,
    event: &Scheduled,
) -> Result<(), String> {
    let destination = destinations
        .get_mut(&event.endpoint)
        .ok_or_else(|| "exact MIDI endpoint is not connected".to_owned())?;
    if destination.version != event.connection_version {
        return Err("MIDI connection changed; stale output was not rerouted".into());
    }
    if let Some(batch) = &event.mpe {
        if batch.profile.endpoint != event.endpoint {
            return Err("MPE profile endpoint does not match the scheduled connection".into());
        }
        destination.send_mpe(&batch.profile, &batch.actions)
    } else {
        destination.send_voice(event.packet, event.voice)
    }
}

fn discard_endpoint(queue: &mut Vec<Scheduled>, endpoint: &str, shared: &Shared) {
    discard_where(queue, shared, |event| event.endpoint == endpoint);
}

fn discard_stale_epochs(queue: &mut Vec<Scheduled>, epoch: u64, shared: &Shared) {
    // New-epoch commands can arrive between cancellation and worker cleanup.
    // Preserve those commands while discarding the canceled epoch.
    discard_where(queue, shared, |event| event.epoch != epoch);
}

fn discard_where(
    queue: &mut Vec<Scheduled>,
    shared: &Shared,
    discard: impl Fn(&Scheduled) -> bool,
) {
    let mut cost = 0;
    queue.retain(|event| {
        if discard(event) {
            cost += event.cost();
            false
        } else {
            true
        }
    });
    shared.pending.fetch_sub(cost, Ordering::AcqRel);
}

fn discard_stale_monitor(queue: &mut Vec<Scheduled>, generation: u64, shared: &Shared) {
    let cost: usize = queue
        .iter_mut()
        .map(|event| event.discard_monitor(generation))
        .sum();
    queue.retain(|event| event.cost() > 0);
    shared.pending.fetch_sub(cost, Ordering::AcqRel);
}

fn report(shared: &Shared, operation: &str, endpoint: Option<&str>, message: String) {
    if let Ok(mut snapshot) = shared.snapshot.lock() {
        snapshot.error_count = snapshot.error_count.saturating_add(1);
        snapshot.last_error = Some(MidiOutputFailure {
            operation: operation.to_owned(),
            endpoint_id: endpoint.map(str::to_owned),
            message,
        });
    }
}

fn publish_connections<S: Sink>(shared: &Shared, destinations: &BTreeMap<String, Destination<S>>) {
    if let Ok(mut snapshot) = shared.snapshot.lock() {
        snapshot.connected = destinations
            .iter()
            .filter(|(_, destination)| !destination.failed)
            .map(|(id, _)| id.clone())
            .collect();
        snapshot.connection_versions = destinations
            .iter()
            .filter(|(_, destination)| !destination.failed)
            .map(|(id, destination)| (id.clone(), destination.version))
            .collect();
    }
}

fn cleanup_all<S: Sink>(shared: &Shared, destinations: &mut BTreeMap<String, Destination<S>>) {
    for (id, destination) in destinations.iter_mut() {
        if let Err(error) = destination.cleanup() {
            report(shared, "cleanup", Some(id), error);
        }
    }
    publish_connections(shared, destinations);
}

fn discover() -> Result<Vec<MidiOutputEndpoint>, String> {
    let output =
        MidiOutput::new("Crest Tracker Output Discovery").map_err(|error| error.to_string())?;
    let mut endpoints = Vec::new();
    for port in output.ports() {
        let id = port.id();
        if id.is_empty()
            || endpoints
                .iter()
                .any(|endpoint: &MidiOutputEndpoint| endpoint.id == id)
        {
            return Err("MIDI backend returned missing or duplicate endpoint identity".into());
        }
        endpoints.push(MidiOutputEndpoint {
            id,
            name: output.port_name(&port).map_err(|error| error.to_string())?,
        });
    }
    endpoints.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
    Ok(endpoints)
}

fn connect_exact(id: &str) -> Result<MidiOutputConnection, String> {
    let output = MidiOutput::new("Crest Tracker MIDI Output").map_err(|error| error.to_string())?;
    let mut matches = output.ports().into_iter().filter(|port| port.id() == id);
    let port = matches
        .next()
        .ok_or_else(|| "exact MIDI endpoint is unavailable".to_owned())?;
    if matches.next().is_some() {
        return Err("MIDI endpoint identity is ambiguous".into());
    }
    output
        .connect(&port, "crest-tracker-output")
        .map_err(|error| error.to_string())
}

fn worker_main(receiver: Receiver<Command>, shared: Arc<Shared>, capacity: usize) {
    let mut destinations: BTreeMap<String, Destination<MidiOutputConnection>> = BTreeMap::new();
    let mut queue: Vec<Scheduled> = Vec::with_capacity(capacity);
    let mut epoch = 1;
    let mut monitor_generation = 1;
    let mut connection_version = 0_u64;
    loop {
        let requested = shared.epoch.load(Ordering::Acquire);
        if requested != epoch {
            discard_stale_epochs(&mut queue, requested, &shared);
            cleanup_all(&shared, &mut destinations);
            epoch = requested;
            if let Ok(mut snapshot) = shared.snapshot.lock() {
                snapshot.settled_epoch = epoch;
            }
        }
        let requested_monitor = shared.monitor_generation.load(Ordering::Acquire);
        if requested_monitor != monitor_generation {
            discard_stale_monitor(&mut queue, requested_monitor, &shared);
            for (id, destination) in &mut destinations {
                if let Err(error) = destination.cancel_monitor_mpe() {
                    report(&shared, "cancel_monitor_mpe", Some(id), error);
                }
            }
            publish_connections(&shared, &destinations);
            monitor_generation = requested_monitor;
            if let Ok(mut snapshot) = shared.snapshot.lock() {
                snapshot.settled_monitor_generation = monitor_generation;
            }
        }
        if shared.shutdown.load(Ordering::Acquire) {
            break;
        }
        while queue
            .first()
            .is_some_and(|event| event.deadline <= Instant::now())
        {
            if shared.epoch.load(Ordering::Acquire) != epoch
                || shared.monitor_generation.load(Ordering::Acquire) != monitor_generation
            {
                break;
            }
            let event = queue.remove(0);
            shared.pending.fetch_sub(event.cost(), Ordering::AcqRel);
            let result = deliver(&mut destinations, &event);
            if let Err(error) = result {
                report(&shared, "send", Some(&event.endpoint), error);
                publish_connections(&shared, &destinations);
            }
        }
        let wait = queue.first().map_or(POLL_INTERVAL, |event| {
            event
                .deadline
                .saturating_duration_since(Instant::now())
                .min(POLL_INTERVAL)
        });
        let command = match receiver.recv_timeout(wait) {
            Ok(command) => command,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        match command {
            Command::Discover => match discover() {
                Ok(endpoints) => {
                    if let Ok(mut snapshot) = shared.snapshot.lock() {
                        snapshot.endpoints = endpoints;
                    }
                }
                Err(error) => report(&shared, "discover", None, error),
            },
            Command::Connect(id) => {
                if destinations
                    .get(&id)
                    .is_some_and(|destination| !destination.failed)
                {
                    continue;
                }
                discard_endpoint(&mut queue, &id, &shared);
                if let Some(mut old) = destinations.remove(&id) {
                    if let Err(error) = old.cleanup() {
                        report(&shared, "cleanup", Some(&id), error);
                        destinations.insert(id.clone(), old);
                        publish_connections(&shared, &destinations);
                        continue;
                    }
                }
                let Some(next_version) = connection_version.checked_add(1) else {
                    report(
                        &shared,
                        "connect",
                        Some(&id),
                        "MIDI connection revision exhausted".into(),
                    );
                    publish_connections(&shared, &destinations);
                    continue;
                };
                connection_version = next_version;
                match connect_exact(&id) {
                    Ok(connection) => {
                        destinations
                            .insert(id.clone(), Destination::new(connection, connection_version));
                    }
                    Err(error) => report(&shared, "connect", Some(&id), error),
                }
                publish_connections(&shared, &destinations);
            }
            Command::Disconnect(id) => {
                discard_endpoint(&mut queue, &id, &shared);
                if let Some(mut destination) = destinations.remove(&id) {
                    if let Err(error) = destination.cleanup() {
                        report(&shared, "cleanup", Some(&id), error);
                        destinations.insert(id.clone(), destination);
                    }
                }
                publish_connections(&shared, &destinations);
            }
            Command::Enqueue(mut event) => {
                let discarded =
                    event.discard_monitor(shared.monitor_generation.load(Ordering::Acquire));
                shared.pending.fetch_sub(discarded, Ordering::AcqRel);
                if event.cost() == 0 {
                    continue;
                }
                if event.epoch != shared.epoch.load(Ordering::Acquire) {
                    shared.pending.fetch_sub(event.cost(), Ordering::AcqRel);
                } else if destinations
                    .get(&event.endpoint)
                    .is_none_or(|destination| destination.failed)
                {
                    shared.pending.fetch_sub(event.cost(), Ordering::AcqRel);
                    report(
                        &shared,
                        "enqueue",
                        Some(&event.endpoint),
                        "exact MIDI endpoint is not connected".into(),
                    );
                } else {
                    insert_ordered(&mut queue, event);
                }
            }
        }
    }
    cleanup_all(&shared, &mut destinations);
    shared
        .pending
        .fetch_sub(queue.iter().map(Scheduled::cost).sum(), Ordering::AcqRel);
    while let Ok(command) = receiver.try_recv() {
        if let Command::Enqueue(event) = command {
            shared.pending.fetch_sub(event.cost(), Ordering::AcqRel);
        }
    }
    if let Ok(mut snapshot) = shared.snapshot.lock() {
        snapshot.running = false;
    }
}

#[cfg(test)]
pub(crate) use test_support::MidiOutputTestDriver;

#[cfg(test)]
impl MidiOutputService {
    /// A manually clocked worker fixture using the production queue, allocator,
    /// delivery and cancellation paths, with no native MIDI port dependency.
    pub(crate) fn recording_for_test(
        endpoint: &str,
        capacity: usize,
    ) -> (Self, MidiOutputTestDriver) {
        test_support::recording(endpoint, capacity)
    }
}

#[cfg(test)]
mod test_support {
    use super::*;

    pub(crate) struct MidiOutputTestBatch {
        pub profile: MpeOutputProfile,
        pub actions: Vec<MpeOutputAction>,
        pub deadline: Instant,
    }

    #[derive(Default)]
    struct RecordingSink(Arc<Mutex<Vec<Vec<u8>>>>);
    impl Sink for RecordingSink {
        fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
            self.0.lock().unwrap().push(bytes.to_vec());
            Ok(())
        }
    }

    pub(crate) struct MidiOutputTestDriver {
        receiver: Receiver<Command>,
        shared: Arc<Shared>,
        destinations: BTreeMap<String, Destination<RecordingSink>>,
        queue: Vec<Scheduled>,
        packets: Arc<Mutex<Vec<Vec<u8>>>>,
        batches: Vec<MidiOutputTestBatch>,
        epoch: u64,
        monitor_generation: u64,
    }

    pub(super) fn recording(
        endpoint: &str,
        capacity: usize,
    ) -> (MidiOutputService, MidiOutputTestDriver) {
        assert!(capacity > 0 && !endpoint.is_empty());
        let (sender, receiver) = mpsc::sync_channel(capacity);
        let snapshot = MidiOutputSnapshot {
            running: true,
            requested_epoch: 1,
            settled_epoch: 1,
            requested_monitor_generation: 1,
            settled_monitor_generation: 1,
            endpoints: vec![MidiOutputEndpoint {
                id: endpoint.into(),
                name: endpoint.into(),
            }],
            connected: vec![endpoint.into()],
            connection_versions: BTreeMap::from([(endpoint.into(), 1)]),
            ..Default::default()
        };
        let shared = Arc::new(Shared {
            epoch: AtomicU64::new(1),
            monitor_generation: AtomicU64::new(1),
            pending: AtomicUsize::new(0),
            shutdown: AtomicBool::new(false),
            snapshot: Mutex::new(snapshot.clone()),
        });
        let packets = Arc::new(Mutex::new(Vec::new()));
        let driver = MidiOutputTestDriver {
            receiver,
            shared: Arc::clone(&shared),
            destinations: BTreeMap::from([(
                endpoint.into(),
                Destination::new(RecordingSink(Arc::clone(&packets)), 1),
            )]),
            queue: Vec::new(),
            packets,
            batches: Vec::new(),
            epoch: 1,
            monitor_generation: 1,
        };
        (
            MidiOutputService {
                sender,
                shared,
                snapshot,
                capacity,
                worker: None,
            },
            driver,
        )
    }

    impl MidiOutputTestDriver {
        pub(crate) fn packets(&self) -> Vec<Vec<u8>> {
            self.packets.lock().unwrap().clone()
        }

        pub(crate) fn batches(&self) -> &[MidiOutputTestBatch] {
            &self.batches
        }

        pub(crate) fn flush_due(&mut self, now: Instant) -> Result<(), String> {
            let epoch = self.shared.epoch.load(Ordering::Acquire);
            if epoch != self.epoch {
                discard_stale_epochs(&mut self.queue, epoch, &self.shared);
                cleanup_all(&self.shared, &mut self.destinations);
                self.epoch = epoch;
                self.shared.snapshot.lock().unwrap().settled_epoch = epoch;
            }
            let generation = self.shared.monitor_generation.load(Ordering::Acquire);
            if generation != self.monitor_generation {
                discard_stale_monitor(&mut self.queue, generation, &self.shared);
                for (endpoint, destination) in &mut self.destinations {
                    if let Err(error) = destination.cancel_monitor_mpe() {
                        report(
                            &self.shared,
                            "cancel_monitor_mpe",
                            Some(endpoint),
                            error.clone(),
                        );
                        return Err(error);
                    }
                }
                self.monitor_generation = generation;
                self.shared
                    .snapshot
                    .lock()
                    .unwrap()
                    .settled_monitor_generation = generation;
            }
            while let Ok(command) = self.receiver.try_recv() {
                match command {
                    Command::Enqueue(mut event) => {
                        let discarded = event.discard_monitor(generation);
                        self.shared.pending.fetch_sub(discarded, Ordering::AcqRel);
                        if event.epoch != epoch {
                            self.shared
                                .pending
                                .fetch_sub(event.cost(), Ordering::AcqRel);
                        } else if event.cost() > 0 {
                            insert_ordered(&mut self.queue, event);
                        }
                    }
                    Command::Discover => {}
                    Command::Connect(endpoint) => {
                        if !self.destinations.contains_key(&endpoint) {
                            return Err(
                                "recording fixture cannot connect an undeclared endpoint".into()
                            );
                        }
                    }
                    Command::Disconnect(endpoint) => {
                        discard_endpoint(&mut self.queue, &endpoint, &self.shared);
                        if let Some(destination) = self.destinations.get_mut(&endpoint) {
                            destination.cleanup()?;
                        }
                        self.destinations.remove(&endpoint);
                        publish_connections(&self.shared, &self.destinations);
                    }
                }
            }
            while self
                .queue
                .first()
                .is_some_and(|event| event.deadline <= now)
            {
                let event = self.queue.remove(0);
                self.shared
                    .pending
                    .fetch_sub(event.cost(), Ordering::AcqRel);
                if let Err(error) = deliver(&mut self.destinations, &event) {
                    report(&self.shared, "send", Some(&event.endpoint), error.clone());
                    publish_connections(&self.shared, &self.destinations);
                    return Err(error);
                }
                if let Some(batch) = event.mpe {
                    self.batches.push(MidiOutputTestBatch {
                        profile: batch.profile,
                        actions: batch.actions,
                        deadline: event.deadline,
                    });
                }
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct RecordingSink(Vec<Vec<u8>>);
    impl Sink for RecordingSink {
        fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
            self.0.push(bytes.to_vec());
            Ok(())
        }
    }

    fn tagged<S: Sink>(
        destination: &mut Destination<S>,
        bytes: &[u8],
        voice: u64,
    ) -> Result<(), String> {
        destination.send_voice(Packet::parse(bytes).unwrap(), NonZeroU64::new(voice))
    }

    #[test]
    fn transport_packet_parser_is_explicit_and_channel_parser_stays_restricted() {
        for bytes in [
            vec![0xfa],
            vec![0xfb],
            vec![0xfc],
            vec![0xf8],
            vec![0xf2, 127, 127],
        ] {
            assert_eq!(Packet::parse_transport(&bytes).unwrap().bytes(), bytes);
            assert!(Packet::parse(&bytes).is_err());
        }
        for bytes in [
            vec![],
            vec![0xf2, 0],
            vec![0xf2, 0, 128],
            vec![0xf8, 0],
            vec![0xfe],
            vec![0xf9],
            vec![0x90, 60, 100],
        ] {
            assert!(Packet::parse_transport(&bytes).is_err());
        }
    }

    #[test]
    fn cancellation_stops_only_transport_destinations_that_may_have_started() {
        for status in [0xfa, 0xfb, 0xf8] {
            let mut destination = Destination::new(RecordingSink::default(), 1);
            destination
                .send(Packet::parse_transport(&[status]).unwrap())
                .unwrap();
            assert!(destination.channels.iter().all(|channel| !channel.touched));
            destination.cleanup().unwrap();
            destination.cleanup().unwrap();
            assert_eq!(destination.sink.0, [vec![status], vec![0xfc]]);
        }
        let mut destination = Destination::new(RecordingSink::default(), 1);
        destination
            .send(Packet::parse_transport(&[0xf2, 4, 0]).unwrap())
            .unwrap();
        destination.cleanup().unwrap();
        assert_eq!(destination.sink.0, [vec![0xf2, 4, 0]]);
        for status in [0xfa, 0xf8, 0xfc] {
            destination
                .send(Packet::parse_transport(&[status]).unwrap())
                .unwrap();
        }
        let sent = destination.sink.0.len();
        destination.cleanup().unwrap();
        assert_eq!(
            destination.sink.0.len(),
            sent,
            "explicit Stop already settled transport ownership"
        );
    }

    #[test]
    fn failed_transport_delivery_keeps_stop_cleanup_and_surfaces_its_failure() {
        struct FailingSink {
            sent: Vec<Vec<u8>>,
            fail: bool,
        }
        impl Sink for FailingSink {
            fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
                self.sent.push(bytes.to_vec());
                if self.fail {
                    Err("backend failed".into())
                } else {
                    Ok(())
                }
            }
        }
        let mut destination = Destination::new(
            FailingSink {
                sent: Vec::new(),
                fail: true,
            },
            1,
        );
        assert_eq!(
            destination.send(Packet::parse_transport(&[0xfa]).unwrap()),
            Err("backend failed".into())
        );
        assert_eq!(destination.cleanup(), Err("backend failed".into()));
        assert!(destination.transport_active);
        destination.sink.fail = false;
        destination.cleanup().unwrap();
        assert!(!destination.transport_active);
        assert!(destination.failed, "only reconnect clears backend failure");
        assert_eq!(destination.sink.sent, [vec![0xfa], vec![0xfc], vec![0xfc]]);
    }

    #[test]
    fn clock_packets_keep_exact_connection_revision_and_cannot_carry_voice_ids() {
        let mut destinations =
            BTreeMap::from([("port".into(), Destination::new(RecordingSink::default(), 2))]);
        let mut event = Scheduled {
            endpoint: "port".into(),
            packet: Packet::parse_transport(&[0xf8]).unwrap(),
            deadline: Instant::now(),
            epoch: 1,
            connection_version: 1,
            voice: None,
            mpe: None,
        };
        assert!(deliver(&mut destinations, &event)
            .unwrap_err()
            .contains("stale"));
        assert!(destinations["port"].sink.0.is_empty());
        event.connection_version = 2;
        event.voice = NonZeroU64::new(1);
        assert!(deliver(&mut destinations, &event).is_err());
        event.voice = None;
        deliver(&mut destinations, &event).unwrap();
        destinations.get_mut("port").unwrap().cleanup().unwrap();
        assert_eq!(destinations["port"].sink.0, [vec![0xf8], vec![0xfc]]);
    }

    #[test]
    fn tagged_same_pitch_releases_prior_owner_and_ignores_its_late_release() {
        let mut destination = Destination::new(RecordingSink::default(), 1);
        tagged(&mut destination, &[0x92, 60, 100], 1).unwrap();
        tagged(&mut destination, &[0x92, 60, 110], 2).unwrap();
        tagged(&mut destination, &[0x82, 60, 90], 1).unwrap();
        tagged(&mut destination, &[0x92, 60, 0], 1).unwrap();
        tagged(&mut destination, &[0x82, 60, 80], 2).unwrap();
        assert_eq!(
            destination.sink.0,
            [
                vec![0x92, 60, 100],
                vec![0x82, 60, 0],
                vec![0x92, 60, 110],
                vec![0x82, 60, 80],
            ]
        );
        assert_eq!(destination.channels[2].notes[60], 0);
        assert_eq!(destination.channels[2].voices[60], None);
    }

    #[test]
    fn tagged_ownership_is_scoped_to_exact_endpoint_and_channel() {
        let mut destinations = BTreeMap::from([
            (
                "first".into(),
                Destination::new(RecordingSink::default(), 1),
            ),
            (
                "second".into(),
                Destination::new(RecordingSink::default(), 2),
            ),
        ]);
        let send =
            |destinations: &mut BTreeMap<_, _>, endpoint: &str, version, bytes: &[u8], voice| {
                deliver(
                    destinations,
                    &Scheduled {
                        endpoint: endpoint.into(),
                        packet: Packet::parse(bytes).unwrap(),
                        deadline: Instant::now(),
                        epoch: 1,
                        connection_version: version,
                        voice: NonZeroU64::new(voice),
                        mpe: None,
                    },
                )
                .unwrap();
            };
        send(&mut destinations, "first", 1, &[0x90, 60, 100], 1);
        send(&mut destinations, "first", 1, &[0x91, 60, 100], 2);
        send(&mut destinations, "second", 2, &[0x90, 60, 100], 3);
        send(&mut destinations, "first", 1, &[0x90, 60, 110], 4);
        send(&mut destinations, "first", 1, &[0x80, 60, 0], 1);
        assert_eq!(
            destinations["first"].channels[0].voices[60],
            NonZeroU64::new(4)
        );
        assert_eq!(
            destinations["first"].channels[1].voices[60],
            NonZeroU64::new(2)
        );
        assert_eq!(
            destinations["second"].channels[0].voices[60],
            NonZeroU64::new(3)
        );
        assert_eq!(destinations["second"].sink.0, [vec![0x90, 60, 100]]);
        assert_eq!(destinations["first"].sink.0.len(), 4);
    }

    #[test]
    fn tagged_expression_rejects_ambiguous_channel_but_keeps_poly_and_raw_controls() {
        let mut destination = Destination::new(RecordingSink::default(), 1);
        tagged(&mut destination, &[0x90, 60, 100], 1).unwrap();
        tagged(&mut destination, &[0xe0, 127, 127], 1).unwrap();
        tagged(&mut destination, &[0x90, 62, 100], 2).unwrap();
        for bytes in [&[0xe0, 1, 65][..], &[0xd0, 80][..], &[0xb0, 74, 90][..]] {
            assert!(tagged(&mut destination, bytes, 1)
                .unwrap_err()
                .contains("overlapping"));
        }
        tagged(&mut destination, &[0xa0, 60, 80], 1).unwrap();
        destination
            .send(Packet::parse(&[0xe0, 0, 64]).unwrap())
            .unwrap();
        assert_eq!(
            destination.sink.0,
            [
                vec![0x90, 60, 100],
                vec![0xe0, 127, 127],
                vec![0x90, 62, 100],
                vec![0xa0, 60, 80],
                vec![0xe0, 0, 64],
            ]
        );
        assert!(
            !destination.failed,
            "ownership rejection must not mark the backend failed"
        );
    }

    #[test]
    fn stale_or_unsupported_tagged_expression_has_no_channelwide_fallback() {
        let mut destination = Destination::new(RecordingSink::default(), 1);
        tagged(&mut destination, &[0x90, 60, 100], 1).unwrap();
        tagged(&mut destination, &[0x90, 60, 110], 2).unwrap();
        let sent = destination.sink.0.len();
        for bytes in [&[0xe0, 0, 64][..], &[0xa0, 60, 80][..]] {
            assert!(tagged(&mut destination, bytes, 1).is_err());
        }
        for bytes in [&[0xc0, 7][..], &[0xb0, 64, 127][..], &[0xb0, 123, 0][..]] {
            assert!(tagged(&mut destination, bytes, 2)
                .unwrap_err()
                .contains("unsupported"));
        }
        assert_eq!(destination.sink.0.len(), sent);
        destination
            .send(Packet::parse(&[0xb0, 64, 127]).unwrap())
            .unwrap();
        assert!(tagged(&mut destination, &[0xe0, 0, 64], 2)
            .unwrap_err()
            .contains("sustain"));
        destination
            .send(Packet::parse(&[0xb0, 64, 0]).unwrap())
            .unwrap();
        tagged(&mut destination, &[0xd0, 80], 2).unwrap();
    }

    #[test]
    fn tagged_and_untagged_same_pitch_conflicts_fail_without_sending() {
        let mut destination = Destination::new(RecordingSink::default(), 1);
        destination
            .send(Packet::parse(&[0x90, 60, 100]).unwrap())
            .unwrap();
        assert!(tagged(&mut destination, &[0x90, 60, 100], 1).is_err());
        tagged(&mut destination, &[0x90, 62, 100], 2).unwrap();
        destination
            .send(Packet::parse(&[0x90, 62, 100]).unwrap())
            .unwrap();
        let sent = destination.sink.0.len();
        assert!(tagged(&mut destination, &[0x80, 62, 0], 2).is_err());
        assert!(tagged(&mut destination, &[0xa0, 62, 100], 2).is_err());
        assert!(tagged(&mut destination, &[0x90, 63, 100], 2).is_err());
        assert_eq!(destination.sink.0.len(), sent);
    }

    #[test]
    fn tagged_cancellation_releases_only_current_owners_without_global_panic() {
        let mut destination = Destination::new(RecordingSink::default(), 1);
        tagged(&mut destination, &[0x92, 60, 100], 1).unwrap();
        tagged(&mut destination, &[0x92, 60, 110], 2).unwrap();
        tagged(&mut destination, &[0x9f, 72, 100], 3).unwrap();
        destination.sink.0.clear();
        destination.cleanup().unwrap();
        assert_eq!(
            destination.sink.0,
            [
                vec![0x82, 60, 0],
                vec![0xb2, 64, 0],
                vec![0xe2, 0, 64],
                vec![0x8f, 72, 0],
                vec![0xbf, 64, 0],
                vec![0xef, 0, 64],
            ]
        );
        destination.sink.0.clear();
        tagged(&mut destination, &[0x82, 60, 0], 2).unwrap();
        destination.cleanup().unwrap();
        assert!(destination.sink.0.is_empty());
        assert!(destination
            .channels
            .iter()
            .all(|channel| channel.voices.iter().all(Option::is_none)));
    }

    #[test]
    fn cancellation_releases_each_owned_attack_and_only_touched_channels() {
        let mut destination = Destination::new(RecordingSink::default(), 1);
        for bytes in [
            [0x92, 60, 100],
            [0x92, 60, 100],
            [0x9f, 72, 100],
            [0xbf, 64, 127],
            [0x8f, 72, 0],
        ] {
            destination.send(Packet::parse(&bytes).unwrap()).unwrap();
        }
        destination.sink.0.clear();
        destination.cleanup().unwrap();
        assert_eq!(
            destination.sink.0,
            [
                vec![0x82, 60, 0],
                vec![0x82, 60, 0],
                vec![0xb2, 64, 0],
                vec![0xe2, 0, 64],
                vec![0xbf, 64, 0],
                vec![0xef, 0, 64],
            ]
        );
        destination.sink.0.clear();
        destination.cleanup().unwrap();
        assert!(destination.sink.0.is_empty());
    }

    #[test]
    fn scheduling_orders_deadlines_and_keeps_equal_time_fifo() {
        let now = Instant::now();
        let event = |note, millis| Scheduled {
            endpoint: "opaque-port".into(),
            packet: Packet::parse(&[0x90, note, 100]).unwrap(),
            deadline: now + Duration::from_millis(millis),
            epoch: 1,
            connection_version: 1,
            voice: None,
            mpe: None,
        };
        let mut queue = Vec::new();
        for event in [event(60, 30), event(61, 10), event(62, 10)] {
            insert_ordered(&mut queue, event);
        }
        assert_eq!(
            queue
                .iter()
                .map(|event| event.packet.bytes[1])
                .collect::<Vec<_>>(),
            [61, 62, 60]
        );
        assert!(queue[0].deadline > now);
    }

    #[test]
    fn stale_connection_events_cannot_reach_reconnected_destination() {
        let event = Scheduled {
            endpoint: "opaque-port".into(),
            packet: Packet::parse(&[0x90, 60, 100]).unwrap(),
            deadline: Instant::now(),
            epoch: 1,
            connection_version: 1,
            voice: NonZeroU64::new(1),
            mpe: None,
        };
        let mut destinations = BTreeMap::from([
            (
                "opaque-port".to_owned(),
                Destination::new(RecordingSink::default(), 2),
            ),
            (
                "same-name-other-id".to_owned(),
                Destination::new(RecordingSink::default(), 1),
            ),
        ]);
        assert!(deliver(&mut destinations, &event)
            .unwrap_err()
            .contains("not rerouted"));
        assert!(destinations
            .values()
            .all(|destination| destination.sink.0.is_empty()));
    }

    #[test]
    fn enqueue_voice_validates_identity_and_captures_connection_and_epoch() {
        let shared = Arc::new(Shared {
            epoch: AtomicU64::new(3),
            monitor_generation: AtomicU64::new(1),
            pending: AtomicUsize::new(0),
            shutdown: AtomicBool::new(false),
            snapshot: Mutex::new(MidiOutputSnapshot {
                connection_versions: BTreeMap::from([("port".into(), 7)]),
                ..Default::default()
            }),
        });
        let (sender, receiver) = mpsc::sync_channel(2);
        let service = MidiOutputService {
            sender,
            shared: Arc::clone(&shared),
            snapshot: MidiOutputSnapshot::default(),
            capacity: 2,
            worker: None,
        };
        let deadline = Instant::now();
        assert!(matches!(
            service.enqueue_voice("port", &[0x90, 60, 100], deadline, 3, 0),
            Err(MidiOutputError::InvalidVoice)
        ));
        assert_eq!(shared.pending.load(Ordering::Acquire), 0);
        service
            .enqueue_voice("port", &[0x90, 60, 100], deadline, 3, 42)
            .unwrap();
        let Command::Enqueue(event) = receiver.try_recv().unwrap() else {
            panic!("expected owned MIDI event");
        };
        assert_eq!(event.voice, NonZeroU64::new(42));
        assert_eq!(event.endpoint, "port");
        assert_eq!(event.connection_version, 7);
        assert_eq!(event.epoch, 3);
        assert_eq!(event.deadline, deadline);
        let mut queue = vec![event];
        discard_stale_epochs(&mut queue, service.cancel().unwrap(), &shared);
        assert!(queue.is_empty());
        assert_eq!(shared.pending.load(Ordering::Acquire), 0);
    }

    #[test]
    fn failed_retrigger_release_preserves_old_owner_for_cleanup() {
        struct FailingReleaseSink {
            sent: Vec<Vec<u8>>,
            fail_release: bool,
        }
        impl Sink for FailingReleaseSink {
            fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
                if bytes[0] >> 4 == 8 && std::mem::take(&mut self.fail_release) {
                    return Err("release failed".into());
                }
                self.sent.push(bytes.to_vec());
                Ok(())
            }
        }
        let mut destination = Destination::new(
            FailingReleaseSink {
                sent: Vec::new(),
                fail_release: true,
            },
            1,
        );
        tagged(&mut destination, &[0x90, 60, 100], 1).unwrap();
        assert!(tagged(&mut destination, &[0x90, 60, 110], 2).is_err());
        assert_eq!(destination.channels[0].voices[60], NonZeroU64::new(1));
        assert_eq!(destination.channels[0].notes[60], 1);
        assert_eq!(destination.sink.sent, [vec![0x90, 60, 100]]);
        destination.cleanup().unwrap();
        assert_eq!(
            destination.sink.sent,
            [
                vec![0x90, 60, 100],
                vec![0x80, 60, 0],
                vec![0xb0, 64, 0],
                vec![0xe0, 0, 64]
            ]
        );
    }

    #[test]
    fn cancel_bypasses_full_queue_and_preserves_new_epoch_events() {
        let shared = Arc::new(Shared {
            epoch: AtomicU64::new(1),
            monitor_generation: AtomicU64::new(1),
            pending: AtomicUsize::new(0),
            shutdown: AtomicBool::new(false),
            snapshot: Mutex::new(MidiOutputSnapshot {
                connection_versions: BTreeMap::from([("port".into(), 1)]),
                ..Default::default()
            }),
        });
        let (sender, receiver) = mpsc::sync_channel(2);
        let service = MidiOutputService {
            sender,
            shared: Arc::clone(&shared),
            snapshot: MidiOutputSnapshot::default(),
            capacity: 2,
            worker: None,
        };
        for _ in 0..2 {
            service
                .enqueue("port", &[0x90, 60, 100], Instant::now(), 1)
                .unwrap();
        }
        assert!(matches!(
            service.enqueue("port", &[0x90, 60, 100], Instant::now(), 1),
            Err(MidiOutputError::QueueFull)
        ));
        let epoch = service.cancel().unwrap();
        assert_eq!(epoch, 2);
        assert!(matches!(
            service.enqueue("port", &[0x90, 60, 100], Instant::now(), 1),
            Err(MidiOutputError::StaleEpoch)
        ));
        let mut queue = Vec::new();
        for _ in 0..2 {
            let Command::Enqueue(event) = receiver.try_recv().unwrap() else {
                panic!("expected MIDI");
            };
            queue.push(event);
        }
        discard_stale_epochs(&mut queue, epoch, &shared);
        assert_eq!(shared.pending.load(Ordering::Acquire), 0);
        service
            .enqueue("port", &[0x90, 62, 100], Instant::now(), epoch)
            .unwrap();
        let Command::Enqueue(event) = receiver.try_recv().unwrap() else {
            panic!("expected MIDI");
        };
        queue.push(event);
        discard_stale_epochs(&mut queue, epoch, &shared);
        assert_eq!(queue.len(), 1);
        assert_eq!(shared.pending.load(Ordering::Acquire), 1);
    }

    #[test]
    fn failed_note_off_retains_ownership_for_cleanup_retry() {
        struct FallibleSink {
            sent: Vec<Vec<u8>>,
            fail_next: bool,
        }
        impl Sink for FallibleSink {
            fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
                if std::mem::take(&mut self.fail_next) {
                    return Err("disconnected".into());
                }
                self.sent.push(bytes.to_vec());
                Ok(())
            }
        }
        let mut destination = Destination::new(
            FallibleSink {
                sent: Vec::new(),
                fail_next: false,
            },
            1,
        );
        destination
            .send(Packet::parse(&[0x90, 60, 100]).unwrap())
            .unwrap();
        destination.sink.fail_next = true;
        assert!(destination
            .send(Packet::parse(&[0x80, 60, 0]).unwrap())
            .is_err());
        assert!(destination.failed);
        destination.sink.sent.clear();
        destination.cleanup().unwrap();
        assert_eq!(
            destination.sink.sent,
            [vec![0x80, 60, 0], vec![0xb0, 64, 0], vec![0xe0, 0, 64]]
        );
    }

    #[test]
    fn raw_packets_keep_precision_and_reject_malformed_messages() {
        for bytes in [&[0xe2, 127, 127][..], &[0xd2, 127][..], &[0xa2, 60, 63][..]] {
            assert_eq!(Packet::parse(bytes).unwrap().bytes(), bytes);
        }
        for bytes in [
            &[][..],
            &[0x90, 60][..],
            &[0x90, 128, 100][..],
            &[0xf0, 1, 0xf7][..],
        ] {
            assert!(matches!(
                Packet::parse(bytes),
                Err(MidiOutputError::InvalidMessage)
            ));
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "requires a running CoreMIDI service; creates an ephemeral virtual endpoint"]
    fn coremidi_virtual_endpoint_receives_deadline_and_owned_cleanup() {
        use midir::os::unix::VirtualInput;
        let name = format!("Crest Tracker Output Witness {}", std::process::id());
        let (sender, receiver) = mpsc::channel();
        let input = midir::MidiInput::new(&name).unwrap();
        let _virtual_input = input
            .create_virtual(
                &name,
                move |_, bytes, _| {
                    let _ = sender.send(bytes.to_vec());
                },
                (),
            )
            .unwrap();
        let mut service = MidiOutputService::new(32).unwrap();
        service.discover().unwrap();
        let limit = Instant::now() + Duration::from_secs(3);
        let id = loop {
            let snapshot = service.snapshot();
            if let Some(endpoint) = snapshot
                .endpoints
                .iter()
                .find(|endpoint| endpoint.name == name)
            {
                break endpoint.id.clone();
            }
            assert!(
                Instant::now() < limit,
                "discovery failed: {:?}",
                snapshot.last_error
            );
            thread::sleep(Duration::from_millis(2));
        };
        service.connect(&id).unwrap();
        while !service.snapshot().connected.contains(&id) {
            assert!(Instant::now() < limit);
            thread::sleep(Duration::from_millis(2));
        }
        let deadline = Instant::now() + Duration::from_millis(60);
        service
            .enqueue(&id, &[0x93, 60, 100], deadline, service.epoch())
            .unwrap();
        assert!(receiver.recv_timeout(Duration::from_millis(20)).is_err());
        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(1)).unwrap(),
            [0x93, 60, 100]
        );
        assert!(Instant::now() >= deadline);
        service.cancel().unwrap();
        for expected in [[0x83, 60, 0], [0xb3, 64, 0], [0xe3, 0, 64]] {
            assert_eq!(
                receiver.recv_timeout(Duration::from_secs(1)).unwrap(),
                expected
            );
        }
        witness_tagged_retrigger(&service, &id, &receiver);
        assert_eq!(service.snapshot().error_count, 0);
        service.shutdown().unwrap();
    }

    #[cfg(target_os = "macos")]
    fn witness_tagged_retrigger(
        service: &MidiOutputService,
        id: &str,
        receiver: &Receiver<Vec<u8>>,
    ) {
        let epoch = service.epoch();
        let deadline = Instant::now() + Duration::from_millis(20);
        for (bytes, millis, voice) in [
            ([0x93, 60, 90], 0, 1),
            ([0x93, 60, 110], 10, 2),
            ([0x83, 60, 0], 20, 1),
            ([0x83, 60, 80], 30, 2),
        ] {
            service
                .enqueue_voice(
                    id,
                    &bytes,
                    deadline + Duration::from_millis(millis),
                    epoch,
                    voice,
                )
                .unwrap();
        }
        for expected in [
            [0x93, 60, 90],
            [0x83, 60, 0],
            [0x93, 60, 110],
            [0x83, 60, 80],
        ] {
            assert_eq!(
                receiver.recv_timeout(Duration::from_secs(1)).unwrap(),
                expected
            );
        }
        assert!(receiver.recv_timeout(Duration::from_millis(30)).is_err());
    }
}
