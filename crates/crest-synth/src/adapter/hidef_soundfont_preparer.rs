use crate::adapter::hidef_soundfont_asset::HiDefSoundFontAsset;
use crate::adapter::hidef_soundfont_capability::{
    HIDEF_CAPABILITY_ID, HIDEF_SOUNDFONT_PATH, SOUNDFONT_FILE_PARAMETER_ID,
    SOUNDFONT_PRESET_PARAMETER_ID,
};
use crate::adapter::soundfont_voice_engine::PreparedSoundFontBank;
#[cfg(test)]
use crate::adapter::{
    hidef_soundfont_capability::HIDEF_POLYPHONY_CEILING,
    soundfont_voice_engine::SoundFontVoiceEngine,
};
#[cfg(test)]
use crate::kernel::midi_message::MidiMessage;
use crate::kernel::patch_id::PatchId;
use crate::synth::capability_id::CapabilityId;
use crate::synth::instrument_capability::{AssetKind, ParameterValue};
use crate::synth::instrument_preparer::{InstrumentPreparationError, InstrumentPreparer};
use crate::synth::parameter_id::ParameterId;
use crate::synth::patch::Patch;
use crate::synth::prepared_instrument::PreparedInstrument;
#[cfg(test)]
use crate::synth::prepared_instrument::PreparedInstrumentError;
use std::sync::Arc;

#[cfg(test)]
const SOUNDFONT_ENGINE_VOICE_SLOTS: usize = HIDEF_POLYPHONY_CEILING as usize;

/// Control/worker-side SoundFont preparer with a compatible bundled default.
///
/// Construction receives the bundled bank's immutable projections. A configured
/// library resolves other selected banks off-thread. Patches sharing an active
/// bank reuse numeric ownership with independent voices, MIDI state, and scratch.
pub struct HiDefSoundFontPreparer {
    capability_id: CapabilityId,
    catalog: Arc<crate::synth::SoundFontPresetCatalog>,
    prepared_bank: Arc<PreparedSoundFontBank>,
    parsed_bank_count: usize,
    library: Option<Arc<super::filesystem_soundfont_catalog::FilesystemSoundFontCatalog>>,
}

impl HiDefSoundFontPreparer {
    /// Accepts the two projections of the already parsed production asset.
    pub fn new(asset: &HiDefSoundFontAsset) -> Result<Self, InstrumentPreparationError> {
        let capability_id = CapabilityId::new(HIDEF_CAPABILITY_ID)
            .map_err(|_| InstrumentPreparationError::AssetParseFailed)?;
        Ok(Self {
            capability_id,
            catalog: asset.catalog(),
            prepared_bank: asset.prepared_bank(),
            parsed_bank_count: asset.parse_count(),
            library: None,
        })
    }

    pub fn with_library(
        mut self,
        library: Arc<super::filesystem_soundfont_catalog::FilesystemSoundFontCatalog>,
    ) -> Self {
        self.library = Some(library);
        self
    }

    fn asset_for(
        &self,
        config: &crate::synth::InstrumentConfig,
    ) -> Result<HiDefSoundFontAsset, InstrumentPreparationError> {
        let reference = config
            .asset_reference(
                &ParameterId::new(SOUNDFONT_FILE_PARAMETER_ID)
                    .map_err(|_| InstrumentPreparationError::AssetParseFailed)?,
            )
            .ok_or(InstrumentPreparationError::AssetParseFailed)?;
        if reference.kind() != AssetKind::SoundFont {
            return Err(InstrumentPreparationError::AssetParseFailed);
        }
        if reference.locator() == HIDEF_SOUNDFONT_PATH {
            return Ok(HiDefSoundFontAsset::from_projections(
                self.catalog.clone(),
                self.prepared_bank.clone(),
            ));
        }
        self.library
            .as_ref()
            .ok_or(InstrumentPreparationError::AssetLoadFailed)?
            .load(reference)
            .map_err(|error| match error {
                crate::synth::SampleAssetError::Unavailable
                | crate::synth::SampleAssetError::DownloadRequired => {
                    InstrumentPreparationError::AssetLoadFailed
                }
                _ => InstrumentPreparationError::AssetParseFailed,
            })
    }

    /// Returns the bundled asset's parse count, excluding later library loads.
    pub const fn parsed_bank_count(&self) -> usize {
        self.parsed_bank_count
    }

