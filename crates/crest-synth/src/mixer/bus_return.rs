use crate::mixer::bus_id::{BusId, DEFAULT_BUS_RETURNS};
use crate::synth::{EffectCapabilityId, EffectCapabilityRegistry, EffectSlotId, PostEffectConfig};
use core::fmt;

/// Failures that can occur while preparing bounded effect storage.
///
/// Relocated verbatim from the retired `global_effects_processor` module: the
/// four original variants are the preparation vocabulary the return-rack and
/// mix-engine preparers still speak. `InvalidReturnLevel` extends that
/// vocabulary for the return-owned output level introduced with [`BusReturn`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectError {
    /// The sample rate is not finite and strictly positive.
    InvalidSampleRate,
    /// The configured maximum block size is zero.
    InvalidMaxFrames,
    /// The maximum delay is not finite and strictly positive.
    InvalidMaxDelayMilliseconds,
    /// The return level is not finite or is outside 0.0..=1.0.
    InvalidReturnLevel,
    /// The implementation could not reserve all storage before processing.
    StorageAllocationFailed,
}

impl fmt::Display for EffectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidSampleRate => "sample rate must be finite and greater than zero",
            Self::InvalidMaxFrames => "maximum frame count must be greater than zero",
            Self::InvalidMaxDelayMilliseconds => {
                "maximum delay must be finite and greater than zero milliseconds"
            }
            Self::InvalidReturnLevel => "return level must be finite and in 0.0..=1.0",
            Self::StorageAllocationFailed => "global effect storage allocation failed",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for EffectError {}

/// Bounds and edit steps shared by every return-owned output level.
///
/// Copied exactly from the retired `ReverbReturn`/`DelayReturn` global
/// descriptors so the generalization changes no bound: 0.0..=1.0, fine 0.01,
/// coarse 0.1. All returns share this one descriptor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReturnLevelDescriptor {
    minimum: f32,
    maximum: f32,
    default: f32,
    fine_step: f32,
    coarse_step: f32,
}

impl ReturnLevelDescriptor {
    pub const fn minimum(self) -> f32 {
        self.minimum
    }

    pub const fn maximum(self) -> f32 {
        self.maximum
    }

    pub const fn default(self) -> f32 {
        self.default
    }

    pub const fn fine_step(self) -> f32 {
        self.fine_step
    }

    pub const fn coarse_step(self) -> f32 {
        self.coarse_step
    }

    pub fn contains(self, value: f32) -> bool {
        value.is_finite() && (self.minimum..=self.maximum).contains(&value)
    }
}

/// The one shared descriptor for every return-owned level.
///
/// The default of 0.5 is the retired production initial value of both
/// `reverbReturn` and `delayReturn`.
pub const RETURN_LEVEL_DESCRIPTOR: ReturnLevelDescriptor = ReturnLevelDescriptor {
    minimum: 0.0,
    maximum: 1.0,
    default: 0.5,
    fine_step: 0.01,
    coarse_step: 0.1,
};

/// A violation of one bus-return invariant.
#[derive(Clone, Debug, PartialEq)]
pub enum BusReturnError {
    /// The return level is not finite or is outside the shared descriptor bounds.
    InvalidReturnLevel { value: f32 },
    /// The requested registry entry is not installed in the shared registry.
    UnknownRegistryEntry { id: EffectCapabilityId },
    /// The registry rejected building a default configuration for the entry.
    ConfigurationRejected { id: EffectCapabilityId },
    /// A value edit targeted a missing effect instance.
    Unoccupied { id: BusId },
    /// The return name is empty after trimming or contains control characters.
    InvalidName,
    /// The requested bank cannot be represented by nonempty positional storage.
    InvalidReturnCount { count: usize },
    /// The identity is not installed in this bank.
    UnknownReturn { id: BusId },
    /// A restored chain repeats an instance identity.
    DuplicateEffectSlot { slot_id: EffectSlotId },
    /// The bank could not reserve its control-side storage.
    StorageAllocationFailed,
}

