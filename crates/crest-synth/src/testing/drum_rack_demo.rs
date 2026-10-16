//! Drum Rack listening fixture. SoundFont percussion is rendered to ordinary
//! WAV assets before startup; the scene itself plays only the Sample renderer.
use super::full_instrument_effect_demo::{
    failure, Audition, DemoBar, FullDemoError, FullDemoPlan, ParameterEdit,
};
use super::patch_control_navigation::next_patch_control_action;
use crate::adapter::drum_rack_capability::{
    pad_parameter_id, DRUM_RACK_CAPABILITY_ID, DRUM_RACK_FIRST_NOTE, DRUM_RACK_PADS,
    DRUM_RACK_PAD_PARAMETER_ID,
};
use crate::adapter::sample_capability::SAMPLE_ASSET_PARAMETER_ID;
use crate::control::{
    AppEvent, AppState, Direction, InteractionMode, PatchControlId, SavedSession, TopLevelContext,
};
use crate::kernel::{MidiChannel, PatchId};
use crate::mixer::{global_parameters::GlobalParameters, patch_output::PatchOutput};
use crate::synth::{
    AssetKind, AssetReference, CapabilityId, DescriptorDefaultConfigFactory,
    EffectCapabilityRegistry, ParameterId, ParameterValue, Patch,
};
use std::path::Path;

pub fn sample_filename(pad: usize) -> String {
    format!("{:02} {}.wav", pad + 1, DRUM_RACK_PADS[pad].1)
}

