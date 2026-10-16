use crate::synth::{
    AssetAssignment, CapabilityError, CapabilitySection, EffectCapabilityDescriptor,
    EffectCapabilityError, EffectCapabilityId, EffectCapabilityProvider, EffectSlotId,
    ParameterAssignment, ParameterDefault, ParameterId, ParameterKind, ParameterRange,
    ParameterSpec, ParameterUpdate, ParameterValue, PatchInteraction, PostEffectConfig,
};

pub const DELAY_CAPABILITY_ID: &str = "effect.delay";
pub const DELAY_MILLISECONDS_PARAMETER_ID: &str = "delay.milliseconds";
pub const DELAY_FEEDBACK_PARAMETER_ID: &str = "delay.feedback";

/// Bounds, steps, and defaults are carried forward value-for-value from the
/// retired `GlobalParameter::DelayMilliseconds` (1.0..=2000.0, fine 1.0,
/// coarse 100.0) and `GlobalParameter::DelayFeedback` (0.0..=1.0, fine 0.01,
/// coarse 0.1) surface descriptors and the production `ApplicationConfig`
/// default global parameters (250.0 ms, feedback 0.5). A changed value here
/// is an audible behavior change.
pub const DELAY_MILLISECONDS_MINIMUM: f64 = 1.0;
pub const DELAY_MILLISECONDS_MAXIMUM: f64 = 2000.0;
pub const DELAY_MILLISECONDS_DEFAULT: f64 = 250.0;
pub const DELAY_FEEDBACK_DEFAULT: f64 = 0.5;

/// The maximum-delay preparation requirement declared by this entry.
///
/// Preparation sizes every delay line to satisfy the complete declared
/// milliseconds range, so an in-range delay time is never silently truncated.
/// A configuration this requirement cannot be satisfied for is refused with a
/// typed preparation error.
pub const DELAY_MAX_DELAY_MILLISECONDS: f32 = DELAY_MILLISECONDS_MAXIMUM as f32;

/// Product-facing capability schema for the shared stereo feedback Delay.
///
/// Delay is an ordinary registry entry, a peer of Chorus. It declares no
/// return level — return level is a property of the destination `BusReturn`,
/// not of the effect — and no role, send-suitability, or return-only marker.
#[derive(Clone, Debug)]
pub struct DelayCapability {
    descriptor: EffectCapabilityDescriptor,
}

impl DelayCapability {
    pub fn new() -> Result<Self, EffectCapabilityError> {
        let milliseconds = ParameterSpec::new_with_patch_interaction(
            parameter_id(DELAY_MILLISECONDS_PARAMETER_ID)?,
            "Milliseconds",
            ParameterKind::Continuous,
            ParameterUpdate::Scalar,
            PatchInteraction::ScalarEdit,
            ParameterDefault::Value(ParameterValue::continuous(DELAY_MILLISECONDS_DEFAULT)?),
            Some(ParameterRange::new(
                DELAY_MILLISECONDS_MINIMUM,
                DELAY_MILLISECONDS_MAXIMUM,
            )?),
            Vec::new(),
            Some(1.0),
            Some(100.0),
            Some("ms".to_owned()),
            "milliseconds",
            None,
            None,
        )
        .map_err(EffectCapabilityError::from)?;
        let feedback = ParameterSpec::new_with_patch_interaction(
            parameter_id(DELAY_FEEDBACK_PARAMETER_ID)?,
            "Feedback",
            ParameterKind::Continuous,
            ParameterUpdate::Scalar,
            PatchInteraction::ScalarEdit,
            ParameterDefault::Value(ParameterValue::continuous(DELAY_FEEDBACK_DEFAULT)?),
            Some(ParameterRange::new(0.0, 1.0)?),
            Vec::new(),
            Some(0.01),
            Some(0.1),
            None,
            "normalized",
            None,
            None,
        )
        .map_err(EffectCapabilityError::from)?;
        let descriptor = EffectCapabilityDescriptor::new(
            EffectCapabilityId::new(DELAY_CAPABILITY_ID).map_err(|_| {
                CapabilityError::InvalidMetadataIdentifier(DELAY_CAPABILITY_ID.to_owned())
            })?,
            "Delay",
            "effect.delay",
            vec![CapabilitySection::new(
                "delay.controls",
                "Delay",
                vec![milliseconds, feedback],
            )?],
            Vec::new(),
        )?;
        Ok(Self {
            descriptor: descriptor.with_effect_category(crate::synth::EffectCategory::DelayAndEcho),
        })
    }