    #[cfg(test)]
    fn prepare_patch(
        &self,
        patch: &Patch,
        sample_rate: f32,
        max_frames: usize,
    ) -> Result<HiDefPreparedInstrument, InstrumentPreparationError> {
        let sample_rate = validated_sample_rate(sample_rate)?;
        if max_frames == 0 {
            return Err(InstrumentPreparationError::InvalidFrameCapacity);
        }

        if patch.instrument_config().capability_id() != &self.capability_id {
            return Err(InstrumentPreparationError::UnsupportedCapability {
                patch_id: patch.id(),
            });
        }
        let config = patch.instrument_config();
        let file = config.asset_reference(&ParameterId::new(SOUNDFONT_FILE_PARAMETER_ID).map_err(
            |_| InstrumentPreparationError::InvalidConfiguration {
                patch_id: patch.id(),
            },
        )?);
        if config.values().len() != 1
            || config.asset_references().len() != 1
            || file.is_none_or(|reference| {
                reference.kind() != AssetKind::SoundFont
                    || (self.library.is_none() && reference.locator() != HIDEF_SOUNDFONT_PATH)
            })
        {
            return Err(InstrumentPreparationError::InvalidConfiguration {
                patch_id: patch.id(),
            });
        }
        let asset = self.asset_for(patch.instrument_config())?;
        let bank = asset.prepared_bank();
        let prepared = PreparedPatch::try_from_patch(patch, &asset.catalog())?;
        if !bank.has_preset(prepared.preset_id) {
            return Err(InstrumentPreparationError::PresetUnavailable {
                patch_id: patch.id(),
            });
        }
        let engine = SoundFontVoiceEngine::<SOUNDFONT_ENGINE_VOICE_SLOTS>::new(
            bank,
            sample_rate as f32,
            max_frames,
            prepared.preset_id,
        )
        .map_err(|_| InstrumentPreparationError::VoiceCapacityExceeded {
            patch_id: patch.id(),
        })?;

        Ok(HiDefPreparedInstrument {
            prepared,
            engine,
            max_frames,
        })
    }
}

impl InstrumentPreparer for HiDefSoundFontPreparer {
    fn capability_id(&self) -> &CapabilityId {
        &self.capability_id
    }

    fn asset_descriptor(
        &self,
        config: &crate::synth::InstrumentConfig,
    ) -> Result<Option<crate::synth::CapabilityDescriptor>, InstrumentPreparationError> {
        use crate::synth::InstrumentCapabilityProvider;
        let asset = self.asset_for(config)?;
        let reference = config
            .asset_references()
            .first()
            .ok_or(InstrumentPreparationError::AssetParseFailed)?
            .reference()
            .clone();
        super::hidef_soundfont_capability::HiDefSoundFontCapability::for_asset(
            asset.catalog(),
            reference,
        )
        .map(|provider| Some(provider.descriptor()))
        .map_err(|_| InstrumentPreparationError::AssetParseFailed)
    }

    fn prepared_shared_asset_count(&self) -> usize {
        self.parsed_bank_count
    }

    fn prepare(
        &self,
        patch: &Patch,
        sample_rate: f32,
        max_frames: usize,
    ) -> Result<Box<dyn PreparedInstrument>, InstrumentPreparationError> {
        if patch.instrument_config().capability_id() != &self.capability_id {
            return Err(InstrumentPreparationError::UnsupportedCapability {
                patch_id: patch.id(),
            });
        }
        let rate = validated_sample_rate(sample_rate)?;
        if max_frames == 0 {
            return Err(InstrumentPreparationError::InvalidFrameCapacity);
        }
        let config = patch.instrument_config();
        let file = config.asset_reference(&ParameterId::new(SOUNDFONT_FILE_PARAMETER_ID).map_err(
            |_| InstrumentPreparationError::InvalidConfiguration {
                patch_id: patch.id(),
            },
        )?);
        if config.values().len() != 1
            || config.asset_references().len() != 1
            || file.is_none_or(|reference| {
                reference.kind() != AssetKind::SoundFont
                    || (self.library.is_none() && reference.locator() != HIDEF_SOUNDFONT_PATH)
            })
        {
            return Err(InstrumentPreparationError::InvalidConfiguration {
                patch_id: patch.id(),
            });
        }
        let asset = self.asset_for(patch.instrument_config())?;
        let prepared = PreparedPatch::try_from_patch(patch, &asset.catalog())?;
        let font = asset.prepared_bank().upstream.clone();
        super::upstream_audio::prepare_voice_bank(patch, rate as f32, max_frames, || {
            super::rustysynth_voice::RustyVoice::new(
                &font,
                prepared.preset_id,
                rate as f32,
                max_frames,
            )
        })
    }
}

