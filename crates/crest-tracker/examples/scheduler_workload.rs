//! Pure scheduler workload. No native audio, devices, UI, or latency claims.
use crest_tracker::composition::*;
use crest_tracker::playback::{Message, Owner, Playback, PlaybackBudget, Snapshot, TimedEvent};
use crest_tracker::time::{Beat, Division};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::error::Error;
use std::io::{self, Write};
use std::sync::Arc;
use std::time::Instant;

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const TRACKS: usize = 16;
const ROWS: [u32; 4] = [4, 6, 8, 3];
const CHORD: [i8; 4] = [0, 4, 7, 12];
const COMPILE_SAMPLES: usize = 32;
const BUDGET: PlaybackBudget = PlaybackBudget {
    steps_per_window: 128,
    events_per_window: 2048,
};

fn fixture(cycles: u32) -> Result<Song> {
    let divisions = [
        Division::SIXTEENTH,
        Division::new(16, 3, 2)?,
        Division::new(32, 1, 1)?,
        Division::new(8, 3, 2)?,
    ];
    let mut song = Song::default();
    for index in 0..TRACKS {
        let identity = (index as u64 + 1) * 100;
        let phrase_id = PhraseId::new(identity + 1)?;
        let chain_id = ChainId::new(identity + 2)?;
        let division = divisions[index % 4];
        let step = division.beat_length();
        let mut events = Vec::new();
        for row in 0..ROWS[index % 4] {
            let mut event = Event::note(
                EventId::new(u64::from(row) + 1)?,
                step.checked_scale(u64::from(row), 1)?,
                Target::Channel {
                    channel: Channel::new(index as u8 + 1)?,
                    endpoint: None,
                },
                Note::new(48 + (index % 4) as u8 * 2, 100, step.checked_scale(1, 2)?)?,
            );
            event.controls = vec![
                Control::Bend {
                    value: 8192 + (row as u16 * 13 + index as u16 * 17) % 4096,
                },
                Control::Pressure {
                    value: 32 + row as u8,
                },
                Control::Cc {
                    number: 74,
                    value: 16 + index as u8,
                },
                Control::Cc {
                    number: 1,
                    value: 64 + row as u8,
                },
            ];
            event.fx = [
                Some(Fx::Chord {
                    intervals: CHORD.to_vec(),
                }),
                Some(Fx::Retrigger {
                    count: 2,
                    spacing: step.checked_scale(1, 2)?,
                }),
                None,
            ];
            events.push(event);
        }
        let phrase = Phrase {
            steps: ROWS[index % 4],
            division,
            events,
            ..Phrase::default()
        };
        if phrase.length()? != Beat::ONE {
            return Err("fixture cycle must be exactly one beat".into());
        }
        song.phrases.insert(phrase_id, phrase);
        song.chains.insert(
            chain_id,
            Chain {
                name: format!("Workload {}", index + 1),
                phrases: vec![PhraseUse {
                    id: UseId::new(identity + 3)?,
                    phrase: phrase_id,
                    repeats: 1,
                }],
            },
        );
        song.tracks[index].slots = vec![Some(ChainUse {
            id: UseId::new(identity + 4)?,
            chain: chain_id,
            repeats: cycles,
        })];
        song.tracks[index].looped = false;
    }
    Ok(song)
}

