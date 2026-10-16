use core::fmt;
use core::ptr::NonNull;

pub const BRAIDS_VOICE_COUNT: usize = 16;
pub const BRAIDS_MODEL_COUNT: u8 = 47;
pub const BRAIDS_INTERNAL_CHUNK_FRAMES: usize = 24;

const NATIVE_OK: i32 = 0;

#[repr(C)]
struct NativeBraidsBank {
    _opaque: [u8; 0],
}

unsafe extern "C" {
    fn crest_braids_bank_create(voice_count: usize) -> *mut NativeBraidsBank;
    fn crest_braids_bank_destroy(bank: *mut NativeBraidsBank);
    fn crest_braids_voice_count(bank: *const NativeBraidsBank) -> usize;
    fn crest_braids_voice_reset(bank: *mut NativeBraidsBank, voice: usize) -> i32;
    fn crest_braids_voice_configure(
        bank: *mut NativeBraidsBank,
        voice: usize,
        model: u8,
        pitch: i16,
        timbre: i16,
        color: i16,
    ) -> i32;
    fn crest_braids_voice_strike(bank: *mut NativeBraidsBank, voice: usize) -> i32;
    fn crest_braids_voice_render(
        bank: *mut NativeBraidsBank,
        voice: usize,
        output: *mut i16,
        frame_count: usize,
    ) -> i32;
    fn crest_braids_banks_created() -> u64;
    fn crest_braids_banks_destroyed() -> u64;
    fn crest_braids_banks_active() -> u64;
}

/// Fixed-size native adapter failure. Callback-side methods return this value
/// without allocating, formatting, panicking, or constructing owned data.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BraidsNativeError {
    AllocationFailed,
    InvalidVoice { voice: usize },
    InvalidModel { model: u8 },
    InvalidFrameCount { frame_count: usize },
    NativeRejected { status: i32 },
}

impl fmt::Display for BraidsNativeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::AllocationFailed => formatter.write_str("Braids voice bank allocation failed"),
            Self::InvalidVoice { voice } => {
                write!(formatter, "Braids voice index {voice} is out of range")
            }
            Self::InvalidModel { model } => {
                write!(formatter, "Braids model index {model} is out of range")
            }
            Self::InvalidFrameCount { frame_count } => write!(
                formatter,
                "Braids internal render size {frame_count} is outside 1..={BRAIDS_INTERNAL_CHUNK_FRAMES}"
            ),
            Self::NativeRejected { status } => {
                write!(formatter, "Braids native adapter rejected operation with status {status}")
            }
        }
    }
}

impl std::error::Error for BraidsNativeError {}

/// Process-wide ownership counters maintained entirely by the native boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BraidsLifecycleCounts {
    pub created: u64,
    pub destroyed: u64,
    pub active: u64,
}

pub fn braids_lifecycle_counts() -> BraidsLifecycleCounts {
    // SAFETY: These functions have no parameters and only perform relaxed
    // atomic loads in the linked wrapper.
    unsafe {
        BraidsLifecycleCounts {
            created: crest_braids_banks_created(),
            destroyed: crest_braids_banks_destroyed(),
            active: crest_braids_banks_active(),
        }
    }
}

/// Unique Rust owner of one native, initialized oscillator bank with prepared capacity.
pub(crate) struct BraidsVoiceBank {
    native: NonNull<NativeBraidsBank>,
    capacity: usize,
}

// SAFETY: Ownership is unique, no native pointer escapes, and all access
// requires `&mut self`. Prepared instruments move between threads but are not
// concurrently shared.
unsafe impl Send for BraidsVoiceBank {}

impl BraidsVoiceBank {
    #[cfg(test)]
    pub(crate) fn new() -> Result<Self, BraidsNativeError> {
        Self::with_capacity(BRAIDS_VOICE_COUNT)
    }

    pub(crate) fn with_capacity(capacity: usize) -> Result<Self, BraidsNativeError> {
        // SAFETY: The wrapper returns either null or exclusive ownership of a
        // fully initialized bank.
        let native = unsafe { crest_braids_bank_create(capacity) };
        let native = NonNull::new(native).ok_or(BraidsNativeError::AllocationFailed)?;
        debug_assert_eq!(
            unsafe { crest_braids_voice_count(native.as_ptr()) },
            capacity
        );
        Ok(Self { native, capacity })
    }