#[cfg(test)]
struct HiDefPreparedInstrument {
    prepared: PreparedPatch,
    engine: SoundFontVoiceEngine<SOUNDFONT_ENGINE_VOICE_SLOTS>,
    max_frames: usize,
}

#[cfg(test)]
impl PreparedInstrument for HiDefPreparedInstrument {
    fn patch_id(&self) -> PatchId {
        self.prepared.patch_id
    }

    fn dispatch(
        &mut self,
        message: MidiMessage,
        parameters: &crate::real_time::RtPatchParameters,
    ) -> Result<(), PreparedInstrumentError> {
        if parameters.patch_id() != Some(self.prepared.patch_id) {
            return Err(PreparedInstrumentError::DispatchRejected);
        }
        self.engine.dispatch(message, *parameters.envelope())
    }

    fn render(
        &mut self,
        interleaved_stereo: &mut [f32],
        frame_count: usize,
        parameters: &crate::real_time::RtPatchParameters,
    ) -> Result<(), crate::synth::PreparedInstrumentError> {
        let frame_count = frame_count
            .min(self.max_frames)
            .min(interleaved_stereo.len() / 2);
        if frame_count == 0 {
            return Ok(());
        }

        let output = &mut interleaved_stereo[..frame_count * 2];
        output.fill(0.0);
        if parameters.patch_id() == Some(self.prepared.patch_id) {
            self.engine.render(output, frame_count);
        }

        Ok(())
    }