/// Canonical serde JSON for each complete TimedEvent plus newline, streamed
/// through FNV-1a-64. Direct event equality below is the proof; this digest is
/// a compact reproducibility identifier, not a cryptographic certificate.
struct TraceDigest(u64);
impl Default for TraceDigest {
    fn default() -> Self {
        Self(0xcbf29ce484222325)
    }
}
impl Write for TraceDigest {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        for byte in bytes {
            self.0 = (self.0 ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct Run {
    name: &'static str,
    playback: Playback,
    parts: Vec<Beat>,
    next_part: usize,
    end: Beat,
    flushed: bool,
    buffered: std::vec::IntoIter<TimedEvent>,
    window_ns: Vec<u64>,
    window_events: Vec<usize>,
    total_events: u64,
    digest: TraceDigest,
}
impl Run {
    fn new(name: &'static str, snapshot: Arc<Snapshot>, parts: Vec<Beat>, end: Beat) -> Self {
        Self {
            name,
            playback: Playback::new(snapshot),
            parts,
            next_part: 0,
            end,
            flushed: false,
            buffered: Vec::new().into_iter(),
            window_ns: Vec::new(),
            window_events: Vec::new(),
            total_events: 0,
            digest: TraceDigest::default(),
        }
    }
    fn next(&mut self) -> Result<Option<TimedEvent>> {
        loop {
            if let Some(event) = self.buffered.next() {
                serde_json::to_writer(&mut self.digest, &event)?;
                self.digest.write_all(b"\n")?;
                self.total_events += 1;
                return Ok(Some(event));
            }
            if self.flushed {
                return Ok(None);
            }
            let end = if self.playback.horizon() == self.end {
                self.flushed = true;
                self.end.checked_add(Beat::new(1, 48)?)?
            } else {
                let part = self.parts[self.next_part % self.parts.len()];
                self.next_part += 1;
                self.playback.horizon().checked_add(part)?.min(self.end)
            };
            let started = Instant::now();
            let events = self.playback.window(end)?;
            self.window_ns
                .push(u64::try_from(started.elapsed().as_nanos())?);
            self.window_events.push(events.len());
            self.buffered = events.into_iter();
        }
    }
    fn report(&self) -> Value {
        json!({
            "name": self.name, "window_parts_beats": self.parts,
            "window_count": self.window_ns.len(), "window_timing_ns": percentiles(&self.window_ns),
            "raw_window_ns": self.window_ns, "raw_window_event_counts": self.window_events,
            "delivered_window_events_high_water": self.window_events.iter().max(),
            "total_events": self.total_events, "trace_digest_fnv1a64": format!("{:016x}", self.digest.0),
            "final_horizon_beats": self.playback.horizon(), "finished": self.playback.finished(),
        })
    }
}

struct Audit {
    active: BTreeMap<(Owner, u8), Beat>,
    high_water: usize,
    attacks: [u64; TRACKS],
    releases: u64,
    controls: u64,
    last_attack: [Beat; TRACKS],
    last_event: Beat,
}
impl Audit {
    fn new() -> Self {
        Self {
            active: BTreeMap::new(),
            high_water: 0,
            attacks: [0; TRACKS],
            releases: 0,
            controls: 0,
            last_attack: [Beat::ZERO; TRACKS],
            last_event: Beat::ZERO,
        }
    }
    fn event(&mut self, event: &TimedEvent, cycles: u32) -> Result<()> {
        let track = usize::try_from(event.owner.track.get() - 1)?;
        if track >= TRACKS || event.at < self.last_event || event.owner.cycle >= u64::from(cycles) {
            return Err(format!("invalid event identity/order: {event:?}").into());
        }
        let step = Beat::new(1, u64::from(ROWS[track % 4]))?;
        let half = step.checked_scale(1, 2)?;
        let row = event.owner.event.get() - 1;
        if row >= u64::from(ROWS[track % 4]) {
            return Err("unexpected row identity".into());
        }
        let row_start =
            Beat::integer(event.owner.cycle).checked_add(step.checked_scale(row, 1)?)?;
        match event.message {
            Message::NoteOn { pitch, .. } => {
                if event.at != row_start && event.at != row_start.checked_add(half)? {
                    return Err(format!("attack drift at {event:?}").into());
                }
                if self
                    .active
                    .insert((event.owner.clone(), pitch), event.at)
                    .is_some()
                {
                    return Err("overlapping retrigger reused an active owner".into());
                }
                self.attacks[track] += 1;
                self.last_attack[track] = event.at;
                self.high_water = self.high_water.max(self.active.len());
            }
            Message::NoteOff { pitch, .. } => {
                let attack = self
                    .active
                    .remove(&(event.owner.clone(), pitch))
                    .ok_or("release has no active owner")?;
                if event.at != attack.checked_add(half)? {
                    return Err(format!("release drift at {event:?}").into());
                }
                self.releases += 1;
            }
            Message::Control { .. } => {
                if event.at != row_start {
                    return Err("control timing drift".into());
                }
                self.controls += 1;
            }
            _ => return Err("unexpected workload message".into()),
        }
        self.last_event = event.at;
        Ok(())
    }
    fn finish(&self, cycles: u32) -> Result<Value> {
        let end = Beat::integer(u64::from(cycles));
        for track in 0..TRACKS {
            let expected_count = u64::from(cycles) * u64::from(ROWS[track % 4]) * 8;
            let expected_last = end.checked_sub(Beat::new(1, u64::from(ROWS[track % 4]) * 2)?)?;
            if self.attacks[track] != expected_count || self.last_attack[track] != expected_last {
                return Err(format!("track {} lost cycles or accumulated drift", track + 1).into());
            }
        }
        let attacks: u64 = self.attacks.iter().sum();
        if !self.active.is_empty()
            || self.high_water != 64
            || self.releases != attacks
            || self.controls != u64::from(cycles) * 336
            || self.last_event != end
        {
            return Err(
                "workload counts, final releases, or concurrency differ from declared fixture"
                    .into(),
            );
        }
        Ok(
            json!({ "note_ons": attacks, "note_offs": self.releases, "raw_controls": self.controls,
            "attacks_per_track": self.attacks, "simultaneous_voices_high_water": self.high_water,
            "remaining_active_voices": self.active.len(), "last_event_beat": self.last_event,
            "last_attack_per_track": self.last_attack, "exact_time_drift": "zero" }),
        )
    }
}

fn percentiles(samples: &[u64]) -> Value {
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let at = |percent: usize| sorted[(sorted.len() * percent).div_ceil(100).saturating_sub(1)];
    json!({ "p50": at(50), "p95": at(95), "p99": at(99), "max": sorted.last(), "method": "nearest_rank" })
}

fn cycles() -> Result<u32> {
    let mut arguments = std::env::args().skip(1);
    match arguments.next().as_deref() {
        None => Ok(10_000),
        Some("--cycles") => {
            let cycles: u32 = arguments
                .next()
                .ok_or("--cycles requires a positive integer")?
                .parse()?;
            if cycles == 0 || arguments.next().is_some() {
                return Err("usage: scheduler_workload [--cycles POSITIVE_INTEGER]".into());
            }
            Ok(cycles)
        }
        _ => Err("usage: scheduler_workload [--cycles POSITIVE_INTEGER]".into()),
    }
}

fn run() -> Result<()> {
    let cycles = cycles()?;
    let started = Instant::now();
    let song = fixture(cycles)?;
    let mut compile_ns = Vec::new();
    let mut snapshot = None;
    for _ in 0..COMPILE_SAMPLES {
        let started = Instant::now();
        let compiled = Snapshot::compile(&song, 1, BUDGET)?;
        compile_ns.push(u64::try_from(started.elapsed().as_nanos())?);
        snapshot = Some(Arc::new(compiled));
    }
    let snapshot = snapshot.ok_or("no compiled snapshot")?;
    let end = Beat::integer(u64::from(cycles));
    let mut whole = Run::new("one_beat", snapshot.clone(), vec![Beat::ONE], end);
    let mut split = Run::new(
        "sevenths",
        snapshot,
        [1, 2, 1, 3]
            .into_iter()
            .map(|n| Beat::new(n, 7))
            .collect::<std::result::Result<_, _>>()?,
        end,
    );
    let mut audit = Audit::new();
    let mut compared = 0_u64;
    loop {
        match (whole.next()?, split.next()?) {
            (None, None) => break,
            (Some(left), Some(right)) if left == right => {
                audit.event(&left, cycles)?;
                compared += 1;
            }
            (left, right) => {
                return Err(
                    format!("trace mismatch at event {compared}: {left:?} != {right:?}").into(),
                )
            }
        }
    }
    if whole.digest.0 != split.digest.0 || !whole.playback.finished() || !split.playback.finished()
    {
        return Err("trace digest or terminal playback state mismatch".into());
    }
    let audit = audit.finish(cycles)?;
    let report = json!({
        "schema_version": 1, "status": "passed", "scope": "pure_scheduler",
        "not_measured": ["native audio callbacks", "device queues", "physical latency", "monitoring", "live UI edits", "30-minute callback soak"],
        "workload": { "cycles": cycles, "cycle_beats": 1, "tracks": TRACKS, "voices_per_track": 4,
            "retrigger_count": 2, "raw_controls_per_row": 4, "rows_per_cycle_by_group": ROWS,
            "steps_per_window_budget": BUDGET.steps_per_window, "pending_events_budget": BUDGET.events_per_window,
            "terminal_flush_beats": Beat::new(1, 48)?, "seed": 1, "snapshot_revision": 1 },
        "comparison": { "method": "direct equality of every complete ordered TimedEvent", "events_compared": compared,
            "digest_encoding": "canonical serde JSON of each full TimedEvent followed by LF; FNV-1a-64", "full_trace_retained": false },
        "timing_scope": "Snapshot::compile and Playback::window only; excludes digest, equality, time audit and artifact serialization; partitions run interleaved",
        "compile_timing_ns": percentiles(&compile_ns), "raw_compile_ns": compile_ns,
        "runs": [whole.report(), split.report()], "audit": audit,
        "measurement_wall_ns": u64::try_from(started.elapsed().as_nanos())?,
    });
    serde_json::to_writer(io::stdout().lock(), &report)?;
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("scheduler workload failed: {error}");
        std::process::exit(1);
    }
}
