use crate::synth::{
    AssetAssignment, CapabilityError, CapabilitySection, EffectCapabilityDescriptor,
    EffectCapabilityError, EffectCapabilityId, EffectCapabilityProvider, EffectSlotId,
    ParameterAssignment, ParameterDefault, ParameterId, ParameterKind, ParameterRange,
    ParameterSpec, ParameterUpdate, ParameterValue, PatchInteraction, PostEffectConfig,
};

pub const REVERB_CAPABILITY_ID: &str = "effect.reverb";
pub const REVERB_ROOM_SIZE_PARAMETER_ID: &str = "reverb.room-size";
pub const REVERB_DAMPING_PARAMETER_ID: &str = "reverb.damping";

/// Bounds, steps, and defaults are carried forward value-for-value from the
/// retired `GlobalParameter::ReverbRoomSize` and `GlobalParameter::ReverbDamping`
/// surface descriptors (0.0..=1.0, fine 0.01, coarse 0.1) and the production
/// `ApplicationConfig` default global parameters (room size 0.5, damping 0.5).
/// A changed value here is an audible behavior change.
pub const REVERB_ROOM_SIZE_DEFAULT: f64 = 0.5;
pub const REVERB_DAMPING_DEFAULT: f64 = 0.5;

/// Product-facing capability schema for the shared algorithmic stereo Reverb.
///
/// Reverb is an ordinary registry entry, a peer of Chorus. It declares no
/// return level — return level is a property of the destination `BusReturn`,
/// not of the effect — and no role, send-suitability, or return-only marker.
#[derive(Clone, Debug)]
pub struct ReverbCapability {
    descriptor: EffectCapabilityDescriptor,
}

impl ReverbCapability {
    pub fn new() -> Result<Self, EffectCapabilityError> {
        let room_size = continuous_parameter(
            REVERB_ROOM_SIZE_PARAMETER_ID,
            "Room Size",
            REVERB_ROOM_SIZE_DEFAULT,
        )?;
        let damping = continuous_parameter(
            REVERB_DAMPING_PARAMETER_ID,
            "Damping",
            REVERB_DAMPING_DEFAULT,
        )?;
        let descriptor = EffectCapabilityDescriptor::new(
            EffectCapabilityId::new(REVERB_CAPABILITY_ID).map_err(|_| {
                CapabilityError::InvalidMetadataIdentifier(REVERB_CAPABILITY_ID.to_owned())
            })?,
            "Reverb",
            "effect.reverb",
            vec![CapabilitySection::new(
                "reverb.controls",
                "Reverb",
                vec![room_size, damping],
            )?],
            Vec::new(),
        )?;
        Ok(Self {
            descriptor: descriptor.with_effect_category(crate::synth::EffectCategory::ReverbAndIr),
        })
    }

    pub fn default_config(
        &self,
        slot_id: EffectSlotId,
    ) -> Result<PostEffectConfig, EffectCapabilityError> {
        self.descriptor.default_config(slot_id)
    }
}

impl EffectCapabilityProvider for ReverbCapability {
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

fn continuous_parameter(
    id: &str,
    label: &str,
    default: f64,
) -> Result<ParameterSpec, EffectCapabilityError> {
    Ok(ParameterSpec::new_with_patch_interaction(
        parameter_id(id)?,
        label,
        ParameterKind::Continuous,
        ParameterUpdate::Scalar,
        PatchInteraction::ScalarEdit,
        ParameterDefault::Value(ParameterValue::continuous(default)?),
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

#[cfg(test)]
mod tests {
    use super::{
        ReverbCapability, REVERB_CAPABILITY_ID, REVERB_DAMPING_PARAMETER_ID,
        REVERB_ROOM_SIZE_PARAMETER_ID,
    };
    use crate::synth::{
        EffectCapabilityProvider, EffectSlotId, ParameterDefault, ParameterKind, ParameterUpdate,
        ParameterValue,
    };

    #[test]
    fn declares_exactly_room_size_and_damping_with_retired_global_values() {
        let descriptor = ReverbCapability::new().unwrap().descriptor();

        assert_eq!(descriptor.id().as_str(), REVERB_CAPABILITY_ID);
        assert_eq!(descriptor.label(), "Reverb");
        let parameters: Vec<_> = descriptor.parameters().collect();
        assert_eq!(parameters.len(), 2);
        assert_eq!(
            parameters[0].id().as_str(),
            REVERB_ROOM_SIZE_PARAMETER_ID,
            "room size is the first descriptor scalar"
        );
        assert_eq!(
            parameters[1].id().as_str(),
            REVERB_DAMPING_PARAMETER_ID,
            "damping is the second descriptor scalar"
        );
        for (parameter, default) in parameters.iter().zip([0.5, 0.5]) {
            // Values transcribed from the retired GlobalParameter::ReverbRoomSize
            // and ::ReverbDamping descriptors and the production
            // ApplicationConfig::default global parameters.
            assert_eq!(parameter.kind(), ParameterKind::Continuous);
            assert_eq!(parameter.update(), ParameterUpdate::Scalar);
            let range = parameter.range().expect("reverb scalars are bounded");
            assert_eq!(range.minimum(), 0.0);
            assert_eq!(range.maximum(), 1.0);
            assert_eq!(parameter.fine_step(), Some(0.01));
            assert_eq!(parameter.coarse_step(), Some(0.1));
            assert_eq!(
                parameter.default_value(),
                &ParameterDefault::Value(ParameterValue::continuous(default).unwrap())
            );
        }
    }

    #[test]
    fn declares_no_return_level_and_no_role_marker() {
        let descriptor = ReverbCapability::new().unwrap().descriptor();

        assert!(
            descriptor
                .parameters()
                .all(|parameter| !parameter.id().as_str().contains("return")),
            "return level belongs to the BusReturn, not the effect"
        );
        assert_eq!(descriptor.scalar_parameter_count(), 2);
        assert!(descriptor.asset_requirements().is_empty());
    }

    #[test]
    fn default_config_is_valid_without_any_role_hint() {
        let capability = ReverbCapability::new().unwrap();
        let slot_id = EffectSlotId::new(1).unwrap();

        let config = capability.default_config(slot_id).unwrap();

        assert_eq!(config.slot_id(), slot_id);
        assert_eq!(config.capability_id().as_str(), REVERB_CAPABILITY_ID);
        assert_eq!(config.values().len(), 2);
    }
}