    fn all_notes_off(&mut self) {
        self.engine.all_notes_off();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PreparedPatch {
    patch_id: PatchId,
    preset_id: crate::synth::SoundFontPresetId,
}

impl PreparedPatch {
    fn try_from_patch(
        patch: &Patch,
        catalog: &crate::synth::SoundFontPresetCatalog,
    ) -> Result<Self, InstrumentPreparationError> {
        let config = patch.instrument_config();
        if config.capability_id().as_str() != HIDEF_CAPABILITY_ID {
            return Err(InstrumentPreparationError::UnsupportedCapability {
                patch_id: patch.id(),
            });
        }
        let parameter_id = |value: &str| {
            ParameterId::new(value).map_err(|_| InstrumentPreparationError::InvalidConfiguration {
                patch_id: patch.id(),
            })
        };
        let preset_choice = match config.value(&parameter_id(SOUNDFONT_PRESET_PARAMETER_ID)?) {
            Some(ParameterValue::Choice(value)) => Some(value.as_str()),
            _ => None,
        };
        let file = config.asset_reference(&parameter_id(SOUNDFONT_FILE_PARAMETER_ID)?);
        let (Some(preset_choice), Some(file)) = (preset_choice, file) else {
            return Err(InstrumentPreparationError::InvalidConfiguration {
                patch_id: patch.id(),
            });
        };
        let preset_id = preset_choice.parse().map_err(|_| {
            InstrumentPreparationError::InvalidConfiguration {
                patch_id: patch.id(),
            }
        })?;
        if catalog.entry(preset_id).is_none() {
            return Err(InstrumentPreparationError::PresetUnavailable {
                patch_id: patch.id(),
            });
        }
        if file.kind() != AssetKind::SoundFont
            || config.values().len() != 1
            || config.asset_references().len() != 1
        {
            return Err(InstrumentPreparationError::InvalidConfiguration {
                patch_id: patch.id(),
            });
        }

        Ok(Self {
            patch_id: patch.id(),
            preset_id,
        })
    }
}

fn validated_sample_rate(sample_rate: f32) -> Result<i32, InstrumentPreparationError> {
    if !sample_rate.is_finite()
        || sample_rate.fract() != 0.0
        || !(16_000.0..=192_000.0).contains(&sample_rate)
    {
        return Err(InstrumentPreparationError::InvalidSampleRate);
    }
    Ok(sample_rate as i32)
}

#[cfg(test)]
mod tests {
    use super::{HiDefSoundFontPreparer, PreparedPatch, HIDEF_SOUNDFONT_PATH};
    use crate::adapter::hidef_soundfont_capability::HIDEF_CAPABILITY_ID;
    use crate::kernel::midi_channel::MidiChannel;
    use crate::kernel::midi_message::{MidiMessage, MidiMessageKind};
    use crate::kernel::patch_id::PatchId;
    use crate::mixer::patch_output::PatchOutput;
    use crate::real_time::parameter_snapshot::RtInstrumentParameters;
    use crate::synth::capability_id::CapabilityId;
    use crate::synth::instrument_capability::{
        AssetAssignment, AssetKind, AssetReference, InstrumentConfig,
    };
    use crate::synth::instrument_preparer::{InstrumentPreparationError, InstrumentPreparer};
    use crate::synth::parameter_id::ParameterId;
    use crate::synth::patch::Patch;
    use crate::synth::prepared_instrument::PreparedInstrument;
    use crate::synth::sound_font_instrument::SoundFontInstrument;
    use crate::synth::voice_envelope::VoiceEnvelope;
    use crate::testing::automatic_midi_test::create_soundfont_config;
    use std::sync::Arc;

    fn patch(id: u32, channel: u8, bank: u16, program: u8, percussion: bool) -> Patch {
        let provider =
            crate::adapter::production_instruments::production_soundfont_capability().unwrap();
        Patch::new(
            PatchId::new(id).unwrap(),
            format!("Patch {id}"),
            create_soundfont_config(
                &provider,
                SoundFontInstrument::new(bank, program, percussion).unwrap(),
            )
            .unwrap(),
            MidiChannel::new(channel).unwrap(),
            PatchOutput::default(),
        )
    }

    #[test]
    fn one_parse_supplies_independent_melodic_and_percussion_instruments() {
        let asset = crate::adapter::hidef_soundfont_asset::HiDefSoundFontAsset::load().unwrap();
        let preparer = HiDefSoundFontPreparer::new(&asset).unwrap();
        let melodic_patch = patch(1, 3, 0, 0, false);
        let percussion_patch = patch(2, 4, 0, 0, true);
        let shared_count = Arc::strong_count(&preparer.prepared_bank);
        let mut melodic = preparer
            .prepare_patch(&melodic_patch, 48_000.0, 512)
            .unwrap();
        let percussion = preparer
            .prepare_patch(&percussion_patch, 48_000.0, 512)
            .unwrap();

        assert_eq!(preparer.parsed_bank_count(), 1);
        assert_eq!(Arc::strong_count(&preparer.prepared_bank), shared_count + 2);
        assert_ne!(melodic.prepared, percussion.prepared);
        assert_eq!(melodic.prepared.preset_id.bank(), 0);
        assert_eq!(percussion.prepared.preset_id.bank(), 128);
        assert!(!melodic.prepared.preset_id.is_percussion());
        assert!(percussion.prepared.preset_id.is_percussion());
        let parameters =
            crate::real_time::RtPatchParameters::new(melodic_patch.id(), melodic_patch.output());

        melodic
            .dispatch(
                MidiMessage::try_new(melodic_patch.channel(), MidiMessageKind::NoteOn, 60, 110)
                    .unwrap(),
                &parameters,
            )
            .unwrap();
        let mut output = [0.0; 1_024];
        melodic.render(&mut output, 512, &parameters).unwrap();

        assert!(output.iter().all(|sample| sample.is_finite()));
        assert!(output.iter().any(|sample| sample.abs() > 0.000_001));
        assert_eq!(percussion.engine.active_note_voice_count(), 0);
    }

    #[test]
    fn preparer_rejects_invalid_assets_configs_presets_and_audio_limits() {
        let preparer = HiDefSoundFontPreparer::new(
            crate::adapter::production_instruments::production_soundfont_asset().unwrap(),
        )
        .unwrap();
        let valid = patch(1, 0, 0, 0, false);
        assert!(matches!(
            preparer.prepare(&valid, f32::NAN, 512),
            Err(InstrumentPreparationError::InvalidSampleRate)
        ));
        assert!(matches!(
            preparer.prepare(&valid, 48_000.0, 0),
            Err(InstrumentPreparationError::InvalidFrameCapacity)
        ));

        let unsupported = Patch::new(
            PatchId::new(2).unwrap(),
            "Unsupported".to_owned(),
            InstrumentConfig::from_parts(
                CapabilityId::new("instrument.test.other").unwrap(),
                Vec::new(),
                Vec::new(),
            ),
            MidiChannel::new(0).unwrap(),
            PatchOutput::default(),
        );
        assert!(matches!(
            preparer.prepare(&unsupported, 48_000.0, 512),
            Err(InstrumentPreparationError::UnsupportedCapability { .. })
        ));

        let malformed = Patch::new(
            PatchId::new(3).unwrap(),
            "Malformed".to_owned(),
            InstrumentConfig::from_parts(
                CapabilityId::new(HIDEF_CAPABILITY_ID).unwrap(),
                Vec::new(),
                Vec::new(),
            ),
            MidiChannel::new(1).unwrap(),
            PatchOutput::default(),
        );
        assert!(matches!(
            preparer.prepare(&malformed, 48_000.0, 512),
            Err(InstrumentPreparationError::InvalidConfiguration { .. })
        ));

        let provider =
            crate::adapter::production_instruments::production_soundfont_capability().unwrap();
        let mut wrong_asset =
            create_soundfont_config(&provider, SoundFontInstrument::new(0, 0, false).unwrap())
                .unwrap();
        let file_id = ParameterId::new("soundfont.file").unwrap();
        wrong_asset = InstrumentConfig::from_parts(
            wrong_asset.capability_id().clone(),
            wrong_asset.values().to_vec(),
            vec![AssetAssignment::new(
                file_id,
                AssetReference::new(AssetKind::SoundFont, "./sf2/Other.sf2").unwrap(),
            )],
        );
        let wrong_asset = Patch::new(
            PatchId::new(4).unwrap(),
            "Wrong asset".to_owned(),
            wrong_asset,
            MidiChannel::new(2).unwrap(),
            PatchOutput::default(),
        );
        assert!(matches!(
            preparer.prepare(&wrong_asset, 48_000.0, 512),
            Err(InstrumentPreparationError::InvalidConfiguration { .. })
        ));

        let unavailable_preset = Patch::new(
            PatchId::new(5).unwrap(),
            "Unavailable".to_owned(),
            InstrumentConfig::from_parts(
                CapabilityId::new(HIDEF_CAPABILITY_ID).unwrap(),
                vec![crate::synth::ParameterAssignment::new(
                    ParameterId::new(
                        crate::adapter::hidef_soundfont_capability::SOUNDFONT_PRESET_PARAMETER_ID,
                    )
                    .unwrap(),
                    crate::synth::ParameterValue::Choice(
                        crate::synth::SoundFontPresetId::new(u16::MAX, 127)
                            .unwrap()
                            .choice_id(),
                    ),
                )],
                vec![AssetAssignment::new(
                    ParameterId::new(
                        crate::adapter::hidef_soundfont_capability::SOUNDFONT_FILE_PARAMETER_ID,
                    )
                    .unwrap(),
                    AssetReference::new(AssetKind::SoundFont, HIDEF_SOUNDFONT_PATH).unwrap(),
                )],
            ),
            MidiChannel::new(3).unwrap(),
            PatchOutput::default(),
        );
        assert!(matches!(
            preparer.prepare(&unavailable_preset, 48_000.0, 512),
            Err(InstrumentPreparationError::PresetUnavailable { .. })
        ));
    }

    #[test]
    fn prepared_mapping_is_patch_scoped_and_uses_the_fixed_asset() {
        let catalog = crate::adapter::production_instruments::production_soundfont_asset()
            .unwrap()
            .catalog();
        let melodic_id = catalog
            .entries()
            .iter()
            .find(|entry| entry.id().bank() == 0)
            .unwrap()
            .id();
        let percussion_id = catalog
            .entries()
            .iter()
            .find(|entry| entry.id().bank() == 128)
            .unwrap()
            .id();
        let melodic = PreparedPatch::try_from_patch(
            &patch(7, 12, melodic_id.bank(), melodic_id.program(), false),
            &catalog,
        )
        .unwrap();
        let percussion = PreparedPatch::try_from_patch(
            &patch(8, 13, 0, percussion_id.program(), true),
            &catalog,
        )
        .unwrap();

        assert_eq!(HIDEF_SOUNDFONT_PATH, "./sf2/HiDef.sf2");
        assert_eq!(melodic.patch_id, PatchId::new(7).unwrap());
        assert_eq!(melodic.preset_id.bank(), 0);
        assert_eq!(percussion.preset_id.bank(), 128);
        assert!(!melodic.preset_id.is_percussion());
        assert!(percussion.preset_id.is_percussion());
    }

    fn projected_parameters(
        patch: &Patch,
        envelope: VoiceEnvelope,
    ) -> crate::real_time::RtPatchParameters {
        crate::real_time::RtPatchParameters::projected(
            patch.id(),
            patch.output(),
            envelope,
            RtInstrumentParameters::EMPTY,
        )
    }

    fn note(channel: MidiChannel, kind: MidiMessageKind, key: u8, velocity: u8) -> MidiMessage {
        MidiMessage::try_new(channel, kind, key, velocity).unwrap()
    }

    fn energy(output: &[f32]) -> f32 {
        output.iter().map(|sample| sample.abs()).sum()
    }

    #[test]
    fn common_adsr_is_owned_by_each_overlapping_soundfont_note_voice() {
        let preparer = HiDefSoundFontPreparer::new(
            crate::adapter::production_instruments::production_soundfont_asset().unwrap(),
        )
        .unwrap();
        let patch = patch(1, 1, 0, 0, false);
        let envelope = VoiceEnvelope::new(0.0, 0.0, 1.0, 100.0).unwrap();
        let parameters = projected_parameters(&patch, envelope);
        let mut prepared = preparer.prepare_patch(&patch, 48_000.0, 256).unwrap();

        prepared
            .dispatch(
                note(patch.channel(), MidiMessageKind::NoteOn, 60, 110),
                &parameters,
            )
            .unwrap();
        prepared
            .dispatch(
                note(patch.channel(), MidiMessageKind::NoteOn, 64, 110),
                &parameters,
            )
            .unwrap();
        let first = prepared.engine.note_voice_counts(60);
        let second = prepared.engine.note_voice_counts(64);
        assert!(first.0 > 0 && second.0 > 0);

        prepared
            .dispatch(
                note(patch.channel(), MidiMessageKind::NoteOff, 60, 0),
                &parameters,
            )
            .unwrap();
        let first = prepared.engine.note_voice_counts(60);
        let second = prepared.engine.note_voice_counts(64);
        assert_eq!(first.1, first.0);
        assert_eq!(second.1, 0);

        let mut output = [0.0; 512];
        prepared.render(&mut output, 256, &parameters).unwrap();
        assert!(output.iter().all(|sample| sample.is_finite()));
        assert!(energy(&output) > 0.0);
        prepared.all_notes_off();
        assert_eq!(prepared.engine.active_note_voice_count(), 0);
    }

    #[test]
    fn every_common_adsr_field_changes_soundfont_audio_before_the_patch_stem() {
        let preparer = HiDefSoundFontPreparer::new(
            crate::adapter::production_instruments::production_soundfont_asset().unwrap(),
        )
        .unwrap();
        let patch = patch(1, 1, 0, 0, false);

        let render_onset = |envelope: VoiceEnvelope| {
            let parameters = projected_parameters(&patch, envelope);
            let mut prepared = preparer.prepare_patch(&patch, 48_000.0, 256).unwrap();
            prepared
                .dispatch(
                    note(patch.channel(), MidiMessageKind::NoteOn, 60, 127),
                    &parameters,
                )
                .unwrap();
            let mut output = [0.0; 512];
            prepared.render(&mut output, 256, &parameters).unwrap();
            energy(&output)
        };

        let immediate = render_onset(VoiceEnvelope::DEFAULT);
        let attacked = render_onset(VoiceEnvelope::new(100.0, 0.0, 1.0, 0.0).unwrap());
        let decayed = render_onset(VoiceEnvelope::new(0.0, 100.0, 0.0, 0.0).unwrap());
        let sustained = render_onset(VoiceEnvelope::new(0.0, 0.0, 0.25, 0.0).unwrap());
        assert!(immediate > attacked);
        assert!(immediate > decayed);
        assert!(immediate > sustained);

        let release_energy = |release_milliseconds: f32| {
            let envelope = VoiceEnvelope::new(0.0, 0.0, 1.0, release_milliseconds).unwrap();
            let parameters = projected_parameters(&patch, envelope);
            let mut prepared = preparer.prepare_patch(&patch, 48_000.0, 256).unwrap();
            prepared
                .dispatch(
                    note(patch.channel(), MidiMessageKind::NoteOn, 60, 127),
                    &parameters,
                )
                .unwrap();
            let mut onset = [0.0; 512];
            prepared.render(&mut onset, 256, &parameters).unwrap();
            prepared
                .dispatch(
                    note(patch.channel(), MidiMessageKind::NoteOff, 60, 0),
                    &parameters,
                )
                .unwrap();
            let mut released = [0.0; 512];
            prepared.render(&mut released, 256, &parameters).unwrap();
            energy(&released)
        };
        assert_eq!(release_energy(0.0), 0.0);
        assert!(release_energy(100.0) > 0.0);
    }
}
