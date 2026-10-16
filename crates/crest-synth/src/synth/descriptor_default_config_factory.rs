use crate::synth::capability_id::CapabilityId;
use crate::synth::instrument_capability::{
    AssetReference, CapabilityError, CapabilityRegistry, InstrumentConfig, ParameterAssignment,
    ParameterDefault, ParameterKind, ParameterUpdate, ParameterValue, PatchInteraction,
};
use crate::synth::instrument_capability_provider::InstrumentCapabilityProvider;
use crate::synth::ParameterId;

/// Constructs provider-validated instrument configs from descriptor defaults.
///
/// This control-side service owns no Patch, prepared engine, graph, renderer, or
/// inactive-engine cache. It only translates the selected immutable descriptor
/// into the generic provider inputs already declared by that descriptor.
pub struct DescriptorDefaultConfigFactory {
    registry: CapabilityRegistry,
    providers: Vec<Box<dyn InstrumentCapabilityProvider>>,
}

impl DescriptorDefaultConfigFactory {
    pub const fn new(
        registry: CapabilityRegistry,
        providers: Vec<Box<dyn InstrumentCapabilityProvider>>,
    ) -> Self {
        Self {
            registry,
            providers,
        }
    }

    pub const fn registry(&self) -> &CapabilityRegistry {
        &self.registry
    }

    pub fn use_asset_descriptors(
        &mut self,
        registry: &CapabilityRegistry,
    ) -> Result<(), CapabilityError> {
        let mut updated = CapabilityRegistry::new(self.registry.descriptors().to_vec())?;
        for descriptor in registry.asset_descriptors() {
            updated = updated.with_asset_descriptor(descriptor.clone())?;
        }
        self.registry = updated;
        Ok(())
    }

    /// Creates one exact descriptor-default config without capability-specific
    /// branching or substitution.
    pub fn create(
        &self,
        capability_id: &CapabilityId,
    ) -> Result<InstrumentConfig, CapabilityError> {
        let descriptor = self
            .registry
            .descriptor(capability_id)
            .ok_or_else(|| CapabilityError::UnknownCapability(capability_id.clone()))?;

        let provider = self.provider_for(capability_id, descriptor)?;

        let mut values = Vec::with_capacity(descriptor.parameters().count());
        let assets = descriptor.default_assets();
        for parameter in descriptor.parameters() {
            match parameter.default_value() {
                ParameterDefault::Value(value) => values.push(ParameterAssignment::new(
                    parameter.id().clone(),
                    value.clone(),
                )),
                ParameterDefault::Asset(_) => {}
            }
        }

        let config = provider.create_config(&values, &assets)?;
        descriptor.create_config(config.values(), config.asset_references())?;
        self.registry.validate_config(&config)?;
        if config.capability_id() != capability_id {
            return Err(CapabilityError::ProviderRegistryMismatch(
                capability_id.clone(),
            ));
        }
        Ok(config)
    }

