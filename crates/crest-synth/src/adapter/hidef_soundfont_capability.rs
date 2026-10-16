use crate::kernel::midi_message::MidiMessageKind;
use crate::synth::{
    AssetAssignment, AssetKind, AssetReference, AssetRequirement, CapabilityDescriptor,
    CapabilityError, CapabilityId, CapabilityRegistry, CapabilitySection,
    InstrumentCapabilityProvider, InstrumentConfig, ParameterAssignment, ParameterChoice,
    ParameterDefault, ParameterId, ParameterKind, ParameterSpec, ParameterUpdate, ParameterValue,
    PatchInteraction, SoundFontPresetCatalog, VoicePolicy,
};
use std::sync::Arc;

pub const HIDEF_CAPABILITY_ID: &str = "instrument.soundfont.hidef";
pub const HIDEF_SOUNDFONT_PATH: &str = "./sf2/HiDef.sf2";
pub const SOUNDFONT_PRESET_PARAMETER_ID: &str = "soundfont.preset";
pub const SOUNDFONT_FILE_PARAMETER_ID: &str = "soundfont.file";
pub const HIDEF_POLYPHONY_CEILING: u16 = 64;
pub const HIDEF_SUPPORTED_MIDI_KINDS: [MidiMessageKind; 7] = [
    MidiMessageKind::NoteOn,
    MidiMessageKind::NoteOff,
    MidiMessageKind::ControlChange,
    MidiMessageKind::ChannelPressure,
    MidiMessageKind::PolyPressure,
    MidiMessageKind::PitchBend,
    MidiMessageKind::AllNotesOff,
];

/// Control-side descriptor/config provider hydrated from the one shared asset
/// catalog. Provider operations perform no file I/O or SF2 parsing.
#[derive(Clone, Debug)]
pub struct HiDefSoundFontCapability {
    descriptor: CapabilityDescriptor,
}

impl HiDefSoundFontCapability {
    pub fn new(catalog: Arc<SoundFontPresetCatalog>) -> Result<Self, CapabilityError> {
        Self::for_asset(
            catalog,
            AssetReference::new(AssetKind::SoundFont, HIDEF_SOUNDFONT_PATH)?,
        )
    }

    pub fn for_asset(
        catalog: Arc<SoundFontPresetCatalog>,
        reference: AssetReference,
    ) -> Result<Self, CapabilityError> {
        if reference.kind() != AssetKind::SoundFont {
            return Err(CapabilityError::WrongAssetKind(parameter_id(
                SOUNDFONT_FILE_PARAMETER_ID,
            )?));
        }
        let preset_id = parameter_id(SOUNDFONT_PRESET_PARAMETER_ID)?;
        let choices = catalog
            .entries()
            .iter()
            .map(|entry| ParameterChoice::new(entry.choice_id(), entry.name()))
            .collect::<Result<Vec<_>, _>>()?;
        let preset = ParameterSpec::new_with_patch_interaction(
            preset_id,
            "Preset",
            ParameterKind::Choice,
            ParameterUpdate::Structural,
            PatchInteraction::StructuralChoice,
            ParameterDefault::Value(ParameterValue::Choice(catalog.default_entry().choice_id())),
            None,
            choices,
            None,
            None,
            None,
            "choice",
            None,
            None,
        )?;
        let file_id = parameter_id(SOUNDFONT_FILE_PARAMETER_ID)?;
        let file = ParameterSpec::new_with_patch_interaction(
            file_id.clone(),
            "SoundFont File",
            ParameterKind::Asset,
            ParameterUpdate::Structural,
            PatchInteraction::ReadOnly,
            ParameterDefault::Asset(reference),
            None,
            Vec::new(),
            None,
            None,
            None,
            "asset",
            None,
            None,
        )?;
        let descriptor = CapabilityDescriptor::new(
            CapabilityId::new(HIDEF_CAPABILITY_ID).map_err(|_| {
                CapabilityError::InvalidMetadataIdentifier(HIDEF_CAPABILITY_ID.to_owned())
            })?,
            "HiDef SoundFont",
            "instrument.soundfont",
            vec![CapabilitySection::new(
                "soundfont",
                "SoundFont",
                vec![preset, file],
            )?],
            vec![AssetRequirement::new(file_id, true)],
            VoicePolicy::Configurable { default_voices: 64 },
            HIDEF_SUPPORTED_MIDI_KINDS.to_vec(),
        )?;
        Ok(Self {
            descriptor: descriptor
                .with_instrument_category(crate::synth::InstrumentCategory::Samplers)
                .with_asset_scoped_choices(),
        })
    }