impl fmt::Display for BusReturnError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidReturnLevel { value } => write!(
                formatter,
                "return level must be finite and in {}..={}, got {value}",
                RETURN_LEVEL_DESCRIPTOR.minimum(),
                RETURN_LEVEL_DESCRIPTOR.maximum()
            ),
            Self::UnknownRegistryEntry { id } => {
                write!(formatter, "registry entry {id} is not installed")
            }
            Self::ConfigurationRejected { id } => {
                write!(formatter, "registry entry {id} rejected its default config")
            }
            Self::Unoccupied { id } => {
                write!(
                    formatter,
                    "bus return {id} holds no matching effect to edit"
                )
            }
            Self::InvalidName => formatter
                .write_str("return name must be nonempty and contain no control characters"),
            Self::InvalidReturnCount { count } => write!(
                formatter,
                "return count must be in 1..={}, got {count}",
                usize::from(u16::MAX) + 1
            ),
            Self::UnknownReturn { id } => write!(formatter, "bus return {id} is not installed"),
            Self::DuplicateEffectSlot { slot_id } => write!(
                formatter,
                "return effect slot {slot_id} appears more than once"
            ),
            Self::StorageAllocationFailed => {
                formatter.write_str("bus return storage allocation failed")
            }
        }
    }
}

impl std::error::Error for BusReturnError {}

/// One named routing destination holding an ordered effect chain and output level.
///
/// Effects use the same registry as Patch effects. The name and output level
/// belong to the return and survive chain edits. An empty return contributes
/// silence; it never passes accumulated input through.
#[derive(Clone, Debug, PartialEq)]
pub struct BusReturn {
    id: BusId,
    name: String,
    effects: Vec<PostEffectConfig>,
    return_level: f32,
}

impl BusReturn {
    /// Creates a validated return using the legacy single-effect representation.
    pub fn new(
        id: BusId,
        effect: Option<PostEffectConfig>,
        return_level: f32,
    ) -> Result<Self, BusReturnError> {
        Self::unoccupied(id)
            .with_effect(effect)
            .with_return_level(return_level)
    }

    pub fn unoccupied(id: BusId) -> Self {
        Self {
            id,
            name: "INIT".to_owned(),
            effects: Vec::new(),
            return_level: RETURN_LEVEL_DESCRIPTOR.default(),
        }
    }

    pub const fn id(&self) -> BusId {
        self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn set_name(&mut self, name: &str) -> Result<(), BusReturnError> {
        if name.chars().any(char::is_control) || name.trim().is_empty() {
            return Err(BusReturnError::InvalidName);
        }
        self.name = name.trim().to_owned();
        Ok(())
    }

    pub fn with_name(mut self, name: &str) -> Result<Self, BusReturnError> {
        self.set_name(name)?;
        Ok(self)
    }

    /// Compatibility accessor for the first effect in the chain.
    pub fn effect(&self) -> Option<&PostEffectConfig> {
        self.effects.first()
    }

    pub fn effects(&self) -> &[PostEffectConfig] {
        &self.effects
    }

    pub fn effect_at(&self, slot_id: EffectSlotId) -> Option<&PostEffectConfig> {
        self.effects
            .iter()
            .find(|effect| effect.slot_id() == slot_id)
    }

    /// Finds an unused identity without renumbering any surviving instance.
    /// Identities are local to a return, so empty chains retain the legacy seed.
    pub fn next_slot_id(&self) -> Option<EffectSlotId> {
        let seed = Self::slot_id(self.id).value();
        (seed..=u16::MAX)
            .chain(1..seed)
            .filter_map(|value| EffectSlotId::new(value).ok())
            .find(|slot_id| self.effect_at(*slot_id).is_none())
    }

    pub fn is_occupied(&self) -> bool {
        !self.effects.is_empty()
    }

    pub const fn return_level(&self) -> f32 {
        self.return_level
    }

    /// Legacy occupancy edit: replaces the entire chain, preserving metadata.
    pub fn with_effect(mut self, effect: Option<PostEffectConfig>) -> Self {
        self.effects = effect.into_iter().collect();
        self
    }

    /// Restores an ordered chain, rejecting duplicate stable instance identities.
    pub fn with_effects(mut self, effects: Vec<PostEffectConfig>) -> Result<Self, BusReturnError> {
        for (index, effect) in effects.iter().enumerate() {
            if effects[..index]
                .iter()
                .any(|prior| prior.slot_id() == effect.slot_id())
            {
                return Err(BusReturnError::DuplicateEffectSlot {
                    slot_id: effect.slot_id(),
                });
            }
        }
        self.effects = effects;
        Ok(self)
    }

    pub fn with_return_level(mut self, return_level: f32) -> Result<Self, BusReturnError> {
        if !RETURN_LEVEL_DESCRIPTOR.contains(return_level) {
            return Err(BusReturnError::InvalidReturnLevel {
                value: return_level,
            });
        }
        self.return_level = return_level;
        Ok(self)
    }

    /// Legacy first-effect identity. Instance identities are scoped to each bus.
    /// The final representable bus wraps to one, avoiding a zero slot identity.
    pub const fn slot_id(id: BusId) -> EffectSlotId {
        let value = (id.value() as u32 % u16::MAX as u32 + 1) as u16;
        match EffectSlotId::new(value) {
            Ok(slot) => slot,
            Err(_) => panic!("bus-derived slot ids are always non-zero"),
        }
    }
}

/// A configured bank of positional returns, with sixteen empty sends by default.
/// Bank size is independent of the startup default and validated against only
/// the identity representation and available control-side storage.
#[derive(Clone, Debug, PartialEq)]
pub struct BusReturnBank {
    returns: Vec<BusReturn>,
}

impl BusReturnBank {
    pub fn with_count(count: usize) -> Result<Self, BusReturnError> {
        if count == 0 || count > usize::from(u16::MAX) + 1 {
            return Err(BusReturnError::InvalidReturnCount { count });
        }
        let mut returns = Vec::new();
        returns
            .try_reserve_exact(count)
            .map_err(|_| BusReturnError::StorageAllocationFailed)?;
        for index in 0..count {
            let id = BusId::new(index as u16).expect("validated bank count fits bus identities");
            returns.push(BusReturn::unoccupied(id));
        }
        Ok(Self { returns })
    }