    /// Replaces exactly one descriptor-declared PATCH structural Choice and
    /// returns the provider- and registry-validated canonical candidate.
    pub fn replace_structural_choice(
        &self,
        source: &InstrumentConfig,
        parameter_id: &ParameterId,
        choice_id: &str,
    ) -> Result<InstrumentConfig, CapabilityError> {
        self.registry.validate_config(source)?;
        let descriptor = self
            .registry
            .descriptor_for_config(source)
            .ok_or_else(|| CapabilityError::UnknownCapability(source.capability_id().clone()))?;
        let spec = descriptor
            .parameter(parameter_id)
            .ok_or_else(|| CapabilityError::UndeclaredParameter(parameter_id.clone()))?;
        if spec.kind() != ParameterKind::Choice
            || spec.update() != ParameterUpdate::Structural
            || spec.patch_interaction() != PatchInteraction::StructuralChoice
        {
            return Err(CapabilityError::StructuralParameter(parameter_id.clone()));
        }
        if !spec.choices().iter().any(|choice| choice.id() == choice_id) {
            return Err(CapabilityError::UnknownChoice(parameter_id.clone()));
        }

        let mut values = source.values().to_vec();
        let assignment = values
            .iter_mut()
            .find(|assignment| assignment.parameter_id() == parameter_id)
            .ok_or_else(|| CapabilityError::MissingParameter(parameter_id.clone()))?;
        *assignment = ParameterAssignment::new(
            parameter_id.clone(),
            ParameterValue::Choice(choice_id.to_owned()),
        );
        let installed = self
            .registry
            .descriptor(source.capability_id())
            .ok_or_else(|| CapabilityError::UnknownCapability(source.capability_id().clone()))?;
        let provider = self.provider_for(source.capability_id(), installed)?;
        let candidate = if installed == descriptor {
            provider.create_config(&values, source.asset_references())?
        } else {
            descriptor.create_config(&values, source.asset_references())?
        };
        self.registry.validate_config(&candidate)?;
        if candidate.capability_id() != source.capability_id()
            || candidate.asset_references() != source.asset_references()
            || candidate.values().len() != source.values().len()
            || candidate
                .values()
                .iter()
                .zip(source.values())
                .any(|(candidate, original)| {
                    candidate.parameter_id() != original.parameter_id()
                        || (candidate.parameter_id() != parameter_id && candidate != original)
                })
        {
            return Err(CapabilityError::ProviderRegistryMismatch(
                source.capability_id().clone(),
            ));
        }
        Ok(candidate)
    }

    /// Replaces exactly one descriptor-declared asset reference while keeping
    /// every scalar assignment and other asset byte-for-byte canonical.
    pub fn replace_asset(
        &self,
        source: &InstrumentConfig,
        parameter_id: &ParameterId,
        reference: AssetReference,
    ) -> Result<InstrumentConfig, CapabilityError> {
        let installed = self
            .registry
            .descriptor(source.capability_id())
            .ok_or_else(|| CapabilityError::UnknownCapability(source.capability_id().clone()))?;
        self.provider_for(source.capability_id(), installed)?;
        self.registry.replace_asset(source, parameter_id, reference)
    }

