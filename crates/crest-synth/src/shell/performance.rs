//! Opt-in process timing. Producers only read clocks and update fixed atomics;
//! a separate thread formats and writes cumulative JSONL observations.
use crate::real_time::AudioObservationSnapshot;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{mpsc, OnceLock};
use std::time::{Duration, Instant};

const BIN_NS: u64 = 100_000;
const BINS: usize = 512;
const STAGES: usize = 10;

#[derive(Clone, Copy)]
#[repr(usize)]
pub enum Stage {
    AudioCallback,
    WindowFrame,
    TickGap,
    HostTick,
    Request,
    Scheduler,
    HostProjection,
    SynthProjection,
    SceneCheck,
    PageRender,
}
const NAMES: [&str; STAGES] = [
    "audio_callback",
    "window_frame",
    "tick_gap",
    "host_tick",
    "request",
    "scheduler",
    "host_projection",
    "synth_projection",
    "scene_check",
    "page_render",
];

struct Histogram {
    bins: [AtomicU64; BINS],
    total_ns: AtomicU64,
    max_ns: AtomicU64,
    over_budget: AtomicU64,
}
impl Default for Histogram {
    fn default() -> Self {
        Self {
            bins: std::array::from_fn(|_| AtomicU64::new(0)),
            total_ns: AtomicU64::new(0),
            max_ns: AtomicU64::new(0),
            over_budget: AtomicU64::new(0),
        }
    }
}
impl Histogram {
    fn record(&self, ns: u64, budget_ns: Option<u64>) {
        self.bins[(ns / BIN_NS).min((BINS - 1) as u64) as usize].fetch_add(1, Relaxed);
        self.total_ns.fetch_add(ns, Relaxed);
        self.max_ns.fetch_max(ns, Relaxed);
        if budget_ns.is_some_and(|budget| ns > budget) {
            self.over_budget.fetch_add(1, Relaxed);
        }
    }
    fn snapshot(&self) -> serde_json::Value {
        let bins = self.bins.each_ref().map(|bin| bin.load(Relaxed));
        let count: u64 = bins.iter().sum();
        let max = self.max_ns.load(Relaxed);
        let percentile = |p: u64| {
            let target = (count * p).div_ceil(100);
            let mut seen = 0;
            if count == 0 {
                return 0.0;
            }
            for (i, value) in bins.iter().enumerate() {
                seen += value;
                if seen >= target {
                    return if i == BINS - 1 {
                        max
                    } else {
                        ((i + 1) as u64 * BIN_NS).min(max)
                    } as f64
                        / 1e6;
                }
            }
            max as f64 / 1e6
        };
        serde_json::json!({
            "count": count, "mean_ms": self.total_ns.load(Relaxed) as f64 / count.max(1) as f64 / 1e6,
            "p50_ms": percentile(50), "p95_ms": percentile(95), "p99_ms": percentile(99),
            "max_ms": max as f64 / 1e6, "over_budget": self.over_budget.load(Relaxed),
        })
    }
}

struct Metrics {
    all: [Histogram; STAGES],
    playback: [Histogram; STAGES],
    playing: AtomicBool,
    callback_deadline_ns: AtomicU64,
    rendered_frames: AtomicU64,
    late_events: AtomicU64,
    routing_failures: AtomicU64,
    voice_refusals: AtomicU64,
    nonfinite_samples: AtomicU64,
    recoveries: AtomicU64,
    peak_queued_inputs: AtomicU64,
}
impl Default for Metrics {
    fn default() -> Self {
        Self {
            all: std::array::from_fn(|_| Histogram::default()),
            playback: std::array::from_fn(|_| Histogram::default()),
            playing: AtomicBool::new(false),
            callback_deadline_ns: AtomicU64::new(u64::MAX),
            rendered_frames: AtomicU64::new(0),
            late_events: AtomicU64::new(0),
            routing_failures: AtomicU64::new(0),
            voice_refusals: AtomicU64::new(0),
            nonfinite_samples: AtomicU64::new(0),
            recoveries: AtomicU64::new(0),
            peak_queued_inputs: AtomicU64::new(0),
        }
    }
}
static METRICS: OnceLock<Metrics> = OnceLock::new();