    pub fn returns(&self) -> &[BusReturn] {
        &self.returns
    }

    pub fn len(&self) -> usize {
        self.returns.len()
    }

    pub fn is_empty(&self) -> bool {
        self.returns.is_empty()
    }

    pub fn contains(&self, id: BusId) -> bool {
        self.get(id).is_some()
    }

    pub fn get(&self, id: BusId) -> Option<&BusReturn> {
        self.returns.get(id.index())
    }

    /// Indexes an identity already checked against this bank.
    pub fn bus_return(&self, id: BusId) -> &BusReturn {
        &self.returns[id.index()]
    }

    fn get_mut(&mut self, id: BusId) -> Result<&mut BusReturn, BusReturnError> {
        self.returns
            .get_mut(id.index())
            .ok_or(BusReturnError::UnknownReturn { id })
    }

    pub fn set_name(&mut self, id: BusId, name: &str) -> Result<(), BusReturnError> {
        self.get_mut(id)?.set_name(name)
    }

    /// Restores a validated return at its stable bank position.
    pub fn replace_return(&mut self, bus_return: BusReturn) -> Result<(), BusReturnError> {
        *self.get_mut(bus_return.id())? = bus_return.clone();
        Ok(())
    }

    /// Legacy occupancy operation: replaces or clears the entire return chain.
    pub fn set_return_occupancy(
        &mut self,
        registry: &EffectCapabilityRegistry,
        id: BusId,
        entry: Option<&EffectCapabilityId>,
    ) -> Result<(), BusReturnError> {
        self.get_mut(id)?;
        let effect = Self::default_effect(registry, BusReturn::slot_id(id), entry)?;
        self.get_mut(id)?.effects = effect.into_iter().collect();
        Ok(())
    }

    /// Replaces an existing instance in place, appends a new identity, or clears
    /// only the requested identity. Surviving identities and order never change.
    pub fn set_effect_slot(
        &mut self,
        registry: &EffectCapabilityRegistry,
        id: BusId,
        slot_id: EffectSlotId,
        entry: Option<&EffectCapabilityId>,
    ) -> Result<(), BusReturnError> {
        self.get_mut(id)?;
        let effect = Self::default_effect(registry, slot_id, entry)?;
        let current = self.get_mut(id)?;
        let index = current
            .effects
            .iter()
            .position(|effect| effect.slot_id() == slot_id);
        match (index, effect) {
            (Some(index), Some(effect)) => current.effects[index] = effect,
            (Some(index), None) => {
                current.effects.remove(index);
            }
            (None, Some(effect)) => current.effects.push(effect),
            (None, None) => {}
        }
        Ok(())
    }

