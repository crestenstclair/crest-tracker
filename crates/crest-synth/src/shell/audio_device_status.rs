use crate::shell::audio_output::AudioDeviceRuntimeError;
use core::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

/// Preallocated first-failure handoff from a device callback to control ownership.
pub struct AudioDeviceStatusBoundary {
    shared: Arc<AtomicU8>,
}

impl AudioDeviceStatusBoundary {
    #[must_use]
    pub fn new() -> Self {
        Self {
            shared: Arc::new(AtomicU8::new(0)),
        }
    }

    #[must_use]
    pub fn into_handles(self) -> (AudioDeviceStatusWriter, AudioDeviceStatusReader) {
        (
            AudioDeviceStatusWriter {
                shared: Arc::clone(&self.shared),
            },
            AudioDeviceStatusReader {
                shared: self.shared,
            },
        )
    }
}

impl Default for AudioDeviceStatusBoundary {
    fn default() -> Self {
        Self::new()
    }
}

/// Callback-only fixed-size failure publisher.
pub struct AudioDeviceStatusWriter {
    shared: Arc<AtomicU8>,
}

impl AudioDeviceStatusWriter {
    /// Retains the first unconsumed failure. This operation is bounded and
    /// performs no allocation, locking, blocking, I/O, logging, or formatting.
    pub fn publish_from_callback(&mut self, error: AudioDeviceRuntimeError) {
        let _ = self
            .shared
            .compare_exchange(0, error as u8, Ordering::Release, Ordering::Relaxed);
    }
}

/// Control-only failure consumer.
#[derive(Clone)]
pub struct AudioDeviceStatusReader {
    shared: Arc<AtomicU8>,
}

impl AudioDeviceStatusReader {
    /// Takes the current failure so a later device failure can be observed.
    pub fn take_on_control(&self) -> Option<AudioDeviceRuntimeError> {
        AudioDeviceRuntimeError::from_code(self.shared.swap(0, Ordering::AcqRel))
    }
}

#[cfg(test)]
mod tests {
    use super::AudioDeviceStatusBoundary;
    use crate::shell::audio_output::AudioDeviceRuntimeError;

    #[test]
    fn first_runtime_failure_is_bounded_visible_and_consumable() {
        let (mut writer, reader) = AudioDeviceStatusBoundary::new().into_handles();
        writer.publish_from_callback(AudioDeviceRuntimeError::DeviceUnavailable);
        writer.publish_from_callback(AudioDeviceRuntimeError::Xrun);

        assert_eq!(
            reader.take_on_control(),
            Some(AudioDeviceRuntimeError::DeviceUnavailable)
        );
        assert_eq!(reader.take_on_control(), None);

        writer.publish_from_callback(AudioDeviceRuntimeError::Xrun);
        assert_eq!(
            reader.take_on_control(),
            Some(AudioDeviceRuntimeError::Xrun)
        );
    }

    #[test]
    fn concrete_handles_are_send_and_have_no_destructor_work() {
        fn assert_send<T: Send>() {}

        assert_send::<super::AudioDeviceStatusWriter>();
        assert_send::<super::AudioDeviceStatusReader>();
        assert!(!core::mem::needs_drop::<AudioDeviceRuntimeError>());
    }
}