/// Initialize on the main thread, before starting audio. Keep the returned guard
/// until audio and the window have stopped so the last record is complete.
pub fn start_from_env() -> std::io::Result<Option<Session>> {
    let Some(directory) = std::env::var_os("CREST_PERF_DIR").filter(|v| !v.is_empty() && v != "0")
    else {
        return Ok(None);
    };
    use std::io::Write;
    std::fs::create_dir_all(&directory)?;
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let path = std::path::PathBuf::from(directory)
        .join(format!("run-{timestamp}-{}.jsonl", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)?;
    let metadata = serde_json::json!({"kind":"session", "schema":1, "pid": std::process::id(),
        "unix_ms":timestamp, "os":std::env::consts::OS, "arch":std::env::consts::ARCH,
        "debug_assertions":cfg!(debug_assertions), "histogram_resolution_ms": BIN_NS as f64 / 1e6,
        "histogram_overflow_ms": (BINS - 1) as f64 * BIN_NS as f64 / 1e6});
    writeln!(file, "{metadata}")?;
    METRICS
        .set(Metrics::default())
        .map_err(|_| std::io::Error::other("Performance session already started"))?;
    let metrics = METRICS.get().expect("initialized on control thread");
    let (stop, receiver) = mpsc::channel();
    eprintln!("PERF · {}", path.display());
    let worker = std::thread::Builder::new().name("crest-performance".into()).spawn(move || -> std::io::Result<()> {
        let start = Instant::now();
        loop {
            let finished = !matches!(receiver.recv_timeout(Duration::from_secs(2)), Err(mpsc::RecvTimeoutError::Timeout));
            let timings: serde_json::Map<_, _> = NAMES.iter().enumerate().map(|(i, name)|
                ((*name).into(), serde_json::json!({"all":metrics.all[i].snapshot(), "playing":metrics.playback[i].snapshot()}))
            ).collect();
            let deadline = metrics.callback_deadline_ns.load(Relaxed);
            let record = serde_json::json!({"kind":if finished {"summary"} else {"sample"},
                "elapsed_seconds":start.elapsed().as_secs_f64(), "playing":metrics.playing.load(Relaxed),
                "timings":timings, "audio_deadline_ms":if deadline == u64::MAX {0.0} else {deadline as f64 / 1e6},
                "rendered_frames":metrics.rendered_frames.load(Relaxed), "late_events":metrics.late_events.load(Relaxed),
                "routing_failures":metrics.routing_failures.load(Relaxed), "voice_refusals":metrics.voice_refusals.load(Relaxed),
                "nonfinite_samples":metrics.nonfinite_samples.load(Relaxed), "playback_recoveries":metrics.recoveries.load(Relaxed),
                "peak_queued_inputs":metrics.peak_queued_inputs.load(Relaxed)});
            writeln!(file, "{record}")?;
            file.flush()?;
            if finished { return Ok(()); }
        }
    })?;
    Ok(Some(Session {
        stop,
        worker: Some(worker),
    }))
}

pub struct Session {
    stop: mpsc::Sender<()>,
    worker: Option<std::thread::JoinHandle<std::io::Result<()>>>,
}
impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(worker) = self.worker.take() {
            if !matches!(worker.join(), Ok(Ok(()))) {
                eprintln!("PERF · metric writer failed; log may be incomplete");
            }
        }
    }
}