    fn default_effect(
        registry: &EffectCapabilityRegistry,
        slot_id: EffectSlotId,
        entry: Option<&EffectCapabilityId>,
    ) -> Result<Option<PostEffectConfig>, BusReturnError> {
        entry
            .map(|id| {
                let descriptor = registry
                    .descriptor(id)
                    .ok_or_else(|| BusReturnError::UnknownRegistryEntry { id: id.clone() })?;
                descriptor
                    .default_config(slot_id)
                    .map_err(|_| BusReturnError::ConfigurationRejected { id: id.clone() })
            })
            .transpose()
    }

    pub fn set_return_level(&mut self, id: BusId, return_level: f32) -> Result<(), BusReturnError> {
        if !RETURN_LEVEL_DESCRIPTOR.contains(return_level) {
            return Err(BusReturnError::InvalidReturnLevel {
                value: return_level,
            });
        }
        self.get_mut(id)?.return_level = return_level;
        Ok(())
    }

    /// Updates one chain instance without changing its slot or capability.
    pub fn replace_occupant_values(
        &mut self,
        id: BusId,
        config: PostEffectConfig,
    ) -> Result<(), BusReturnError> {
        let current = self.get_mut(id)?;
        let existing = current
            .effects
            .iter_mut()
            .find(|effect| effect.slot_id() == config.slot_id())
            .ok_or(BusReturnError::Unoccupied { id })?;
        if existing.capability_id() != config.capability_id() {
            return Err(BusReturnError::ConfigurationRejected {
                id: config.capability_id().clone(),
            });
        }
        *existing = config;
        Ok(())
    }
}

impl Default for BusReturnBank {
    fn default() -> Self {
        Self::with_count(DEFAULT_BUS_RETURNS).expect("default return bank storage is available")
    }
}

#[cfg(test)]
mod tests {
    use super::{BusReturn, BusReturnBank, BusReturnError, EffectError, RETURN_LEVEL_DESCRIPTOR};
    use crate::adapter::production_effects::production_effect_registry;
    use crate::mixer::bus_id::BusId;
    use crate::synth::EffectCapabilityId;

    #[test]
    fn preparation_errors_have_actionable_messages() {
        assert_eq!(
            EffectError::InvalidSampleRate.to_string(),
            "sample rate must be finite and greater than zero"
        );
        assert_eq!(
            EffectError::InvalidMaxFrames.to_string(),
            "maximum frame count must be greater than zero"
        );
        assert_eq!(
            EffectError::InvalidMaxDelayMilliseconds.to_string(),
            "maximum delay must be finite and greater than zero milliseconds"
        );
        assert_eq!(
            EffectError::InvalidReturnLevel.to_string(),
            "return level must be finite and in 0.0..=1.0"
        );
        assert_eq!(
            EffectError::StorageAllocationFailed.to_string(),
            "global effect storage allocation failed"
        );
    }

    #[test]
    fn return_level_descriptor_matches_the_retired_return_bounds() {
        assert_eq!(RETURN_LEVEL_DESCRIPTOR.minimum(), 0.0);
        assert_eq!(RETURN_LEVEL_DESCRIPTOR.maximum(), 1.0);
        assert_eq!(RETURN_LEVEL_DESCRIPTOR.default(), 0.5);
        assert_eq!(RETURN_LEVEL_DESCRIPTOR.fine_step(), 0.01);
        assert_eq!(RETURN_LEVEL_DESCRIPTOR.coarse_step(), 0.1);
    }

    #[test]
    fn return_level_is_validated_and_never_clamped() {
        let bus = BusId::new(2).unwrap();
        assert!(BusReturn::new(bus, None, 1.0).is_ok());
        assert!(BusReturn::new(bus, None, 0.0).is_ok());
        assert_eq!(
            BusReturn::new(bus, None, 1.1),
            Err(BusReturnError::InvalidReturnLevel { value: 1.1 })
        );
        assert!(matches!(
            BusReturn::new(bus, None, f32::NAN),
            Err(BusReturnError::InvalidReturnLevel { .. })
        ));
        assert!(BusReturn::unoccupied(bus).with_return_level(-0.1).is_err());
    }

