//! Test output replaces only the device. The supplied callback is the production renderer.
use super::*;
use crest_synth::shell::audio_output::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static MEASURING: Cell<bool> = const { Cell::new(false) };
    static MEMORY: Cell<(u64, u64)> = const { Cell::new((0, 0)) };
}
struct CountingAllocator;
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;
fn count(allocated: u64, freed: u64) {
    let _ = MEASURING.try_with(|active| {
        if active.get() {
            MEMORY.with(|memory| {
                let (a, f) = memory.get();
                memory.set((a + allocated, f + freed));
            });
        }
    });
}
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count(1, 0);
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count(1, 0);
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        count(0, 1);
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count(1, 1);
        unsafe { System.realloc(pointer, layout, size) }
    }
}

#[derive(Clone)]
pub(super) struct Output {
    pub block_frames: usize,
    pub callback: Rc<RefCell<Option<AudioRenderCallback>>>,
    writer: Rc<RefCell<AudioFrameClockWriter>>,
    reader: AudioFrameClockReader,
    frames: Rc<Cell<u64>>,
    recording: Rc<RefCell<Option<Vec<f32>>>>,
}
impl Default for Output {
    fn default() -> Self {
        let (writer, reader) = AudioFrameClockBoundary::create_pair();
        Self {
            block_frames: FRAMES,
            callback: Rc::default(),
            writer: Rc::new(RefCell::new(writer)),
            reader,
            frames: Rc::default(),
            recording: Rc::default(),
        }
    }
}
impl AudioOutput for Output {
    type Negotiated = Self;
    fn negotiate(self) -> Result<Self, AudioOutputError> {
        Ok(self)
    }
}
impl NegotiatedAudioOutput for Output {
    fn config(&self) -> AudioDeviceConfig {
        AudioDeviceConfig::new(48_000.0, 2, AudioSampleFormat::F32, self.block_frames).unwrap()
    }
    fn start(
        self,
        render: AudioRenderCallback,
        _on_error: AudioDeviceStatusCallback,
    ) -> Result<AudioStream, AudioOutputError> {
        *self.callback.borrow_mut() = Some(render);
        Ok(AudioStream::with_clock((), self.reader))
    }
}
impl Output {
    pub(super) fn with_block_frames(block_frames: usize) -> Self {
        Self {
            block_frames,
            ..Self::default()
        }
    }
    pub(super) fn record(&self) {
        *self.recording.borrow_mut() = Some(Vec::new());
    }
    pub(super) fn take_recording(&self) -> Vec<f32> {
        self.recording.borrow_mut().take().unwrap_or_default()
    }
    pub(super) fn render(&self, buffer: &mut [f32]) -> (u64, (u64, u64)) {
        let mut callback = self.callback.borrow_mut();
        MEMORY.with(|memory| memory.set((0, 0)));
        MEASURING.with(|active| active.set(true));
        let start = std::time::Instant::now();
        callback.as_mut().expect("production callback started")(buffer);
        let elapsed = start.elapsed().as_nanos() as u64;
        MEASURING.with(|active| active.set(false));
        // This optional witness capture runs outside the callback and its RT audit.
        if let Some(recording) = &mut *self.recording.borrow_mut() {
            recording.extend_from_slice(buffer);
        }
        let frame = self.frames.get();
        let micros = 1_000_000 + frame * 1_000_000 / 48_000;
        self.writer.borrow_mut().publish_from_callback(
            AudioFrameClock::new(
                frame,
                (buffer.len() / 2) as u64,
                micros,
                micros,
                48_000,
                AudioClockDomain::CoreAudioHost,
            )
            .unwrap(),
        );
        self.frames.set(frame + (buffer.len() / 2) as u64);
        (elapsed, MEMORY.with(Cell::get))
    }
}
