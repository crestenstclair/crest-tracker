use crate::synth::instrument_capability::{
    AssetAssignment, CapabilityDescriptor, CapabilityError, InstrumentConfig, ParameterAssignment,
};

/// Control-side port for capability metadata and generic config construction.
///
/// The boundary intentionally contains no rendering, device, UI, file-loading,
/// audio-buffer, or SoundFont-specific operation.
pub trait InstrumentCapabilityProvider {
    fn descriptor(&self) -> CapabilityDescriptor;

    fn create_config(
        &self,
        values: &[ParameterAssignment],
        asset_references: &[AssetAssignment],
    ) -> Result<InstrumentConfig, CapabilityError>;
}

#[cfg(test)]
mod tests {
    use super::InstrumentCapabilityProvider;
    use crate::synth::instrument_capability::{
        AssetAssignment, CapabilityDescriptor, CapabilityError, InstrumentConfig,
        ParameterAssignment,
    };

    type CreateConfigFn<Provider> = fn(
        &Provider,
        &[ParameterAssignment],
        &[AssetAssignment],
    ) -> Result<InstrumentConfig, CapabilityError>;

    fn generic_contract<Provider: InstrumentCapabilityProvider>() {
        let _: fn(&Provider) -> CapabilityDescriptor = Provider::descriptor;
        let _: CreateConfigFn<Provider> = Provider::create_config;
    }

    #[test]
    fn provider_contract_is_generic_control_side_metadata_only() {
        let _ = generic_contract::<NeverProvider>;
    }

    struct NeverProvider;

    impl InstrumentCapabilityProvider for NeverProvider {
        fn descriptor(&self) -> CapabilityDescriptor {
            unreachable!()
        }

        fn create_config(
            &self,
            _values: &[ParameterAssignment],
            _asset_references: &[AssetAssignment],
        ) -> Result<InstrumentConfig, CapabilityError> {
            unreachable!()
        }
    }
}