    #[test]
    fn legacy_occupancy_can_be_set_replaced_and_cleared_on_each_default_return() {
        let registry = production_effect_registry().unwrap();
        let first = EffectCapabilityId::new("effect.reverb").unwrap();
        let second = EffectCapabilityId::new("effect.delay").unwrap();
        let mut bank = BusReturnBank::default();

        for bus in BusId::ALL {
            assert!(!bank.bus_return(bus).is_occupied());

            bank.set_return_occupancy(&registry, bus, Some(&first))
                .unwrap();
            assert_eq!(
                bank.bus_return(bus).effect().unwrap().capability_id(),
                &first
            );

            bank.set_return_occupancy(&registry, bus, Some(&second))
                .unwrap();
            assert_eq!(
                bank.bus_return(bus).effect().unwrap().capability_id(),
                &second
            );

            bank.set_return_occupancy(&registry, bus, None).unwrap();
            assert!(!bank.bus_return(bus).is_occupied());
            assert_eq!(bank.bus_return(bus).id(), bus);
        }
    }

    #[test]
    fn return_level_survives_every_occupancy_transition() {
        let registry = production_effect_registry().unwrap();
        let entry = EffectCapabilityId::new("effect.chorus").unwrap();
        let bus = BusId::new(5).unwrap();
        let mut bank = BusReturnBank::default();
        bank.set_return_level(bus, 0.75).unwrap();

        bank.set_return_occupancy(&registry, bus, Some(&entry))
            .unwrap();
        assert_eq!(bank.bus_return(bus).return_level(), 0.75);

        bank.set_return_occupancy(&registry, bus, None).unwrap();
        assert_eq!(bank.bus_return(bus).return_level(), 0.75);
    }

    #[test]
    fn unknown_registry_entries_are_rejected_without_substitution() {
        let registry = production_effect_registry().unwrap();
        let unknown = EffectCapabilityId::new("effect.absent").unwrap();
        let mut bank = BusReturnBank::default();

        assert_eq!(
            bank.set_return_occupancy(&registry, BusId::default(), Some(&unknown)),
            Err(BusReturnError::UnknownRegistryEntry { id: unknown })
        );
        assert!(!bank.bus_return(BusId::default()).is_occupied());
    }

    #[test]
    fn default_bank_has_sixteen_init_chains_and_larger_banks_are_supported() {
        let bank = BusReturnBank::default();
        assert_eq!(bank.len(), 16);
        assert!(bank
            .returns()
            .iter()
            .all(|bus| bus.name() == "INIT" && bus.effects().is_empty()));
        let expanded = BusReturnBank::with_count(257).unwrap();
        let beyond_default = BusId::new(256).unwrap();
        assert!(expanded.contains(beyond_default));
        assert_eq!(expanded.get(beyond_default).unwrap().id(), beyond_default);
        assert!(!bank.contains(beyond_default));
        assert_eq!(
            BusReturnBank::with_count(0),
            Err(BusReturnError::InvalidReturnCount { count: 0 })
        );
        let too_many = usize::from(u16::MAX) + 2;
        assert_eq!(
            BusReturnBank::with_count(too_many),
            Err(BusReturnError::InvalidReturnCount { count: too_many })
        );
    }

    #[test]
    fn names_are_trimmed_and_invalid_edits_leave_prior_name_intact() {
        let bus = BusId::default();
        let mut bank = BusReturnBank::default();
        bank.set_name(bus, "  Hall & Echo  ").unwrap();
        assert_eq!(bank.bus_return(bus).name(), "Hall & Echo");
        for invalid in ["", "   ", "Hall\n", "\tHall", "Hall\0Echo"] {
            assert_eq!(
                bank.set_name(bus, invalid),
                Err(BusReturnError::InvalidName)
            );
            assert_eq!(bank.bus_return(bus).name(), "Hall & Echo");
        }
        assert_eq!(
            BusReturn::unoccupied(bus).with_name("Echo").unwrap().name(),
            "Echo"
        );
    }

