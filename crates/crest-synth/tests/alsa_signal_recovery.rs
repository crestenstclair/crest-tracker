//! Real Linux device witness; run with scripts/linux/with-desktop.sh.
#![cfg(target_os = "linux")]

use crest_synth::adapter::cpal_audio_output::CpalAudioOutput;
use crest_synth::shell::audio_output::{AudioOutput, NegotiatedAudioOutput};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

static SIGNALS: AtomicUsize = AtomicUsize::new(0);

extern "C" fn handled_signal(_: libc::c_int) {
    SIGNALS.fetch_add(1, Ordering::Relaxed);
}

struct SignalHandler(libc::sigaction);
impl SignalHandler {
    fn install() -> Self {
        // No SA_RESTART: interrupt the backend's wait, without terminating it.
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        action.sa_sigaction = handled_signal as *const () as usize;
        let mut previous = unsafe { std::mem::zeroed() };
        assert_eq!(unsafe { libc::sigemptyset(&mut action.sa_mask) }, 0);
        assert_eq!(
            unsafe { libc::sigaction(libc::SIGUSR2, &action, &mut previous) },
            0
        );
        Self(previous)
    }
}
impl Drop for SignalHandler {
    fn drop(&mut self) {
        unsafe { libc::sigaction(libc::SIGUSR2, &self.0, std::ptr::null_mut()) };
    }
}

fn wait_for_callback_count(callbacks: &AtomicUsize, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while callbacks.load(Ordering::Relaxed) < count {
        assert!(Instant::now() < deadline, "audio callbacks stopped");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
#[ignore = "requires a Linux audio device: scripts/linux/check.sh"]
fn interrupted_alsa_poll_keeps_audio_running_without_device_failure() {
    let _handler = SignalHandler::install();
    let callbacks = Arc::new(AtomicUsize::new(0));
    let errors = Arc::new(AtomicUsize::new(0));
    let render_callbacks = callbacks.clone();
    let runtime_errors = errors.clone();
    let stream = CpalAudioOutput::new()
        .negotiate()
        .unwrap()
        .start(
            Box::new(move |samples| {
                samples.fill(0.0);
                render_callbacks.fetch_add(1, Ordering::Relaxed);
            }),
            Box::new(move |_| {
                runtime_errors.fetch_add(1, Ordering::Relaxed);
            }),
        )
        .unwrap();
    wait_for_callback_count(&callbacks, 2);
    let workers: Vec<i32> = std::fs::read_dir("/proc/self/task")
        .unwrap()
        .map(Result::unwrap)
        .filter(|entry| {
            std::fs::read_to_string(entry.path().join("comm"))
                .is_ok_and(|name| name.trim() == "cpal_alsa_out")
        })
        .map(|entry| entry.file_name().to_str().unwrap().parse().unwrap())
        .collect();
    assert_eq!(workers.len(), 1, "must target the actual ALSA worker");
    let before = SIGNALS.load(Ordering::Relaxed);
    for _ in 0..100 {
        assert_eq!(
            unsafe { libc::syscall(libc::SYS_tgkill, libc::getpid(), workers[0], libc::SIGUSR2) },
            0
        );
        std::thread::sleep(Duration::from_millis(3));
    }
    assert!(SIGNALS.load(Ordering::Relaxed) > before);
    wait_for_callback_count(&callbacks, callbacks.load(Ordering::Relaxed) + 2);
    drop(stream);
    assert_eq!(
        errors.load(Ordering::Relaxed),
        0,
        "signals are not device failures"
    );
}