    pub(crate) fn reset(&mut self, voice: usize) -> Result<(), BraidsNativeError> {
        validate_voice(voice, self.capacity)?;
        // SAFETY: `self` uniquely owns a live native bank and the index was
        // checked against the wrapper's fixed capacity.
        native_status(unsafe { crest_braids_voice_reset(self.native.as_ptr(), voice) })
    }

    pub(crate) fn configure(
        &mut self,
        voice: usize,
        model: u8,
        pitch: i16,
        timbre: i16,
        color: i16,
    ) -> Result<(), BraidsNativeError> {
        validate_voice(voice, self.capacity)?;
        if model >= BRAIDS_MODEL_COUNT {
            return Err(BraidsNativeError::InvalidModel { model });
        }
        // SAFETY: The bank, voice, and model were validated; remaining inputs
        // are represented exactly by the C ABI.
        native_status(unsafe {
            crest_braids_voice_configure(self.native.as_ptr(), voice, model, pitch, timbre, color)
        })
    }

    pub(crate) fn strike(&mut self, voice: usize) -> Result<(), BraidsNativeError> {
        validate_voice(voice, self.capacity)?;
        // SAFETY: `self` uniquely owns a live bank and the index is valid.
        native_status(unsafe { crest_braids_voice_strike(self.native.as_ptr(), voice) })
    }

    pub(crate) fn render(
        &mut self,
        voice: usize,
        output: &mut [i16],
    ) -> Result<(), BraidsNativeError> {
        validate_voice(voice, self.capacity)?;
        if output.is_empty() || output.len() > BRAIDS_INTERNAL_CHUNK_FRAMES {
            return Err(BraidsNativeError::InvalidFrameCount {
                frame_count: output.len(),
            });
        }
        // SAFETY: The bank is exclusively borrowed, the voice is valid, and
        // the mutable slice is non-empty and bounded to the native maximum.
        native_status(unsafe {
            crest_braids_voice_render(
                self.native.as_ptr(),
                voice,
                output.as_mut_ptr(),
                output.len(),
            )
        })
    }
}

impl Drop for BraidsVoiceBank {
    fn drop(&mut self) {
        // SAFETY: `BraidsVoiceBank` is the unique owner and calls destroy once.
        unsafe { crest_braids_bank_destroy(self.native.as_ptr()) }
    }
}

fn validate_voice(voice: usize, capacity: usize) -> Result<(), BraidsNativeError> {
    if voice >= capacity {
        Err(BraidsNativeError::InvalidVoice { voice })
    } else {
        Ok(())
    }
}

fn native_status(status: i32) -> Result<(), BraidsNativeError> {
    if status == NATIVE_OK {
        Ok(())
    } else {
        Err(BraidsNativeError::NativeRejected { status })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bank_owns_exactly_sixteen_initialized_voices_and_native_lifecycle() {
        let before = braids_lifecycle_counts();
        let mut bank = BraidsVoiceBank::new().unwrap();
        let during = braids_lifecycle_counts();
        assert!(during.created > before.created);

        bank.configure(15, 46, 60 * 128, 16_384, 8_192).unwrap();
        bank.strike(15).unwrap();
        let mut output = [0_i16; BRAIDS_INTERNAL_CHUNK_FRAMES];
        bank.render(15, &mut output).unwrap();
        assert!(output.iter().any(|sample| *sample != 0));
        drop(bank);

        let after = braids_lifecycle_counts();
        assert!(after.destroyed > before.destroyed);
    }

    #[test]
    fn index_model_and_chunk_failures_are_typed_before_native_dispatch() {
        let mut bank = BraidsVoiceBank::new().unwrap();
        assert_eq!(
            bank.reset(BRAIDS_VOICE_COUNT),
            Err(BraidsNativeError::InvalidVoice {
                voice: BRAIDS_VOICE_COUNT
            })
        );
        assert_eq!(
            bank.configure(0, BRAIDS_MODEL_COUNT, 0, 0, 0),
            Err(BraidsNativeError::InvalidModel {
                model: BRAIDS_MODEL_COUNT
            })
        );
        assert_eq!(
            bank.render(0, &mut []),
            Err(BraidsNativeError::InvalidFrameCount { frame_count: 0 })
        );
        assert!(!core::mem::needs_drop::<BraidsNativeError>());
    }
}