    #[test]
    fn chain_edits_preserve_order_surviving_identities_name_and_level() {
        let registry = production_effect_registry().unwrap();
        let reverb = EffectCapabilityId::new("effect.reverb").unwrap();
        let delay = EffectCapabilityId::new("effect.delay").unwrap();
        let chorus = EffectCapabilityId::new("effect.chorus").unwrap();
        let bus = BusId::new(2).unwrap();
        let mut bank = BusReturnBank::default();
        bank.set_name(bus, "Echo room").unwrap();
        bank.set_return_level(bus, 0.7).unwrap();
        let first = bank.bus_return(bus).next_slot_id().unwrap();
        bank.set_effect_slot(&registry, bus, first, Some(&reverb))
            .unwrap();
        let second = bank.bus_return(bus).next_slot_id().unwrap();
        bank.set_effect_slot(&registry, bus, second, Some(&delay))
            .unwrap();
        assert_ne!(first, second);
        bank.set_effect_slot(&registry, bus, first, Some(&chorus))
            .unwrap();
        assert_eq!(
            bank.bus_return(bus)
                .effects()
                .iter()
                .map(|effect| effect.slot_id())
                .collect::<Vec<_>>(),
            vec![first, second]
        );
        let delay_config = bank.bus_return(bus).effect_at(second).unwrap().clone();
        bank.replace_occupant_values(bus, delay_config.clone())
            .unwrap();
        assert_eq!(bank.bus_return(bus).effects().len(), 2);
        bank.set_effect_slot(&registry, bus, first, None).unwrap();
        assert_eq!(bank.bus_return(bus).effects(), &[delay_config]);
        assert_eq!(bank.bus_return(bus).effect().unwrap().slot_id(), second);
        assert_eq!(bank.bus_return(bus).name(), "Echo room");
        assert_eq!(bank.bus_return(bus).return_level(), 0.7);
        let next = bank.bus_return(bus).next_slot_id().unwrap();
        assert_ne!(next, second);
    }

    #[test]
    fn restoring_chain_rejects_duplicate_ids_and_replacement_checks_bank_membership() {
        let registry = production_effect_registry().unwrap();
        let entry = EffectCapabilityId::new("effect.delay").unwrap();
        let bus = BusId::default();
        let config = registry
            .descriptor(&entry)
            .unwrap()
            .default_config(BusReturn::slot_id(bus))
            .unwrap();
        assert_eq!(
            BusReturn::unoccupied(bus).with_effects(vec![config.clone(), config.clone()]),
            Err(BusReturnError::DuplicateEffectSlot {
                slot_id: config.slot_id()
            })
        );
        let restored = BusReturn::unoccupied(bus)
            .with_effects(vec![config])
            .unwrap()
            .with_name("Restored")
            .unwrap();
        let mut bank = BusReturnBank::default();
        bank.replace_return(restored.clone()).unwrap();
        assert_eq!(bank.bus_return(bus), &restored);
        let unknown = BusId::new(256).unwrap();
        assert_eq!(
            bank.set_return_occupancy(&registry, unknown, Some(&entry)),
            Err(BusReturnError::UnknownReturn { id: unknown })
        );
        assert_eq!(
            bank.replace_return(BusReturn::unoccupied(unknown)),
            Err(BusReturnError::UnknownReturn { id: unknown })
        );
        assert_eq!(bank.bus_return(bus), &restored);
    }

    #[test]
    fn compatibility_occupancy_replaces_entire_chain_preserving_metadata() {
        let registry = production_effect_registry().unwrap();
        let entry = EffectCapabilityId::new("effect.delay").unwrap();
        let bus = BusId::default();
        let mut bank = BusReturnBank::default();
        bank.set_name(bus, "Echo").unwrap();
        for _ in 0..2 {
            let slot = bank.bus_return(bus).next_slot_id().unwrap();
            bank.set_effect_slot(&registry, bus, slot, Some(&entry))
                .unwrap();
        }
        assert_eq!(bank.bus_return(bus).effects().len(), 2);
        bank.set_return_occupancy(&registry, bus, Some(&entry))
            .unwrap();
        assert_eq!(bank.bus_return(bus).effects().len(), 1);
        bank.set_return_occupancy(&registry, bus, None).unwrap();
        assert!(bank.bus_return(bus).effects().is_empty());
        assert_eq!(bank.bus_return(bus).name(), "Echo");
    }

    #[test]
    fn derived_slot_identity_is_stable_and_non_zero() {
        for bus in BusId::ALL {
            assert_eq!(BusReturn::slot_id(bus).value(), bus.value() + 1);
        }
        assert_eq!(BusReturn::slot_id(BusId::new(u16::MAX).unwrap()).value(), 1);
    }
}