    pub fn default_config(
        &self,
        slot_id: EffectSlotId,
    ) -> Result<PostEffectConfig, EffectCapabilityError> {
        self.descriptor.default_config(slot_id)
    }
}

impl EffectCapabilityProvider for DelayCapability {
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

fn parameter_id(value: &str) -> Result<ParameterId, EffectCapabilityError> {
    ParameterId::new(value)
        .map_err(|_| CapabilityError::InvalidMetadataIdentifier(value.to_owned()).into())
}

#[cfg(test)]
mod tests {
    use super::{
        DelayCapability, DELAY_CAPABILITY_ID, DELAY_FEEDBACK_PARAMETER_ID,
        DELAY_MAX_DELAY_MILLISECONDS, DELAY_MILLISECONDS_PARAMETER_ID,
    };
    use crate::synth::{
        EffectCapabilityProvider, EffectSlotId, ParameterDefault, ParameterKind, ParameterUpdate,
        ParameterValue,
    };

    #[test]
    fn declares_exactly_milliseconds_and_feedback_with_retired_global_values() {
        let descriptor = DelayCapability::new().unwrap().descriptor();

        assert_eq!(descriptor.id().as_str(), DELAY_CAPABILITY_ID);
        assert_eq!(descriptor.label(), "Delay");
        let parameters: Vec<_> = descriptor.parameters().collect();
        assert_eq!(parameters.len(), 2);

        // Values transcribed from the retired GlobalParameter::DelayMilliseconds
        // descriptor and the production ApplicationConfig::default global
        // parameters.
        let milliseconds = parameters[0];
        assert_eq!(milliseconds.id().as_str(), DELAY_MILLISECONDS_PARAMETER_ID);
        assert_eq!(milliseconds.kind(), ParameterKind::Continuous);
        assert_eq!(milliseconds.update(), ParameterUpdate::Scalar);
        let range = milliseconds.range().expect("delay time is bounded");
        assert_eq!(range.minimum(), 1.0);
        assert_eq!(range.maximum(), 2000.0);
        assert_eq!(milliseconds.fine_step(), Some(1.0));
        assert_eq!(milliseconds.coarse_step(), Some(100.0));
        assert_eq!(milliseconds.unit(), Some("ms"));
        assert_eq!(
            milliseconds.default_value(),
            &ParameterDefault::Value(ParameterValue::continuous(250.0).unwrap())
        );

        // Values transcribed from the retired GlobalParameter::DelayFeedback
        // descriptor and the production ApplicationConfig::default global
        // parameters.
        let feedback = parameters[1];
        assert_eq!(feedback.id().as_str(), DELAY_FEEDBACK_PARAMETER_ID);
        assert_eq!(feedback.kind(), ParameterKind::Continuous);
        assert_eq!(feedback.update(), ParameterUpdate::Scalar);
        let range = feedback.range().expect("delay feedback is bounded");
        assert_eq!(range.minimum(), 0.0);
        assert_eq!(range.maximum(), 1.0);
        assert_eq!(feedback.fine_step(), Some(0.01));
        assert_eq!(feedback.coarse_step(), Some(0.1));
        assert_eq!(
            feedback.default_value(),
            &ParameterDefault::Value(ParameterValue::continuous(0.5).unwrap())
        );
    }

    #[test]
    fn declares_no_return_level_and_no_role_marker() {
        let descriptor = DelayCapability::new().unwrap().descriptor();

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
    fn maximum_delay_requirement_covers_the_complete_declared_range() {
        let descriptor = DelayCapability::new().unwrap().descriptor();
        let milliseconds = descriptor
            .parameters()
            .next()
            .expect("delay declares its time scalar first");

        assert_eq!(
            f64::from(DELAY_MAX_DELAY_MILLISECONDS),
            milliseconds.range().unwrap().maximum(),
            "the preparation requirement equals the declared maximum delay time"
        );
    }

    #[test]
    fn default_config_is_valid_without_any_role_hint() {
        let capability = DelayCapability::new().unwrap();
        let slot_id = EffectSlotId::new(1).unwrap();

        let config = capability.default_config(slot_id).unwrap();

        assert_eq!(config.slot_id(), slot_id);
        assert_eq!(config.capability_id().as_str(), DELAY_CAPABILITY_ID);
        assert_eq!(config.values().len(), 2);
    }
}
