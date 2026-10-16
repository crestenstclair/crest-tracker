//! Complete upstream SoundFont synthesis with shared immutable SF2 ownership.
use super::upstream_audio::VoiceProcessor;
use crate::synth::{InstrumentPreparationError, SoundFontPresetId};
use rustysynth::{SoundFont, Synthesizer, SynthesizerSettings};
use std::sync::Arc;
pub(crate) struct RustyVoice {
    synth: Synthesizer,
    preset: SoundFontPresetId,
    left: Vec<f32>,
    right: Vec<f32>,
}
impl RustyVoice {
    pub fn new(
        font: &Arc<SoundFont>,
        preset: SoundFontPresetId,
        rate: f32,
        frames: usize,
    ) -> Result<Self, InstrumentPreparationError> {
        // A host note can layer several SF2 regions. Validate the exact bank
        // and program before dispatch, so RustySynth's GM fallback is unused.
        let entry = font
            .get_presets()
            .iter()
            .find(|p| {
                p.get_bank_number() == i32::from(preset.bank())
                    && p.get_patch_number() == i32::from(preset.program())
            })
            .ok_or(InstrumentPreparationError::AssetParseFailed)?;
        // Capacity follows the largest set of regions one MIDI note can
        // actually trigger, including overlapping velocity layers.
        let mut counts = vec![0usize; 128 * 128];
        for preset_region in entry.get_regions() {
            let instrument = font
                .get_instruments()
                .get(preset_region.get_instrument_id())
                .ok_or(InstrumentPreparationError::AssetParseFailed)?;
            for region in instrument.get_regions() {
                let key_start = preset_region
                    .get_key_range_start()
                    .max(region.get_key_range_start())
                    .max(0);
                let key_end = preset_region
                    .get_key_range_end()
                    .min(region.get_key_range_end())
                    .min(127);
                let velocity_start = preset_region
                    .get_velocity_range_start()
                    .max(region.get_velocity_range_start())
                    .max(1);
                let velocity_end = preset_region
                    .get_velocity_range_end()
                    .min(region.get_velocity_range_end())
                    .min(127);
                for key in key_start..=key_end {
                    for velocity in velocity_start..=velocity_end {
                        counts[(key * 128 + velocity) as usize] += 1;
                    }
                }
            }
        }
        let layers = counts.into_iter().max().unwrap_or(0);
        let mut settings = SynthesizerSettings::new(rate as i32);
        settings.maximum_polyphony = layers.max(1);
        settings.enable_reverb_and_chorus = false;
        let synth = Synthesizer::new(font, &settings)
            .map_err(|_| InstrumentPreparationError::AssetParseFailed)?;
        let mut voice = Self {
            synth,
            preset,
            left: vec![0.; frames],
            right: vec![0.; frames],
        };
        voice.reset();
        super::soundfont_voice_engine::upstream_engine_created();
        Ok(voice)
    }
}
impl VoiceProcessor for RustyVoice {
    fn latency(&self) -> usize {
        0
    }
    fn set(&mut self, values: &[f32]) -> bool {
        values.is_empty()
    }
    fn reset(&mut self) {
        self.synth.reset();
        self.synth
            .process_midi_message(0, 0xb0, 0, i32::from(self.preset.bank()));
        self.synth
            .process_midi_message(0, 0xc0, i32::from(self.preset.program()), 0);
    }
    fn note(&mut self, status: i32, a: u8, b: u8) {
        self.synth
            .process_midi_message(0, status, i32::from(a), i32::from(b));
    }
    fn process(&mut self, stereo: &mut [f32], frames: usize) -> bool {
        if frames > self.left.len() || stereo.len() != frames * 2 {
            return false;
        }
        self.synth
            .render(&mut self.left[..frames], &mut self.right[..frames]);
        for (i, output) in stereo.chunks_exact_mut(2).enumerate() {
            output[0] = self.left[i];
            output[1] = self.right[i];
        }
        stereo.iter().all(|s| s.is_finite())
    }
}

impl Drop for RustyVoice {
    fn drop(&mut self) {
        super::soundfont_voice_engine::upstream_engine_destroyed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiet_soundfont_notes_keep_their_signal_and_velocity_dynamics() {
        let asset = crate::adapter::production_instruments::production_soundfont_asset().unwrap();
        let font = asset.prepared_bank().upstream.clone();
        for (program, pitch, quiet_velocity) in [(42, 56, 2), (114, 72, 5), (10, 72, 6)] {
            let mut voice = RustyVoice::new(
                &font,
                SoundFontPresetId::new(0, program).unwrap(),
                48_000.0,
                256,
            )
            .unwrap();
            let mut energy = |velocity| {
                voice.reset();
                voice.note(0x90, pitch, velocity);
                let mut energy = 0.0_f64;
                let mut block = [0.0_f32; 512];
                for _ in 0..128 {
                    assert!(voice.process(&mut block, 256));
                    energy += block
                        .iter()
                        .map(|sample| f64::from(*sample).powi(2))
                        .sum::<f64>();
                }
                energy
            };
            assert_eq!(energy(0), 0.0, "velocity zero must not start a note");
            let quiet = energy(quiet_velocity);
            assert!(
                quiet > 1e-12,
                "program {program}, velocity {quiet_velocity} was silenced"
            );
            assert!(energy(100) > quiet * 100.0, "MIDI dynamics must survive");
            voice.reset();
            let mut block = [0.0; 512];
            assert!(voice.process(&mut block, 256));
            assert!(block.iter().all(|sample| *sample == 0.0));
        }
    }
}
