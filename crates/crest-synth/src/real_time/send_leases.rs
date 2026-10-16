//! Temporary Patch send overrides, separate from saved scalar snapshots.
//!
//! Construct the bank off callback. All later operations scan fixed storage;
//! entries are Copy and never own resources requiring callback destruction.

use crate::kernel::PatchId;
use crate::mixer::bus_id::BusId;
use crate::mixer::mixer_track_parameters::BUS_SEND_DESCRIPTOR;
use core::num::{NonZeroU64, NonZeroUsize};

/// Host-assigned identity for one temporary send occurrence. Identities must
/// not be reused while an earlier occurrence can still deliver an end event.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SendLeaseId(NonZeroU64);

impl SendLeaseId {
    pub const fn new(value: u64) -> Option<Self> {
        match NonZeroU64::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

/// A validated temporary send value and its exclusive renderer-frame expiry.
/// The scheduled envelope supplies the Patch and start frame. An end frame is
/// admitted only when it is still in the future at `SendLeases::begin`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SendLease {
    id: SendLeaseId,
    bus: BusId,
    amount_bits: u32,
    end_frame: u64,
}

impl SendLease {
    pub fn new(
        id: SendLeaseId,
        bus: BusId,
        amount: f32,
        end_frame: u64,
    ) -> Result<Self, SendLeaseError> {
        if !BUS_SEND_DESCRIPTOR.contains(amount) {
            return Err(SendLeaseError::InvalidAmount);
        }
        Ok(Self {
            id,
            bus,
            // Both signed zeros represent the same silent override.
            amount_bits: if amount == 0.0 { 0 } else { amount.to_bits() },
            end_frame,
        })
    }

    pub const fn id(self) -> SendLeaseId {
        self.id
    }

    pub const fn bus(self) -> BusId {
        self.bus
    }

    pub const fn amount(self) -> f32 {
        f32::from_bits(self.amount_bits)
    }