pub struct Span {
    metrics: &'static Metrics,
    stage: Stage,
    playing: bool,
    started: Instant,
}
pub fn span(stage: Stage) -> Option<Span> {
    METRICS.get().map(|metrics| Span {
        metrics,
        stage,
        playing: metrics.playing.load(Relaxed),
        started: Instant::now(),
    })
}
impl Drop for Span {
    fn drop(&mut self) {
        record(
            self.metrics,
            self.stage,
            self.playing,
            self.started.elapsed(),
            None,
        );
    }
}
fn record(metrics: &Metrics, stage: Stage, playing: bool, elapsed: Duration, budget: Option<u64>) {
    let ns = elapsed.as_nanos().min(u64::MAX as u128) as u64;
    metrics.all[stage as usize].record(ns, budget);
    if playing {
        metrics.playback[stage as usize].record(ns, budget);
    }
}
pub fn tick_gap(elapsed: Duration) {
    if let Some(metrics) = METRICS.get() {
        record(
            metrics,
            Stage::TickGap,
            metrics.playing.load(Relaxed),
            elapsed,
            Some(33_333_333),
        );
    }
}
pub fn transport(playing: bool, recoveries: u64, queued: usize) {
    if let Some(metrics) = METRICS.get() {
        metrics.playing.store(playing, Relaxed);
        metrics.recoveries.fetch_max(recoveries, Relaxed);
        metrics.peak_queued_inputs.fetch_max(queued as u64, Relaxed);
    }
}
pub fn observe_audio(audio: AudioObservationSnapshot) {
    if let Some(metrics) = METRICS.get() {
        metrics
            .rendered_frames
            .fetch_max(audio.rendered_frames(), Relaxed);
        metrics
            .late_events
            .fetch_max(audio.scheduled_late_events(), Relaxed);
        metrics
            .routing_failures
            .fetch_max(audio.routing_failures(), Relaxed);
        metrics
            .voice_refusals
            .fetch_max(audio.voice_limit_refusals(), Relaxed);
        metrics
            .nonfinite_samples
            .fetch_max(audio.non_finite_samples(), Relaxed);
    }
}

/// Construct off callback. Enabled callbacks perform two monotonic clock reads
/// and bounded atomic updates; no allocation, channels, locks or I/O.
pub fn audio_callback(
    mut render: super::audio_output::AudioRenderCallback,
    sample_rate: f32,
) -> super::audio_output::AudioRenderCallback {
    let Some(metrics) = METRICS.get() else {
        return render;
    };
    Box::new(move |buffer| {
        let playing = metrics.playing.load(Relaxed);
        let start = Instant::now();
        render(buffer);
        let elapsed = start.elapsed();
        let deadline = (buffer.len() as f64 / 2.0 / sample_rate as f64 * 1e9) as u64;
        if deadline > 0 {
            metrics.callback_deadline_ns.fetch_min(deadline, Relaxed);
        }
        record(
            metrics,
            Stage::AudioCallback,
            playing,
            elapsed,
            Some(deadline),
        );
    })
}

/// The page reports synchronous DOM construction/layout time, independently of
/// native frame work. Invalid observations cannot affect application behavior.
pub fn observe_page_render(milliseconds: f64) {
    if milliseconds.is_finite() && (0.0..=60_000.0).contains(&milliseconds) {
        if let Some(metrics) = METRICS.get() {
            record(
                metrics,
                Stage::PageRender,
                metrics.playing.load(Relaxed),
                Duration::from_secs_f64(milliseconds / 1000.0),
                None,
            );
        }
    }
}
pub fn enabled() -> bool {
    METRICS.get().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn histogram_retains_tail_and_budget_failures() {
        let histogram = Histogram::default();
        for _ in 0..99 {
            histogram.record(220_000, Some(500_000));
        }
        histogram.record(200_000_000, Some(500_000));
        let report = histogram.snapshot();
        assert_eq!(report["count"], 100);
        assert_eq!(report["p95_ms"], 0.3);
        assert_eq!(report["max_ms"], 200.0);
        assert_eq!(report["over_budget"], 1);
        let overflow = Histogram::default();
        overflow.record(200_000_000, None);
        assert_eq!(overflow.snapshot()["p99_ms"], 200.0);
    }
    #[test]
    fn idle_work_does_not_contaminate_playback_timings() {
        let metrics = Metrics::default();
        record(
            &metrics,
            Stage::HostTick,
            false,
            Duration::from_secs(1),
            None,
        );
        record(
            &metrics,
            Stage::HostTick,
            true,
            Duration::from_millis(2),
            None,
        );
        assert_eq!(metrics.all[Stage::HostTick as usize].snapshot()["count"], 2);
        assert_eq!(
            metrics.playback[Stage::HostTick as usize].snapshot()["max_ms"],
            2.0
        );
    }
}