/// Offline fixture preparation, using the existing bundled upstream renderer.
pub fn write_samples(root: &Path) -> Result<(), FullDemoError> {
    let asset =
        crate::adapter::production_instruments::production_soundfont_asset().map_err(failure)?;
    let font = asset.prepared_bank().upstream.clone();
    if !font
        .get_presets()
        .iter()
        .any(|preset| preset.get_bank_number() == 128 && preset.get_patch_number() == 0)
    {
        return Err(failure("the bundled bank has no standard percussion kit"));
    }
    // Explicit source articulations, not a runtime fallback or a new mapping.
    // The final pad keeps the requested name "Right" and uses a ride cymbal.
    let source_notes = [
        36, 37, 38, 38, 40, 41, 42, 45, 44, 48, 46, 37, 39, 49, 53, 51,
    ];
    for (pad, source_note) in source_notes.into_iter().enumerate() {
        let mut settings = rustysynth::SynthesizerSettings::new(48_000);
        settings.enable_reverb_and_chorus = false;
        let mut synth = rustysynth::Synthesizer::new(&font, &settings).map_err(failure)?;
        synth.process_midi_message(9, 0xC0, 0, 0);
        let mut left = vec![0.0; 72_000];
        let mut right = vec![0.0; left.len()];
        synth.note_on(9, source_note, 104);
        if pad == 3 {
            // Four close snare hits make the requested roll sample.
            for start in [0, 3_360, 6_720, 10_080] {
                if start != 0 {
                    synth.note_on(9, source_note, 88 + (start / 3_360) as i32 * 4);
                }
                let end = if start == 10_080 {
                    left.len()
                } else {
                    start + 3_360
                };
                synth.render(&mut left[start..end], &mut right[start..end]);
            }
        } else {
            synth.render(&mut left, &mut right);
        }
        let mut mono = left
            .iter()
            .zip(&right)
            .map(|(l, r)| (l + r) * 0.5)
            .collect::<Vec<_>>();
        if mono.iter().any(|sample| !sample.is_finite()) {
            return Err(failure(format!(
                "non-finite percussion sample for pad {pad}"
            )));
        }
        let peak = mono
            .iter()
            .map(|sample| sample.abs())
            .fold(0.0_f32, f32::max);
        if peak < 0.0001 {
            return Err(failure(format!("silent percussion sample for pad {pad}")));
        }
        let frames = mono.len();
        for (frame, sample) in mono.iter_mut().enumerate() {
            // Normalize fixture levels and taper the end of the finite export.
            *sample *= 0.8 / peak * ((frames - frame) as f32 / 240.0).min(1.0);
        }
        let mut writer = hound::WavWriter::create(
            root.join(sample_filename(pad)),
            hound::WavSpec {
                channels: 1,
                sample_rate: 48_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .map_err(failure)?;
        for sample in mono {
            writer
                .write_sample((sample * i16::MAX as f32).round() as i16)
                .map_err(failure)?;
        }
        writer.finalize().map_err(failure)?;
    }
    Ok(())
}

pub(crate) fn build_plan(
    factory: &DescriptorDefaultConfigFactory,
    effects: &EffectCapabilityRegistry,
) -> Result<FullDemoPlan, FullDemoError> {
    let id = CapabilityId::new(DRUM_RACK_CAPABILITY_ID).map_err(failure)?;
    let descriptor = factory
        .registry()
        .descriptor(&id)
        .ok_or_else(|| failure("Drum Rack capability is not installed"))?;
    let mut instrument = factory.create(&id).map_err(failure)?;
    for pad in 0..DRUM_RACK_PADS.len() {
        instrument = factory
            .registry()
            .replace_asset(
                &instrument,
                &pad_parameter_id(pad, SAMPLE_ASSET_PARAMETER_ID),
                AssetReference::new(AssetKind::Sample, sample_filename(pad)).map_err(failure)?,
            )
            .map_err(failure)?;
    }
    let patch = Patch::new(
        PatchId::new(1).map_err(failure)?,
        "Drum Rack Demo".into(),
        instrument,
        MidiChannel::new(0).map_err(failure)?,
        PatchOutput::default(),
    );
    let mut state = AppState::new_with_effects(
        factory.registry().clone(),
        effects.clone(),
        GlobalParameters::new(-12.0).map_err(failure)?,
    );
    state
        .apply(AppEvent::InstallPatches(vec![patch]))
        .map_err(failure)?;
    state
        .apply(AppEvent::SelectContext(TopLevelContext::Patch))
        .map_err(failure)?;
    while let Some(action) =
        next_patch_control_action(&state, &PatchControlId::VoiceLimit).map_err(failure)?
    {
        state.apply_semantic_action(action).map_err(failure)?;
    }
    state
        .apply(AppEvent::SetInteractionMode(InteractionMode::Adjust))
        .map_err(failure)?;
    while state.patches()[0].voice_limit().value() != 8 {
        let direction = if state.patches()[0].voice_limit().value() > 8 {
            Direction::Down
        } else {
            Direction::Up
        };
        state.apply(AppEvent::Adjust(direction)).map_err(failure)?;
    }
    let session = SavedSession::capture(&state);
    let selector = descriptor
        .parameter(&ParameterId::new(DRUM_RACK_PAD_PARAMETER_ID).map_err(failure)?)
        .ok_or_else(|| failure("Drum Rack has no pad selector"))?;
    let bar = |pad: usize, label: String, notes| DemoBar {
        label,
        session: session.clone(),
        control: PatchControlId::Capability(selector.id().clone()),
        edit: Some(ParameterEdit {
            spec: selector.clone(),
            value: ParameterValue::Choice(format!("pad-{pad}")),
        }),
        prerequisites: Vec::new(),
        requires_preparation: false,
        notes: Some(notes),
    };
    let mut auditions = Vec::new();
    for bank in 0..2 {
        let bars = (bank * 8..bank * 8 + 8)
            .map(|pad| {
                let mut notes: [Vec<(u8, u8)>; 8] = Default::default();
                notes[0].push((DRUM_RACK_FIRST_NOTE + pad as u8, 100));
                notes[4].push((DRUM_RACK_FIRST_NOTE + pad as u8, 76));
                bar(
                    pad,
                    format!("{} · {}", DRUM_RACK_PADS[pad].0, DRUM_RACK_PADS[pad].1),
                    notes,
                )
            })
            .collect();
        auditions.push(Audition {
            label: format!("Pad audition {}–{}", bank * 8 + 1, bank * 8 + 8),
            bars,
        });
    }
    let bars = (0..8)
        .map(|index| {
            let mut notes: [Vec<(u8, u8)>; 8] = Default::default();
            for (step, hits) in notes.iter_mut().enumerate() {
                hits.push((42, if step % 2 == 0 { 70 } else { 48 }));
            }
            for step in [0, 3, 4] {
                notes[step].push((36, if step == 3 { 70 } else { 106 }));
            }
            for step in [2, 6] {
                notes[step].push((38, 96));
            }
            if index % 2 == 1 {
                notes[6].push((48, 72));
                notes[7] = vec![(46, 76)];
            }
            if index == 0 || index == 4 {
                notes[0].push((49, 80));
            }
            if index == 3 || index == 7 {
                notes[5] = vec![(45, 92)];
                notes[6] = vec![(43, 96), (38, 84)];
                notes[7] = vec![(41, 100)];
            }
            let pad = [0, 2, 6, 9, 13, 12, 10, 5][index];
            bar(pad, "Kick, snare, hats, claps and tom fills".into(), notes)
        })
        .collect();
    auditions.push(Audition {
        label: "Layered groove".into(),
        bars,
    });
    Ok(FullDemoPlan {
        auditions, instruments: 1, effects: 0, skipped: Vec::new(),
        asset_notes: vec!["WAVs rendered offline from bundled HiDef standard percussion; all playback uses Drum Rack. D#2 retains the name Right and auditions a ride cymbal.".into()],
        title: "Drum Rack demo", max_active_notes: 8,
        introduction: Some("Sixteen loaded pads, one bar per pad, then eight bars of layered drums. 120 BPM, 4/4; 48 seconds plus preparation. Close the window or Ctrl-C to stop."),
    })
}
