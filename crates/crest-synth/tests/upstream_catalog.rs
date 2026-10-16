//! Production asset import, persistence, and rendering for the embedded catalog.
use crest_synth::adapter::lock_free_audio_boundary::LockFreeAudioBoundary;
use crest_synth::adapter::lock_free_structural_graph_boundary::LockFreeStructuralGraphBoundary;
use crest_synth::adapter::{dx7_library, model_assets, sfz_library, upstream_audio};
use crest_synth::control::{AppEvent, AppState, SavedSession};
use crest_synth::kernel::midi_message::{MidiMessage, MidiMessageKind};
use crest_synth::kernel::{MidiChannel, PatchId};
use crest_synth::mixer::{
    global_parameters::GlobalParameters, mixer_state::MixerState, patch_output::PatchOutput,
};
use crest_synth::real_time::{
    AudioBoundary, AudioCommand, AudioRenderer, ControlAudioBoundary, GraphRevision,
    ParameterSnapshot, StructuralGraphBoundary,
};
use crest_synth::synth::effect_slot_id::EffectSlotIndex;
use crest_synth::synth::*;
use std::path::PathBuf;

struct Files {
    source: PathBuf,
    imported: Vec<PathBuf>,
}
impl Drop for Files {
    fn drop(&mut self) {
        for path in &self.imported {
            let _ = std::fs::remove_file(path);
        }
        let _ = std::fs::remove_dir_all(&self.source);
    }
}
fn defaults(descriptor: &CapabilityDescriptor) -> Vec<ParameterAssignment> {
    descriptor
        .parameters()
        .filter_map(|p| match p.default_value() {
            ParameterDefault::Value(value) => {
                Some(ParameterAssignment::new(p.id().clone(), value.clone()))
            }
            _ => None,
        })
        .collect()
}
fn sysex(data: &[u8; 155]) -> Vec<u8> {
    let mut bytes = vec![0xf0, 0x43, 0, 0, 1, 27];
    bytes.extend(data);
    bytes.push(((128 - (data.iter().map(|&v| u32::from(v)).sum::<u32>() & 127)) & 127) as u8);
    bytes.push(0xf7);
    bytes
}

