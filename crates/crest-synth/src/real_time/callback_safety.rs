use core::cell::Cell;
use core::sync::atomic::{AtomicUsize, Ordering};

thread_local! {
    static CALLBACK_DEPTH: Cell<usize> = const { Cell::new(0) };
}

static CALLBACK_ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static CALLBACK_DESTRUCTIONS: AtomicUsize = AtomicUsize::new(0);

/// Process-wide measurements collected only while the physical audio callback
/// owns the current thread.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CallbackSafetySnapshot {
    allocations: usize,
    destructions: usize,
}

impl CallbackSafetySnapshot {
    pub const fn allocations(self) -> usize {
        self.allocations
    }

    /// Includes allocator deallocations and explicit prepared-graph drops.
    pub const fn destructions(self) -> usize {
        self.destructions
    }
}

/// Marks the complete adapter-owned physical callback lifetime.
///
/// Entering and leaving use const-initialized thread-local scalar state only.
/// The guard deliberately does not record its own instrumentation-only drop.
pub(crate) struct CallbackSafetyScope;

impl CallbackSafetyScope {
    pub(crate) fn enter() -> Self {
        let _ = CALLBACK_DEPTH.try_with(|depth| depth.set(depth.get().saturating_add(1)));
        Self
    }
}

impl Drop for CallbackSafetyScope {
    fn drop(&mut self) {
        let _ = CALLBACK_DEPTH.try_with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

/// Resets a physical run before its stream can invoke the callback.
pub(crate) fn reset_callback_safety_counts() {
    CALLBACK_ALLOCATIONS.store(0, Ordering::Relaxed);
    CALLBACK_DESTRUCTIONS.store(0, Ordering::Relaxed);
}

/// Takes a coherent-enough monotonic snapshot after callback progress or after
/// stream release. Acceptance only admits the exact all-zero result.
pub(crate) fn callback_safety_snapshot() -> CallbackSafetySnapshot {
    CallbackSafetySnapshot {
        allocations: CALLBACK_ALLOCATIONS.load(Ordering::Relaxed),
        destructions: CALLBACK_DESTRUCTIONS.load(Ordering::Relaxed),
    }
}

/// Hook used by the release binary's global allocator.
#[doc(hidden)]
pub fn record_callback_allocation_from_allocator() {
    if callback_active_on_current_thread() {
        CALLBACK_ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
    }
}

/// Hook used by the release binary's global allocator.
///
/// Heap deallocation is conservatively classified as destruction for the
/// marker's zero-destruction predicate.
#[doc(hidden)]
pub fn record_callback_deallocation_from_allocator() {
    record_callback_owned_destruction();
}

/// Records destruction of a callback-owned resource that need not deallocate.
pub(crate) fn record_callback_owned_destruction() {
    if callback_active_on_current_thread() {
        CALLBACK_DESTRUCTIONS.fetch_add(1, Ordering::Relaxed);
    }
}

fn callback_active_on_current_thread() -> bool {
    CALLBACK_DEPTH
        .try_with(|depth| depth.get() != 0)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::{callback_active_on_current_thread, CallbackSafetyScope};

    #[test]
    fn callback_scope_is_nested_and_thread_local() {
        assert!(!callback_active_on_current_thread());
        {
            let _outer = CallbackSafetyScope::enter();
            assert!(callback_active_on_current_thread());
            {
                let _inner = CallbackSafetyScope::enter();
                assert!(callback_active_on_current_thread());
            }
            assert!(callback_active_on_current_thread());
        }
        assert!(!callback_active_on_current_thread());
    }
}