    fn provider_for<'a>(
        &'a self,
        capability_id: &CapabilityId,
        descriptor: &crate::synth::CapabilityDescriptor,
    ) -> Result<&'a dyn InstrumentCapabilityProvider, CapabilityError> {
        let mut matching = self
            .providers
            .iter()
            .filter(|provider| provider.descriptor().id() == capability_id);
        let provider = matching
            .next()
            .ok_or_else(|| CapabilityError::ProviderRegistryMismatch(capability_id.clone()))?;
        if matching.next().is_some() || provider.descriptor() != *descriptor {
            return Err(CapabilityError::ProviderRegistryMismatch(
                capability_id.clone(),
            ));
        }
        Ok(provider.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::DescriptorDefaultConfigFactory;
    use crate::adapter::braids_capability::{
        BraidsCapability, BRAIDS_CAPABILITY_ID, BRAIDS_COLOR_PARAMETER_ID, BRAIDS_MODELS,
        BRAIDS_MODEL_PARAMETER_ID, BRAIDS_TIMBRE_PARAMETER_ID,
    };
    use crate::adapter::hidef_soundfont_capability::{
        HIDEF_CAPABILITY_ID, HIDEF_SOUNDFONT_PATH, SOUNDFONT_FILE_PARAMETER_ID,
        SOUNDFONT_PRESET_PARAMETER_ID,
    };
    use crate::synth::instrument_capability::{
        AssetAssignment, CapabilityDescriptor, CapabilityError, CapabilityRegistry,
        InstrumentConfig, ParameterAssignment, ParameterValue,
    };
    use crate::synth::{CapabilityId, InstrumentCapabilityProvider, ParameterId};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn capability_id(value: &str) -> CapabilityId {
        CapabilityId::new(value).unwrap()
    }

    fn parameter_id(value: &str) -> ParameterId {
        ParameterId::new(value).unwrap()
    }

    fn production_factory() -> DescriptorDefaultConfigFactory {
        let providers: Vec<Box<dyn InstrumentCapabilityProvider>> = vec![
            Box::new(
                crate::adapter::production_instruments::production_soundfont_capability().unwrap(),
            ),
            Box::new(BraidsCapability::new().unwrap()),
        ];
        let registry = CapabilityRegistry::new(
            providers
                .iter()
                .map(|provider| provider.descriptor())
                .collect(),
        )
        .unwrap();
        DescriptorDefaultConfigFactory::new(registry, providers)
    }

    #[test]
    fn descriptor_default_config_factory_creates_both_exact_production_defaults() {
        let factory = production_factory();

        let soundfont = factory.create(&capability_id(HIDEF_CAPABILITY_ID)).unwrap();
        assert_eq!(
            soundfont.value(&parameter_id(SOUNDFONT_PRESET_PARAMETER_ID)),
            Some(&ParameterValue::Choice(
                crate::adapter::production_instruments::production_soundfont_asset()
                    .unwrap()
                    .catalog()
                    .default_entry()
                    .choice_id()
            ))
        );
        assert_eq!(
            soundfont
                .asset_reference(&parameter_id(SOUNDFONT_FILE_PARAMETER_ID))
                .map(|reference| reference.locator()),
            Some(HIDEF_SOUNDFONT_PATH)
        );

        let braids = factory
            .create(&capability_id(BRAIDS_CAPABILITY_ID))
            .unwrap();
        assert_eq!(
            braids.value(&parameter_id(BRAIDS_MODEL_PARAMETER_ID)),
            Some(&ParameterValue::Choice(BRAIDS_MODELS[0].id.to_owned()))
        );
        assert_eq!(
            braids.value(&parameter_id(BRAIDS_TIMBRE_PARAMETER_ID)),
            Some(&ParameterValue::continuous(0.5).unwrap())
        );
        assert_eq!(
            braids.value(&parameter_id(BRAIDS_COLOR_PARAMETER_ID)),
            Some(&ParameterValue::continuous(0.5).unwrap())
        );
        assert!(braids.asset_references().is_empty());
    }

    struct NeverCreateProvider {
        descriptor: CapabilityDescriptor,
        calls: Arc<AtomicUsize>,
    }

    impl InstrumentCapabilityProvider for NeverCreateProvider {
        fn descriptor(&self) -> CapabilityDescriptor {
            self.descriptor.clone()
        }

        fn create_config(
            &self,
            _values: &[ParameterAssignment],
            _asset_references: &[AssetAssignment],
        ) -> Result<InstrumentConfig, CapabilityError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            unreachable!("a mismatched provider must not be called")
        }
    }

    struct InvalidOutputProvider {
        descriptor: CapabilityDescriptor,
    }

    impl InstrumentCapabilityProvider for InvalidOutputProvider {
        fn descriptor(&self) -> CapabilityDescriptor {
            self.descriptor.clone()
        }

        fn create_config(
            &self,
            _values: &[ParameterAssignment],
            _asset_references: &[AssetAssignment],
        ) -> Result<InstrumentConfig, CapabilityError> {
            Ok(InstrumentConfig::from_parts(
                self.descriptor.id().clone(),
                Vec::new(),
                Vec::new(),
            ))
        }
    }

    #[test]
    fn descriptor_default_config_factory_rejects_unknown_missing_duplicate_and_mismatched_providers_without_fallback(
    ) {
        let hidef =
            crate::adapter::production_instruments::production_soundfont_capability().unwrap();
        let braids = BraidsCapability::new().unwrap();
        let registry =
            CapabilityRegistry::new(vec![hidef.descriptor(), braids.descriptor()]).unwrap();

        let missing = DescriptorDefaultConfigFactory::new(
            registry.clone(),
            vec![Box::new(
                crate::adapter::production_instruments::production_soundfont_capability().unwrap(),
            )],
        );
        assert_eq!(
            missing.create(&capability_id(BRAIDS_CAPABILITY_ID)),
            Err(CapabilityError::ProviderRegistryMismatch(capability_id(
                BRAIDS_CAPABILITY_ID
            )))
        );
        let unknown = capability_id("instrument.unknown.test");
        assert_eq!(
            missing.create(&unknown),
            Err(CapabilityError::UnknownCapability(unknown))
        );

        let duplicate = DescriptorDefaultConfigFactory::new(
            registry.clone(),
            vec![
                Box::new(
                    crate::adapter::production_instruments::production_soundfont_capability()
                        .unwrap(),
                ),
                Box::new(
                    crate::adapter::production_instruments::production_soundfont_capability()
                        .unwrap(),
                ),
                Box::new(BraidsCapability::new().unwrap()),
            ],
        );
        assert_eq!(
            duplicate.create(&capability_id(HIDEF_CAPABILITY_ID)),
            Err(CapabilityError::ProviderRegistryMismatch(capability_id(
                HIDEF_CAPABILITY_ID
            )))
        );

        let canonical = hidef.descriptor();
        let mismatched = CapabilityDescriptor::new(
            canonical.id().clone(),
            "Wrong provider descriptor",
            canonical.semantic_accent(),
            canonical.sections().to_vec(),
            canonical.asset_requirements().to_vec(),
            canonical.voice_policy(),
            canonical.supported_midi_kinds().to_vec(),
        )
        .unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let factory = DescriptorDefaultConfigFactory::new(
            registry,
            vec![Box::new(NeverCreateProvider {
                descriptor: mismatched,
                calls: Arc::clone(&calls),
            })],
        );
        assert_eq!(
            factory.create(&capability_id(HIDEF_CAPABILITY_ID)),
            Err(CapabilityError::ProviderRegistryMismatch(capability_id(
                HIDEF_CAPABILITY_ID
            )))
        );
        assert_eq!(calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn descriptor_default_config_factory_revalidates_provider_output() {
        let hidef =
            crate::adapter::production_instruments::production_soundfont_capability().unwrap();
        let registry = CapabilityRegistry::new(vec![hidef.descriptor()]).unwrap();
        let factory = DescriptorDefaultConfigFactory::new(
            registry,
            vec![Box::new(InvalidOutputProvider {
                descriptor: hidef.descriptor(),
            })],
        );

        assert_eq!(
            factory.create(&capability_id(HIDEF_CAPABILITY_ID)),
            Err(CapabilityError::MissingParameter(parameter_id(
                SOUNDFONT_PRESET_PARAMETER_ID
            )))
        );
    }

    #[test]
    fn descriptor_default_config_factory_replaces_exactly_one_structural_choice() {
        let factory = production_factory();
        let source = factory.create(&capability_id(HIDEF_CAPABILITY_ID)).unwrap();
        let catalog = crate::adapter::production_instruments::production_soundfont_asset()
            .unwrap()
            .catalog();
        let next = catalog.entries()[1].choice_id();
        let candidate = factory
            .replace_structural_choice(&source, &parameter_id(SOUNDFONT_PRESET_PARAMETER_ID), &next)
            .unwrap();

        assert_eq!(candidate.capability_id(), source.capability_id());
        assert_eq!(candidate.asset_references(), source.asset_references());
        assert_eq!(candidate.values().len(), source.values().len());
        assert_eq!(
            candidate.value(&parameter_id(SOUNDFONT_PRESET_PARAMETER_ID)),
            Some(&ParameterValue::Choice(next))
        );
        assert!(matches!(
            factory.replace_structural_choice(
                &source,
                &parameter_id(SOUNDFONT_PRESET_PARAMETER_ID),
                "sf2.bank-65535.program-127",
            ),
            Err(CapabilityError::UnknownChoice(_))
        ));
    }
}