    pub fn registry(&self) -> Result<CapabilityRegistry, CapabilityError> {
        CapabilityRegistry::new(vec![self.descriptor()])
    }
}

impl InstrumentCapabilityProvider for HiDefSoundFontCapability {
    fn descriptor(&self) -> CapabilityDescriptor {
        self.descriptor.clone()
    }

    fn create_config(
        &self,
        values: &[ParameterAssignment],
        asset_references: &[AssetAssignment],
    ) -> Result<InstrumentConfig, CapabilityError> {
        self.descriptor.create_config(values, asset_references)
    }
}

fn parameter_id(value: &str) -> Result<ParameterId, CapabilityError> {
    ParameterId::new(value)
        .map_err(|_| CapabilityError::InvalidMetadataIdentifier(value.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::hidef_soundfont_asset::HiDefSoundFontAsset;

    #[test]
    fn hidef_soundfont_capability_declares_the_exact_catalog_backed_schema() {
        let asset = HiDefSoundFontAsset::load().unwrap();
        let catalog = asset.catalog();
        let provider = HiDefSoundFontCapability::new(Arc::clone(&catalog)).unwrap();
        let descriptor = provider.descriptor();

        assert_eq!(descriptor.id().as_str(), HIDEF_CAPABILITY_ID);
        assert_eq!(
            descriptor.voice_policy(),
            VoicePolicy::Configurable { default_voices: 64 }
        );
        let parameters = descriptor.sections()[0].parameters();
        assert_eq!(
            parameters
                .iter()
                .map(|parameter| parameter.id().as_str())
                .collect::<Vec<_>>(),
            [SOUNDFONT_PRESET_PARAMETER_ID, SOUNDFONT_FILE_PARAMETER_ID]
        );
        assert_eq!(parameters[0].kind(), ParameterKind::Choice);
        assert_eq!(parameters[0].update(), ParameterUpdate::Structural);
        assert_eq!(
            parameters[0].patch_interaction(),
            PatchInteraction::StructuralChoice
        );
        assert_eq!(parameters[1].kind(), ParameterKind::Asset);
        assert_eq!(
            parameters[1].patch_interaction(),
            PatchInteraction::ReadOnly
        );
        assert_eq!(
            parameters[0]
                .choices()
                .iter()
                .map(|choice| (choice.id(), choice.label()))
                .collect::<Vec<_>>(),
            catalog
                .entries()
                .iter()
                .map(|entry| (entry.choice_id(), entry.name().to_owned()))
                .collect::<Vec<_>>()
                .iter()
                .map(|(id, name)| (id.as_str(), name.as_str()))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            parameters[0].default_value(),
            &ParameterDefault::Value(ParameterValue::Choice(catalog.default_entry().choice_id()))
        );
    }

    #[test]
    fn hidef_soundfont_capability_validates_exact_preset_and_locked_asset() {
        let asset = HiDefSoundFontAsset::load().unwrap();
        let catalog = asset.catalog();
        let provider = HiDefSoundFontCapability::new(Arc::clone(&catalog)).unwrap();
        let preset_id = parameter_id(SOUNDFONT_PRESET_PARAMETER_ID).unwrap();
        let file_id = parameter_id(SOUNDFONT_FILE_PARAMETER_ID).unwrap();
        let config = provider
            .create_config(
                &[ParameterAssignment::new(
                    preset_id.clone(),
                    ParameterValue::Choice(catalog.default_entry().choice_id()),
                )],
                &[AssetAssignment::new(
                    file_id.clone(),
                    AssetReference::new(AssetKind::SoundFont, HIDEF_SOUNDFONT_PATH).unwrap(),
                )],
            )
            .unwrap();
        provider
            .registry()
            .unwrap()
            .validate_config(&config)
            .unwrap();
        assert_eq!(config.values().len(), 1);
        assert_eq!(config.asset_references().len(), 1);

        assert!(matches!(
            provider.create_config(
                &[ParameterAssignment::new(
                    preset_id,
                    ParameterValue::Choice("sf2.bank-65535.program-127".to_owned()),
                )],
                &[AssetAssignment::new(
                    file_id,
                    AssetReference::new(AssetKind::SoundFont, HIDEF_SOUNDFONT_PATH).unwrap(),
                )],
            ),
            Err(CapabilityError::UnknownChoice(_))
        ));
    }
}
