use crate::synth::{
    AssetAssignment, CapabilityError, CapabilitySection, EffectCapabilityDescriptor,
    EffectCapabilityError, EffectCapabilityId, EffectCapabilityProvider, EffectSlotId,
    ParameterAssignment, ParameterDefault, ParameterId, ParameterKind, ParameterRange,
    ParameterSpec, ParameterUpdate, ParameterValue, PatchInteraction, PostEffectConfig,
};

pub const CHORUS_CAPABILITY_ID: &str = "effect.chorus";
pub const CHORUS_AMOUNT_PARAMETER_ID: &str = "chorus.amount";
pub const CHORUS_DEPTH_PARAMETER_ID: &str = "chorus.depth";
pub const CHORUS_EFFECT_SLOT_ID: u16 = 1;

/// Product-facing capability schema for the pinned Chorus processor.
#[derive(Clone, Debug)]
pub struct ChorusCapability {
    descriptor: EffectCapabilityDescriptor,
}

impl ChorusCapability {
    pub fn new() -> Result<Self, EffectCapabilityError> {
        let amount = continuous_parameter(CHORUS_AMOUNT_PARAMETER_ID, "Amount")?;
        let depth = continuous_parameter(CHORUS_DEPTH_PARAMETER_ID, "Depth")?;
        let descriptor = EffectCapabilityDescriptor::new(
            EffectCapabilityId::new(CHORUS_CAPABILITY_ID).map_err(|_| {
                CapabilityError::InvalidMetadataIdentifier(CHORUS_CAPABILITY_ID.to_owned())
            })?,
            "Chorus",
            "effect.chorus",
            vec![CapabilitySection::new(
                "chorus.controls",
                "Chorus",
                vec![amount, depth],
            )?],
            Vec::new(),
        )?;
        Ok(Self {
            descriptor: descriptor.with_effect_category(crate::synth::EffectCategory::Modulation),
        })
    }

    pub fn default_config(
        &self,
        slot_id: EffectSlotId,
    ) -> Result<PostEffectConfig, EffectCapabilityError> {
        self.create_config(
            slot_id,
            &[
                ParameterAssignment::new(
                    parameter_id(CHORUS_AMOUNT_PARAMETER_ID)?,
                    ParameterValue::continuous(0.5)?,
                ),
                ParameterAssignment::new(
                    parameter_id(CHORUS_DEPTH_PARAMETER_ID)?,
                    ParameterValue::continuous(0.5)?,
                ),
            ],
            &[],
        )
    }
}

impl EffectCapabilityProvider for ChorusCapability {
    fn descriptor(&self) -> EffectCapabilityDescriptor {
        self.descriptor.clone()
    }

    fn create_config(
        &self,
        slot_id: EffectSlotId,
        values: &[ParameterAssignment],
        asset_references: &[AssetAssignment],
    ) -> Result<PostEffectConfig, EffectCapabilityError> {
        self.descriptor
            .create_config(slot_id, values, asset_references)
    }
}

fn continuous_parameter(id: &str, label: &str) -> Result<ParameterSpec, EffectCapabilityError> {
    Ok(ParameterSpec::new_with_patch_interaction(
        parameter_id(id)?,
        label,
        ParameterKind::Continuous,
        ParameterUpdate::Scalar,
        PatchInteraction::ScalarEdit,
        ParameterDefault::Value(ParameterValue::continuous(0.5)?),
        Some(ParameterRange::new(0.0, 1.0)?),
        Vec::new(),
        Some(0.01),
        Some(0.1),
        None,
        "normalized",
        None,
        None,
    )?)
}

fn parameter_id(value: &str) -> Result<ParameterId, EffectCapabilityError> {
    ParameterId::new(value)
        .map_err(|_| CapabilityError::InvalidMetadataIdentifier(value.to_owned()).into())
}
