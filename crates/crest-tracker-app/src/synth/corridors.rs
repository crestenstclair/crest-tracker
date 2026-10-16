//! Compose imported musical data with Synth-native, validated SoundFont patches.
use crate::persistence::Document;
use crest_synth::adapter::hidef_soundfont_capability::SOUNDFONT_PRESET_PARAMETER_ID;
use crest_synth::adapter::production_instruments::production_soundfont_capability;
use crest_synth::control::{AppEvent, AppState, SavedSession};
use crest_synth::kernel::{MidiChannel, PatchId};
use crest_synth::mixer::{
    global_parameters::GlobalParameters, mixer_track_id::MixerTrackId, patch_output::PatchOutput,
};
use crest_synth::synth::{
    InstrumentCapabilityProvider, ParameterAssignment, ParameterId, ParameterValue, Patch,
    SoundFontPresetId,
};
use std::collections::BTreeMap;

pub(crate) fn document() -> Result<Document, String> {
    build().map_err(|error| format!("Cannot prepare Corridors of Time: {error}"))
}

fn build() -> Result<Document, Box<dyn std::error::Error>> {
    let imported = crate::corridors::import()?;
    let capability = production_soundfont_capability()?;
    let mut mappings = BTreeMap::new();
    let mut patches = Vec::new();
    for (index, part) in imported.parts.iter().enumerate() {
        let id = PatchId::new(index as u32 + 1)?;
        let preset = SoundFontPresetId::new(part.bank, part.program)?;
        let config = capability.create_config(
            &[ParameterAssignment::new(
                ParameterId::new(SOUNDFONT_PRESET_PARAMETER_ID)?,
                ParameterValue::Choice(preset.choice_id()),
            )],
            &capability.descriptor().default_assets(),
        )?;
        patches.push(Patch::new(
            id,
            part.name.clone(),
            config,
            MidiChannel::new(part.channel)?,
            PatchOutput::to_track(MixerTrackId::ALL[usize::from(part.channel)]),
        ));
        mappings.insert(part.instrument, id.value());
    }
    let mut state = AppState::new(capability.registry()?, GlobalParameters::new(0.0)?);
    state.apply(AppEvent::InstallPatches(patches))?;
    let document = Document::new(
        imported.song,
        serde_json::to_value(SavedSession::capture(&state))?,
        mappings,
    );
    document.validate()?;
    Ok(document)
}