#[test]
fn imported_sfz_sysex_models_and_ir_survive_source_removal_and_session_restore() {
    let name = format!(
        "crest-catalog-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let relative = format!("Music/Crest Synth/{name}");
    let source = PathBuf::from(std::env::var_os("HOME").unwrap()).join(&relative);
    std::fs::create_dir_all(&source).unwrap();
    let mut files = Files {
        source,
        imported: Vec::new(),
    };
    let external =
        |extension: &str| AssetFileId::new(format!("@home/{relative}/{name}.{extension}")).unwrap();
    std::fs::write(
        files.source.join("tone.wav"),
        include_bytes!("../assets/sample-test.wav"),
    )
    .unwrap();
    std::fs::write(
        files.source.join(format!("{name}.sfz")),
        "<region> sample=tone.wav key=60 ampeg_release=0.2\n",
    )
    .unwrap();
    let mut voice = dx7_library::Dx7Library::bundled().presets[0].data;
    let mut bank = sysex(&voice);
    voice[134] = (voice[134] + 1) % 32;
    bank.extend(sysex(&voice));
    std::fs::write(files.source.join(format!("{name}.syx")), bank).unwrap();
    std::fs::write(
        files.source.join(format!("{name}.nam")),
        include_bytes!("../vendor/audio/nam/example_models/lstm.nam"),
    )
    .unwrap();
    std::fs::write(
        files.source.join(format!("{name}.wav")),
        include_bytes!("../assets/sample-test.wav"),
    )
    .unwrap();

    let sfz = sfz_library::import(&external("sfz")).unwrap();
    files.imported.push(
        model_assets::browser(AssetKind::Sfz)
            .unwrap()
            .resolve(sfz.as_str())
            .unwrap(),
    );
    let (dx7, dx7_descriptor) = dx7_library::import(&external("syx")).unwrap();
    files.imported.push(
        dx7_library::browser()
            .unwrap()
            .resolve(dx7.as_str())
            .unwrap(),
    );
    assert_eq!(
        dx7_descriptor
            .parameter(&ParameterId::new(dx7_library::PRESET).unwrap())
            .unwrap()
            .choices()
            .len(),
        2
    );
    let nam = model_assets::import(AssetKind::NeuralModel, &external("nam")).unwrap();
    files.imported.push(
        model_assets::browser(AssetKind::NeuralModel)
            .unwrap()
            .resolve(nam.as_str())
            .unwrap(),
    );
    let ir = model_assets::import(AssetKind::ImpulseResponse, &external("wav")).unwrap();
    files.imported.push(
        model_assets::browser(AssetKind::ImpulseResponse)
            .unwrap()
            .resolve(ir.as_str())
            .unwrap(),
    );
    std::fs::remove_dir_all(&files.source).unwrap();

    let ports = upstream_audio::instrument_ports().unwrap();
    let effects = upstream_audio::effect_ports().unwrap();
    let registry = CapabilityRegistry::new(
        ports
            .iter()
            .map(InstrumentCapabilityProvider::descriptor)
            .collect(),
    )
    .unwrap()
    .with_asset_descriptor(dx7_descriptor.clone())
    .unwrap();
    let effect_registry = EffectCapabilityRegistry::new(
        effects
            .iter()
            .map(EffectCapabilityProvider::descriptor)
            .collect(),
    )
    .unwrap();
    let mut patches = Vec::new();
    for (ordinal, capability, parameter, kind, asset) in [
        (
            0,
            sfz_library::CAPABILITY,
            sfz_library::FILE,
            AssetKind::Sfz,
            sfz,
        ),
        (
            1,
            dx7_library::CAPABILITY,
            dx7_library::FILE,
            AssetKind::SysEx,
            dx7,
        ),
    ] {
        let descriptor = if ordinal == 1 {
            dx7_descriptor.clone()
        } else {
            registry
                .descriptor(&CapabilityId::new(capability).unwrap())
                .unwrap()
                .clone()
        };
        let mut values = defaults(&descriptor);
        if ordinal == 1 {
            values[0] = ParameterAssignment::new(
                ParameterId::new(dx7_library::PRESET).unwrap(),
                ParameterValue::Choice("message-1.voice-0".into()),
            );
        }
        let config = descriptor
            .create_config(
                &values,
                &[AssetAssignment::new(
                    ParameterId::new(parameter).unwrap(),
                    AssetReference::new(kind, asset.as_str()).unwrap(),
                )],
            )
            .unwrap();
        patches.push(Patch::new(
            PatchId::new(ordinal + 1).unwrap(),
            format!("Imported {ordinal}"),
            config,
            MidiChannel::new(ordinal as u8).unwrap(),
            PatchOutput::default(),
        ));
    }
    for (index, capability, parameter, kind, asset) in [
        (
            0,
            "effect.nam.model",
            model_assets::NAM_FILE,
            AssetKind::NeuralModel,
            nam,
        ),
        (
            1,
            "effect.fft.convolver",
            model_assets::IR_FILE,
            AssetKind::ImpulseResponse,
            ir,
        ),
    ] {
        let descriptor = effect_registry
            .descriptor(&EffectCapabilityId::new(capability).unwrap())
            .unwrap();
        let slot = EffectSlotIndex::new(index).unwrap();
        let config = descriptor.default_config(slot.instance_identity()).unwrap();
        let config = effect_registry
            .replace_asset(
                &config,
                &ParameterId::new(parameter).unwrap(),
                AssetReference::new(kind, asset.as_str()).unwrap(),
            )
            .unwrap();
        patches[0] = patches[0].clone().with_effect_slot(slot, config);
    }
    let mut state = AppState::new_with_effects(
        registry.clone(),
        effect_registry.clone(),
        GlobalParameters::new(0.0).unwrap(),
    );
    state.apply(AppEvent::InstallPatches(patches)).unwrap();
    let saved = SavedSession::capture(&state);
    let json = saved.to_json().unwrap();
    assert!(!json.contains("@home/"));
    let saved = SavedSession::from_json(&json, &registry).unwrap();
    let instruments: Vec<Box<dyn InstrumentPreparer>> = ports
        .into_iter()
        .map(|p| Box::new(p) as Box<dyn InstrumentPreparer>)
        .collect();
    let effect_preparers: Vec<Box<dyn EffectPreparer>> = effects
        .into_iter()
        .map(|p| Box::new(p) as Box<dyn EffectPreparer>)
        .collect();
    let restored = saved
        .prepare_restore(
            registry,
            effect_registry,
            &instruments,
            &effect_preparers,
            GraphRevision::INITIAL,
            48_000.0,
            256,
        )
        .unwrap();
    assert_eq!(SavedSession::capture(restored.state()), saved);
    let (_, graph) = restored.into_replacement();
    let (mut control, audio) =
        LockFreeAudioBoundary::new(32, graph.initial_parameters().clone()).into_handles();
    let (_, structural) = LockFreeStructuralGraphBoundary::new(
        2,
        2,
        crest_synth::real_time::GraphHandoffStatus::with_active(GraphRevision::INITIAL),
    )
    .unwrap()
    .into_handles();
    let mut renderer = AudioRenderer::new(audio, structural, graph);
    for index in 0..2 {
        control
            .push_command(AudioCommand::patch_midi(
                PatchId::new(index + 1).unwrap(),
                MidiMessage::try_new(
                    MidiChannel::new(index as u8).unwrap(),
                    MidiMessageKind::NoteOn,
                    60,
                    100,
                )
                .unwrap(),
            ))
            .unwrap();
    }
    let mut energy = [0.0_f64; 2];
    for _ in 0..128 {
        let mut output = [0.0; 512];
        renderer.render(&mut output);
        assert!(output.iter().all(|value| value.is_finite()));
        for (index, total) in energy.iter_mut().enumerate() {
            *total += renderer
                .active_patch_audio()
                .stem(index, PatchId::new(index as u32 + 1).unwrap())
                .unwrap()
                .samples()
                .iter()
                .map(|&v| f64::from(v).powi(2))
                .sum::<f64>();
        }
    }
    assert!(
        energy.iter().all(|v| *v > 1e-8),
        "imported stems: {energy:?}"
    );
}

#[test]
fn configured_voice_banks_play_more_than_the_retired_sixty_four_voice_ceiling() {
    let port = upstream_audio::instrument_ports()
        .unwrap()
        .into_iter()
        .find(|p| p.descriptor().id().as_str() == "instrument.daisy.analogbassdrum")
        .unwrap();
    let descriptor = port.descriptor();
    let config = port
        .create_config(&defaults(&descriptor), &descriptor.default_assets())
        .unwrap();
    let patch = Patch::new(
        PatchId::new(1).unwrap(),
        "Many voices".into(),
        config,
        MidiChannel::new(0).unwrap(),
        PatchOutput::default(),
    )
    .with_voice_limit(129)
    .unwrap();
    let registry = CapabilityRegistry::new(vec![descriptor]).unwrap();
    let snapshot = ParameterSnapshot::project_patches(
        1,
        GraphRevision::INITIAL,
        GlobalParameters::new(0.0).unwrap(),
        MixerState::default(),
        std::slice::from_ref(&patch),
        &registry,
    )
    .unwrap();
    let params = &snapshot.patches()[0];
    let mut instrument = port.prepare(&patch, 48_000.0, 256).unwrap();
    let note = MidiMessage::try_new(
        MidiChannel::new(0).unwrap(),
        MidiMessageKind::NoteOn,
        60,
        100,
    )
    .unwrap();
    for _ in 0..129 {
        instrument.dispatch(note, params).unwrap();
    }
    assert!(instrument.dispatch(note, params).is_err());
    let mut audio = [0.0; 512];
    instrument.render(&mut audio, 256, params).unwrap();
    assert!(audio.iter().all(|value| value.is_finite()));
    assert!(audio.iter().any(|value| value.abs() > 1e-5));
}
