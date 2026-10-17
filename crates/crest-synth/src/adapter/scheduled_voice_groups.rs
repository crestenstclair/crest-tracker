//! Control owns admission/cancellation. Audio only reads preallocated atomics;
//! cancellation never depends on free event-queue capacity.
use crate::real_time::{VoiceGroupError, VoiceGroupId};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};

// Process-wide identities also reject tokens accidentally passed to a different
// boundary. Exhaustion is explicit; neither identity nor storage reuse wraps.
static NEXT_GROUP: AtomicU64 = AtomicU64::new(1);

struct Shared {
    current: Box<[AtomicU64]>,
    changed: AtomicBool,
}
impl Shared {
    fn contains(&self, id: VoiceGroupId) -> bool {
        self.current
            .iter()
            .any(|slot| slot.load(Ordering::Acquire) == id.get())
    }
}
pub(super) struct GroupControl(Arc<Shared>);
pub(super) struct GroupAudio(Arc<Shared>);

pub(super) fn new(capacity: usize) -> Result<(GroupControl, GroupAudio), VoiceGroupError> {
    if capacity == 0 {
        return Err(VoiceGroupError::InvalidCapacity);
    }
    let mut current = Vec::new();
    current
        .try_reserve_exact(capacity)
        .map_err(|_| VoiceGroupError::StorageAllocationFailed)?;
    current.resize_with(capacity, || AtomicU64::new(0));
    let shared = Arc::new(Shared {
        current: current.into_boxed_slice(),
        changed: AtomicBool::new(false),
    });
    Ok((GroupControl(Arc::clone(&shared)), GroupAudio(shared)))
}
impl GroupControl {
    pub(super) fn create(&mut self) -> Result<VoiceGroupId, VoiceGroupError> {
        let slot = self
            .0
            .current
            .iter()
            .find(|slot| slot.load(Ordering::Relaxed) == 0)
            .ok_or(VoiceGroupError::Full)?;
        let value = NEXT_GROUP
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
                next.checked_add(1)
            })
            .map_err(|_| VoiceGroupError::IdentitiesExhausted)?;
        let id = VoiceGroupId::new(value).ok_or(VoiceGroupError::IdentitiesExhausted)?;
        slot.store(value, Ordering::Release);
        self.0.changed.store(true, Ordering::Release);
        Ok(id)
    }
    pub(super) fn cancel(&mut self, id: VoiceGroupId) -> Result<(), VoiceGroupError> {
        let slot = self
            .0
            .current
            .iter()
            .find(|slot| slot.load(Ordering::Relaxed) == id.get())
            .ok_or(VoiceGroupError::Unknown)?;
        slot.store(0, Ordering::Release);
        self.0.changed.store(true, Ordering::Release);
        Ok(())
    }
    pub(super) fn clear(&mut self) {
        for slot in &self.0.current {
            slot.store(0, Ordering::Release);
        }
        self.0.changed.store(true, Ordering::Release);
    }
    pub(super) fn contains(&self, id: VoiceGroupId) -> bool {
        self.0.contains(id)
    }
}
impl GroupAudio {
    pub(super) fn capacity(&self) -> usize {
        self.0.current.len()
    }
    /// Coalesced notification, cleared before reading slots. A concurrent
    /// control update leaves another notification for the next observation.
    pub(super) fn take_changed(&self) -> bool {
        self.0.changed.swap(false, Ordering::AcqRel)
    }
    pub(super) fn at(&self, index: usize) -> Option<VoiceGroupId> {
        self.0
            .current
            .get(index)
            .and_then(|slot| VoiceGroupId::new(slot.load(Ordering::Acquire)))
    }
    pub(super) fn contains(&self, id: VoiceGroupId) -> bool {
        self.0.contains(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_and_storage_reuse_never_revive_old_or_foreign_groups() {
        let (mut control, audio) = new(1).unwrap();
        assert_eq!(audio.capacity(), 1);
        assert!(!audio.take_changed());
        let first = control.create().unwrap();
        assert!(audio.take_changed());
        assert_eq!(audio.at(0), Some(first));
        assert!(audio.contains(first));
        assert_eq!(control.create(), Err(VoiceGroupError::Full));
        assert!(!audio.take_changed());
        let (mut other, _) = new(1).unwrap();
        let foreign = other.create().unwrap();
        assert_eq!(control.cancel(foreign), Err(VoiceGroupError::Unknown));
        assert!(!control.contains(foreign));
        control.cancel(first).unwrap();
        let next = control.create().unwrap();
        assert_ne!(first, next);
        assert!(audio.take_changed());
        assert!(!audio.contains(first));
        assert_eq!(audio.at(0), Some(next));
        assert_eq!(control.cancel(first), Err(VoiceGroupError::Unknown));
        assert!(audio.contains(next));
        control.clear();
        assert!(audio.take_changed());
        assert!(!audio.contains(next));
        assert_eq!(audio.at(0), None);
        assert_eq!(audio.at(1), None);
    }

    #[test]
    fn canceling_one_group_preserves_all_other_owners() {
        assert!(matches!(new(0), Err(VoiceGroupError::InvalidCapacity)));
        let (mut control, audio) = new(3).unwrap();
        let first = control.create().unwrap();
        let second = control.create().unwrap();
        let third = control.create().unwrap();
        control.cancel(second).unwrap();
        assert!(audio.contains(first));
        assert!(!audio.contains(second));
        assert!(audio.contains(third));
        assert!(audio.take_changed());
        assert!(!audio.take_changed());
    }
}
