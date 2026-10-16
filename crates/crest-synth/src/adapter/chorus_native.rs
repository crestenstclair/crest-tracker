use core::fmt;
use core::ptr::NonNull;

const NATIVE_OK: i32 = 0;

#[repr(C)]
struct NativeChorus {
    _opaque: [u8; 0],
}

unsafe extern "C" {
    fn crest_chorus_create(max_frames: usize) -> *mut NativeChorus;
    fn crest_chorus_destroy(chorus: *mut NativeChorus);
    fn crest_chorus_process(
        chorus: *mut NativeChorus,
        interleaved_stereo: *mut f32,
        frame_count: usize,
        amount: f32,
        depth: f32,
    ) -> i32;
    fn crest_chorus_instances_created() -> u64;
    fn crest_chorus_instances_destroyed() -> u64;
    fn crest_chorus_instances_active() -> u64;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChorusNativeError {
    AllocationFailed,
    InvalidFrameCount,
    InvalidParameter,
    NativeRejected { status: i32 },
}

impl fmt::Display for ChorusNativeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AllocationFailed => formatter.write_str("Chorus allocation failed"),
            Self::InvalidFrameCount => formatter.write_str("Chorus frame count is invalid"),
            Self::InvalidParameter => formatter.write_str("Chorus parameters are invalid"),
            Self::NativeRejected { status } => {
                write!(
                    formatter,
                    "Chorus native adapter rejected processing with {status}"
                )
            }
        }
    }
}

impl std::error::Error for ChorusNativeError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChorusLifecycleCounts {
    pub created: u64,
    pub destroyed: u64,
    pub active: u64,
}

pub fn chorus_lifecycle_counts() -> ChorusLifecycleCounts {
    // SAFETY: Each function performs one relaxed native atomic load.
    unsafe {
        ChorusLifecycleCounts {
            created: crest_chorus_instances_created(),
            destroyed: crest_chorus_instances_destroyed(),
            active: crest_chorus_instances_active(),
        }
    }
}

/// Unique Rust owner of one initialized native Chorus and its private buffers.
pub(crate) struct ChorusProcessor {
    native: NonNull<NativeChorus>,
    max_frames: usize,
}

// SAFETY: The pointer has unique ownership and all native access requires `&mut self`.
unsafe impl Send for ChorusProcessor {}

impl ChorusProcessor {
    pub(crate) fn new(max_frames: usize) -> Result<Self, ChorusNativeError> {
        if max_frames == 0 {
            return Err(ChorusNativeError::InvalidFrameCount);
        }
        // SAFETY: The wrapper returns null or exclusive ownership of a fully initialized value.
        let native = NonNull::new(unsafe { crest_chorus_create(max_frames) })
            .ok_or(ChorusNativeError::AllocationFailed)?;
        Ok(Self { native, max_frames })
    }

    pub(crate) fn process(
        &mut self,
        interleaved_stereo: &mut [f32],
        frame_count: usize,
        amount: f32,
        depth: f32,
    ) -> Result<(), ChorusNativeError> {
        if frame_count > self.max_frames || interleaved_stereo.len() != frame_count * 2 {
            return Err(ChorusNativeError::InvalidFrameCount);
        }
        if !amount.is_finite()
            || !depth.is_finite()
            || !(0.0..=1.0).contains(&amount)
            || !(0.0..=1.0).contains(&depth)
        {
            return Err(ChorusNativeError::InvalidParameter);
        }
        // SAFETY: The native instance is exclusively borrowed and the exact mutable slice is live.
        let status = unsafe {
            crest_chorus_process(
                self.native.as_ptr(),
                interleaved_stereo.as_mut_ptr(),
                frame_count,
                amount,
                depth,
            )
        };
        if status == NATIVE_OK {
            Ok(())
        } else {
            Err(ChorusNativeError::NativeRejected { status })
        }
    }
}

impl Drop for ChorusProcessor {
    fn drop(&mut self) {
        // SAFETY: This is the unique owner and destroys the instance exactly once.
        unsafe { crest_chorus_destroy(self.native.as_ptr()) }
    }
}
