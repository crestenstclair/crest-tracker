use crate::synth::{
    AssetAssignment, EffectCapabilityDescriptor, EffectCapabilityError, EffectSlotId,
    ParameterAssignment, PostEffectConfig,
};

/// Control-side metadata/configuration port for one installed effect.
///
/// A provider declares identity, visible parameters, bounds, units, and
/// preparation requirements. It never declares a role, send-suitability, or
/// return-only marker: whether an entry occupies a Patch effect slot or a bus
/// return is the caller's decision.
pub trait EffectCapabilityProvider {
    fn descriptor(&self) -> EffectCapabilityDescriptor;

    fn create_config(
        &self,
        slot_id: EffectSlotId,
        values: &[ParameterAssignment],
        asset_references: &[AssetAssignment],
    ) -> Result<PostEffectConfig, EffectCapabilityError>;
}