    pub const fn end_frame(self) -> u64 {
        self.end_frame
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Entry {
    patch_id: PatchId,
    lease: SendLease,
    sequence: u64,
}

/// Fixed-capacity callback storage. The most recently admitted active lease
/// wins for each `(PatchId, BusId)`. Absence means the mixer must read its latest
/// saved scalar snapshot; this bank never captures or restores baseline values.
pub struct SendLeases {
    entries: Box<[Option<Entry>]>,
    last_sequence: u64,
}

impl SendLeases {
    /// Allocates the complete configurable resource budget off callback.
    pub fn new(capacity: NonZeroUsize) -> Result<Self, SendLeaseError> {
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(capacity.get())
            .map_err(|_| SendLeaseError::StorageAllocationFailed)?;
        entries.resize(capacity.get(), None);
        Ok(Self {
            entries: entries.into_boxed_slice(),
            last_sequence: 0,
        })
    }

    pub fn capacity(&self) -> usize {
        self.entries.len()
    }

    /// Admits one lease after reclaiming expired slots. A rejected admission
    /// never displaces another active lease. The caller validates that the
    /// Patch and bus exist in the current graph before calling this method.
    pub fn begin(
        &mut self,
        patch_id: PatchId,
        lease: SendLease,
        at: u64,
    ) -> Result<(), SendLeaseError> {
        if lease.end_frame <= at {
            return Err(SendLeaseError::AlreadyExpired {
                end_frame: lease.end_frame,
                current_frame: at,
            });
        }
        self.expire(at);
        if self
            .entries
            .iter()
            .flatten()
            .any(|entry| entry.patch_id == patch_id && entry.lease.id == lease.id)
        {
            return Err(SendLeaseError::Duplicate {
                patch_id,
                lease_id: lease.id,
            });
        }
        let slot = self
            .entries
            .iter_mut()
            .find(|entry| entry.is_none())
            .ok_or(SendLeaseError::Full)?;
        let sequence = self
            .last_sequence
            .checked_add(1)
            .ok_or(SendLeaseError::SequenceExhausted)?;
        *slot = Some(Entry {
            patch_id,
            lease,
            sequence,
        });
        self.last_sequence = sequence;
        Ok(())
    }

    /// Releases exactly one owner even when the bank is full. Missing or stale
    /// end events are harmless. Ending the newest override reveals the next
    /// active one; ending an older override leaves the newest unchanged.
    pub fn end(&mut self, patch_id: PatchId, lease_id: SendLeaseId) -> bool {
        let Some(slot) = self.entries.iter_mut().find(|slot| {
            slot.is_some_and(|entry| entry.patch_id == patch_id && entry.lease.id == lease_id)
        }) else {
            return false;
        };
        *slot = None;
        if self.entries.iter().all(Option::is_none) {
            self.last_sequence = 0;
        }
        true
    }

    /// Removes every lease ending at or before this absolute frame. Call at
    /// each render boundary, including boundaries without scheduled commands.
    pub fn expire(&mut self, frame: u64) -> usize {
        let mut expired = 0;
        let mut any_active = false;
        for slot in &mut self.entries {
            if slot.is_some_and(|entry| entry.lease.end_frame <= frame) {
                *slot = None;
                expired += 1;
            }
            any_active |= slot.is_some();
        }
        if !any_active {
            self.last_sequence = 0;
        }
        expired
    }

    pub fn next_expiry(&self) -> Option<u64> {
        self.entries
            .iter()
            .flatten()
            .map(|entry| entry.lease.end_frame)
            .min()
    }

    /// Returns the current override, including an explicit silent `Some(0.0)`.
    /// The renderer must expire elapsed leases before querying a new segment.
    pub fn amount(&self, patch_id: PatchId, bus: BusId) -> Option<f32> {
        self.entries
            .iter()
            .flatten()
            .filter(|entry| entry.patch_id == patch_id && entry.lease.bus == bus)
            .max_by_key(|entry| entry.sequence)
            .map(|entry| entry.lease.amount())
    }

    /// Epoch, Stop, and graph-replacement cleanup retains all allocated storage.
    pub fn clear(&mut self) {
        self.entries.fill(None);
        self.last_sequence = 0;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SendLeaseError {
    #[error("temporary send storage could not be allocated")]
    StorageAllocationFailed,
    #[error("temporary send amount must be finite and between zero and one")]
    InvalidAmount,
    #[error("temporary send ends at {end_frame}, not after current frame {current_frame}")]
    AlreadyExpired { end_frame: u64, current_frame: u64 },
    #[error("temporary send lease {lease_id:?} is already active for Patch {patch_id}")]
    Duplicate {
        patch_id: PatchId,
        lease_id: SendLeaseId,
    },
    #[error("temporary send lease capacity is exhausted")]
    Full,
    #[error("temporary send ordering sequence is exhausted")]
    SequenceExhausted,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bank(capacity: usize) -> SendLeases {
        SendLeases::new(NonZeroUsize::new(capacity).unwrap()).unwrap()
    }

    fn lease(amount: f32, end_frame: u64) -> SendLease {
        SendLease::new(
            SendLeaseId::new(end_frame).unwrap(),
            BusId::default(),
            amount,
            end_frame,
        )
        .unwrap()
    }

    #[test]
    fn latest_override_reveals_current_base_after_older_expiry_and_manual_edit() {
        let patch = PatchId::new(1).unwrap();
        let bus = BusId::default();
        let mut leases = bank(2);
        let mut base = 0.2;
        assert_eq!(leases.amount(patch, bus).unwrap_or(base), 0.2);
        leases.begin(patch, lease(0.35, 10), 1).unwrap();
        assert_eq!(leases.amount(patch, bus).unwrap_or(base), 0.35);
        leases.begin(patch, lease(0.6, 20), 2).unwrap();
        assert_eq!(leases.amount(patch, bus).unwrap_or(base), 0.6);
        assert_eq!(leases.next_expiry(), Some(10));
        assert_eq!(leases.expire(9), 0);
        assert_eq!(leases.expire(10), 1);
        assert_eq!(leases.amount(patch, bus).unwrap_or(base), 0.6);
        base = 0.25;
        assert_eq!(leases.amount(patch, bus).unwrap_or(base), 0.6);
        assert_eq!(leases.next_expiry(), Some(20));
        assert_eq!(leases.expire(20), 1);
        assert_eq!(leases.amount(patch, bus).unwrap_or(base), 0.25);
        assert_eq!(leases.next_expiry(), None);
    }

    #[test]
    fn newest_expiry_reveals_older_override_and_reused_slots_keep_fifo_priority() {
        let patch = PatchId::new(1).unwrap();
        let bus = BusId::default();
        let mut leases = bank(3);
        leases.begin(patch, lease(0.1, 4), 0).unwrap();
        leases.begin(patch, lease(0.2, 30), 0).unwrap();
        leases.begin(patch, lease(0.3, 10), 0).unwrap();
        assert_eq!(leases.amount(patch, bus), Some(0.3));
        // Admission reclaims slot zero, but its newest insertion must win.
        leases.begin(patch, lease(0.4, 8), 4).unwrap();
        assert_eq!(leases.amount(patch, bus), Some(0.4));
        leases.expire(8);
        assert_eq!(leases.amount(patch, bus), Some(0.3));
        leases.expire(10);
        assert_eq!(leases.amount(patch, bus), Some(0.2));
        leases.expire(30);
        assert_eq!(leases.amount(patch, bus), None);
    }

    #[test]
    fn patch_and_bus_ownership_are_exact_and_zero_is_an_override() {
        let first = PatchId::new(1).unwrap();
        let second = PatchId::new(2).unwrap();
        let bus = BusId::new(32).unwrap();
        let mut leases = bank(3);
        leases.begin(first, lease(0.0, 10), 0).unwrap();
        leases.begin(second, lease(0.8, 10), 0).unwrap();
        leases
            .begin(
                first,
                SendLease::new(SendLeaseId::new(20).unwrap(), bus, 0.9, 20).unwrap(),
                0,
            )
            .unwrap();
        assert_eq!(leases.amount(first, BusId::default()), Some(0.0));
        assert_eq!(leases.amount(second, BusId::default()), Some(0.8));
        assert_eq!(leases.amount(first, bus), Some(0.9));
        assert_eq!(leases.amount(second, bus), None);
    }

    #[test]
    fn capacity_and_expired_admission_never_displace_active_entries() {
        let patch = PatchId::new(1).unwrap();
        let mut leases = bank(1);
        leases.begin(patch, lease(0.5, 10), 0).unwrap();
        assert_eq!(
            leases.begin(patch, lease(0.8, 20), 1),
            Err(SendLeaseError::Full)
        );
        assert_eq!(
            leases.begin(patch, lease(0.9, 1), 1),
            Err(SendLeaseError::AlreadyExpired {
                end_frame: 1,
                current_frame: 1
            })
        );
        assert_eq!(leases.amount(patch, BusId::default()), Some(0.5));
        leases.begin(patch, lease(0.7, 20), 10).unwrap();
        assert_eq!(leases.amount(patch, BusId::default()), Some(0.7));
    }

    #[test]
    fn validation_and_order_exhaustion_are_typed_without_wraparound() {
        for amount in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.1, 1.1] {
            assert_eq!(
                SendLease::new(SendLeaseId::new(1).unwrap(), BusId::default(), amount, 10),
                Err(SendLeaseError::InvalidAmount)
            );
        }
        assert_eq!(lease(-0.0, 10), lease(0.0, 10));
        assert_eq!(lease(1.0, u64::MAX).amount(), 1.0);
        let patch = PatchId::new(1).unwrap();
        let mut leases = bank(2);
        leases.begin(patch, lease(0.5, 10), 0).unwrap();
        leases.last_sequence = u64::MAX;
        assert_eq!(
            leases.begin(patch, lease(0.8, 20), 1),
            Err(SendLeaseError::SequenceExhausted)
        );
        assert_eq!(leases.amount(patch, BusId::default()), Some(0.5));
        leases.expire(10);
        leases.begin(patch, lease(0.8, 20), 10).unwrap();
    }

    #[test]
    fn explicit_end_removes_only_its_patch_owner_even_when_storage_is_full() {
        let first_patch = PatchId::new(1).unwrap();
        let second_patch = PatchId::new(2).unwrap();
        let first_id = SendLeaseId::new(1).unwrap();
        let second_id = SendLeaseId::new(2).unwrap();
        let missing_id = SendLeaseId::new(3).unwrap();
        let bus = BusId::default();
        let mut leases = bank(3);
        let storage = leases.entries.as_ptr();
        leases
            .begin(
                first_patch,
                SendLease::new(first_id, bus, 0.35, u64::MAX).unwrap(),
                0,
            )
            .unwrap();
        leases
            .begin(
                first_patch,
                SendLease::new(second_id, bus, 0.6, u64::MAX).unwrap(),
                0,
            )
            .unwrap();
        leases
            .begin(
                second_patch,
                SendLease::new(first_id, bus, 0.9, u64::MAX).unwrap(),
                0,
            )
            .unwrap();
        assert!(!leases.end(first_patch, missing_id));
        assert!(!leases.end(second_patch, second_id));
        assert!(leases.end(first_patch, first_id));
        assert_eq!(leases.amount(first_patch, bus), Some(0.6));
        assert_eq!(leases.amount(second_patch, bus), Some(0.9));
        assert!(!leases.end(first_patch, first_id));
        assert!(leases.end(first_patch, second_id));
        assert_eq!(leases.amount(first_patch, bus), None);
        assert!(leases.end(second_patch, first_id));
        assert_eq!(leases.next_expiry(), None);
        assert_eq!(leases.last_sequence, 0);
        assert_eq!(leases.entries.as_ptr(), storage);
    }

    #[test]
    fn duplicate_identity_is_rejected_and_newest_end_reveals_older_lease() {
        let patch = PatchId::new(1).unwrap();
        let original = lease(0.35, 10);
        let newest = lease(0.6, 20);
        let mut leases = bank(2);
        leases.begin(patch, original, 0).unwrap();
        // A different bus or amount cannot turn the same active ID into a new owner.
        let duplicate = SendLease::new(original.id(), BusId::new(1).unwrap(), 0.9, 30).unwrap();
        assert_eq!(
            leases.begin(patch, duplicate, 0),
            Err(SendLeaseError::Duplicate {
                patch_id: patch,
                lease_id: original.id()
            })
        );
        leases.begin(patch, newest, 0).unwrap();
        assert_eq!(
            leases.begin(patch, duplicate, 0),
            Err(SendLeaseError::Duplicate {
                patch_id: patch,
                lease_id: original.id()
            })
        );
        assert!(leases.end(patch, newest.id()));
        assert_eq!(leases.amount(patch, BusId::default()), Some(0.35));
        leases.expire(10);
        assert!(!leases.end(patch, original.id()));
        assert_eq!(SendLeaseId::new(0), None);
        assert_eq!(SendLeaseId::new(u64::MAX).unwrap().get(), u64::MAX);
    }

    #[test]
    fn clear_reuses_storage_and_unrepresentable_capacity_fails_before_allocation() {
        assert!(!core::mem::needs_drop::<SendLeaseId>());
        assert!(!core::mem::needs_drop::<SendLease>());
        assert!(!core::mem::needs_drop::<Entry>());
        let mut leases = bank(2);
        let storage = leases.entries.as_ptr();
        let patch = PatchId::new(1).unwrap();
        leases.begin(patch, lease(0.5, 10), 0).unwrap();
        leases.clear();
        assert_eq!(leases.next_expiry(), None);
        assert_eq!(leases.amount(patch, BusId::default()), None);
        leases.begin(patch, lease(0.8, 20), 0).unwrap();
        assert_eq!(leases.entries.as_ptr(), storage);
        assert_eq!(leases.capacity(), 2);
        assert!(matches!(
            SendLeases::new(NonZeroUsize::new(usize::MAX).unwrap()),
            Err(SendLeaseError::StorageAllocationFailed)
        ));
    }
}
