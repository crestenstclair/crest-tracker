use crate::synth::SoundFontPresetId;
use rustysynth::{LoopMode, SoundFont};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

#[cfg(test)]
const MIDI_BEND_CENTER: i32 = 8_192;
#[cfg(test)]
const PITCH_BEND_RANGE_SEMITONES: f64 = 2.0;

static ENGINES_CREATED: AtomicU64 = AtomicU64::new(0);
static ENGINES_DESTROYED: AtomicU64 = AtomicU64::new(0);
static ENGINES_ACTIVE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SoundFontEngineLifecycleCounts {
    pub created: u64,
    pub destroyed: u64,
    pub active: u64,
}

pub fn soundfont_engine_lifecycle_counts() -> SoundFontEngineLifecycleCounts {
    SoundFontEngineLifecycleCounts {
        created: ENGINES_CREATED.load(Ordering::Relaxed),
        destroyed: ENGINES_DESTROYED.load(Ordering::Relaxed),
        active: ENGINES_ACTIVE.load(Ordering::Relaxed),
    }
}

pub(crate) fn upstream_engine_created() {
    ENGINES_CREATED.fetch_add(1, Ordering::Relaxed);
    ENGINES_ACTIVE.fetch_add(1, Ordering::Relaxed);
}
pub(crate) fn upstream_engine_destroyed() {
    ENGINES_DESTROYED.fetch_add(1, Ordering::Relaxed);
    ENGINES_ACTIVE.fetch_sub(1, Ordering::Relaxed);
}

/// A typed failure while converting parser-owned SF2 data to callback-safe
/// numeric storage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PreparedSoundFontBankError {
    SampleStorage,
    InvalidPresetAddress { source_ordinal: usize },
    InvalidInstrumentReference { source_ordinal: usize },
    InvalidSampleReference { source_ordinal: usize },
    InvalidRegion { source_ordinal: usize },
}

/// Shared upstream SoundFont plus the legacy renderer projection used by
/// compatibility tests. Parsing and ownership retirement occur off callback.
pub(crate) struct PreparedSoundFontBank {
    pub(crate) upstream: Arc<SoundFont>,
    #[cfg(test)]
    wave_data: Arc<[i16]>,
    #[cfg(test)]
    presets: Vec<PreparedPreset>,
}

impl PreparedSoundFontBank {
    pub(crate) fn from_sound_font(
        sound_font: Arc<SoundFont>,
    ) -> Result<(Self, Vec<usize>), PreparedSoundFontBankError> {
        #[cfg(test)]
        let wave_data = Arc::<[i16]>::from(sound_font.get_wave_data());
        let mut presets = Vec::new();
        presets
            .try_reserve_exact(sound_font.get_presets().len())
            .map_err(|_| PreparedSoundFontBankError::SampleStorage)?;
        let mut playable_source_ordinals = Vec::new();
        playable_source_ordinals
            .try_reserve_exact(sound_font.get_presets().len())
            .map_err(|_| PreparedSoundFontBankError::SampleStorage)?;
        let mut expanded_regions = 0usize;
        for (source_ordinal, preset) in sound_font.get_presets().iter().enumerate() {
            let mut regions = Vec::new();
            for preset_region in preset.get_regions() {
                let instrument = sound_font
                    .get_instruments()
                    .get(preset_region.get_instrument_id())
                    .ok_or(PreparedSoundFontBankError::InvalidInstrumentReference {
                        source_ordinal,
                    })?;
                for instrument_region in instrument.get_regions() {
                    expanded_regions += 1;
                    if expanded_regions > 262_144 || regions.len() >= 4_096 {
                        return Err(PreparedSoundFontBankError::InvalidRegion { source_ordinal });
                    }
                    let key_start = preset_region
                        .get_key_range_start()
                        .max(instrument_region.get_key_range_start());
                    let key_end = preset_region
                        .get_key_range_end()
                        .min(instrument_region.get_key_range_end());
                    let velocity_start = preset_region
                        .get_velocity_range_start()
                        .max(instrument_region.get_velocity_range_start());
                    let velocity_end = preset_region
                        .get_velocity_range_end()
                        .min(instrument_region.get_velocity_range_end());
                    if key_start > key_end || velocity_start > velocity_end {
                        continue;
                    }

                    let sample = sound_font
                        .get_sample_headers()
                        .get(instrument_region.get_sample_id())
                        .ok_or(PreparedSoundFontBankError::InvalidSampleReference {
                            source_ordinal,
                        })?;
                    let sample_start = usize::try_from(instrument_region.get_sample_start())
                        .map_err(|_| PreparedSoundFontBankError::InvalidRegion {
                            source_ordinal,
                        })?;
                    let sample_end =
                        usize::try_from(instrument_region.get_sample_end()).map_err(|_| {
                            PreparedSoundFontBankError::InvalidRegion { source_ordinal }
                        })?;
                    let loop_start = usize::try_from(instrument_region.get_sample_start_loop())
                        .map_err(|_| PreparedSoundFontBankError::InvalidRegion {
                            source_ordinal,
                        })?;
                    let loop_end = usize::try_from(instrument_region.get_sample_end_loop())
                        .map_err(|_| PreparedSoundFontBankError::InvalidRegion {
                            source_ordinal,
                        })?;
                    if sample_start >= sample_end
                        || sample_end >= sound_font.get_wave_data().len()
                        || sample.get_sample_rate() <= 0
                    {
                        return Err(PreparedSoundFontBankError::InvalidRegion { source_ordinal });
                    }
                    let loop_mode = match instrument_region.get_sample_modes() {
                        LoopMode::NoLoop => PreparedLoopMode::NoLoop,
                        LoopMode::Continuous => PreparedLoopMode::Continuous,
                        LoopMode::LoopUntilNoteOff => PreparedLoopMode::UntilNoteOff,
                    };
                    if loop_mode != PreparedLoopMode::NoLoop
                        && (loop_start >= loop_end || loop_end > sample_end)
                    {
                        return Err(PreparedSoundFontBankError::InvalidRegion { source_ordinal });
                    }

                    let attenuation_db = (preset_region.get_initial_attenuation()
                        + instrument_region.get_initial_attenuation())
                    .max(0.0);
                    let amplitude = 10.0_f32.powf(-attenuation_db / 20.0);
                    let pan = ((preset_region.get_pan() + instrument_region.get_pan()) / 50.0)
                        .clamp(-1.0, 1.0);
                    let left_gain = ((1.0 - pan) * 0.5).sqrt();
                    let right_gain = ((1.0 + pan) * 0.5).sqrt();
                    regions.push(PreparedSampleRegion {
                        key_start: key_start as u8,
                        key_end: key_end as u8,
                        velocity_start: velocity_start as u8,
                        velocity_end: velocity_end as u8,
                        sample_start,
                        sample_end,
                        loop_start,
                        loop_end,
                        loop_mode,
                        sample_rate: sample.get_sample_rate() as f64,
                        root_key: instrument_region.get_root_key() as f64,
                        coarse_tune: preset_region.get_coarse_tune()
                            + instrument_region.get_coarse_tune(),
                        fine_tune: preset_region.get_fine_tune()
                            + instrument_region.get_fine_tune(),
                        scale_tuning: preset_region.get_scale_tuning()
                            + instrument_region.get_scale_tuning(),
                        exclusive_class: instrument_region.get_exclusive_class(),
                        amplitude,
                        left_gain,
                        right_gain,
                    });
                }
            }
            if regions.is_empty() {
                continue;
            }
            let bank = u16::try_from(preset.get_bank_number())
                .map_err(|_| PreparedSoundFontBankError::InvalidPresetAddress { source_ordinal })?;
            let program = u8::try_from(preset.get_patch_number())
                .map_err(|_| PreparedSoundFontBankError::InvalidPresetAddress { source_ordinal })?;
            let id = SoundFontPresetId::new(bank, program)
                .map_err(|_| PreparedSoundFontBankError::InvalidPresetAddress { source_ordinal })?;
            playable_source_ordinals.push(source_ordinal);
            if presets
                .iter()
                .any(|candidate: &PreparedPreset| candidate.id == id)
            {
                continue;
            }
            presets.push(PreparedPreset {
                id,
                #[cfg(test)]
                regions,
            });
        }
        presets.sort_by_key(|preset| preset.id);
        Ok((
            Self {
                upstream: sound_font,
                #[cfg(test)]
                wave_data,
                #[cfg(test)]
                presets,
            },
            playable_source_ordinals,
        ))
    }

    #[cfg(test)]
    pub(crate) fn has_preset(&self, id: SoundFontPresetId) -> bool {
        self.preset_index(id).is_some()
    }

    #[cfg(test)]
    fn preset_index(&self, id: SoundFontPresetId) -> Option<usize> {
        self.presets
            .binary_search_by_key(&id, |preset| preset.id)
            .ok()
    }
}

struct PreparedPreset {
    id: SoundFontPresetId,
    #[cfg(test)]
    regions: Vec<PreparedSampleRegion>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PreparedLoopMode {
    NoLoop,
    Continuous,
    UntilNoteOff,
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(not(test), allow(dead_code))]
struct PreparedSampleRegion {
    key_start: u8,
    key_end: u8,
    velocity_start: u8,
    velocity_end: u8,
    sample_start: usize,
    sample_end: usize,
    loop_start: usize,
    loop_end: usize,
    loop_mode: PreparedLoopMode,
    sample_rate: f64,
    root_key: f64,
    coarse_tune: i32,
    fine_tune: i32,
    scale_tuning: i32,
    exclusive_class: i32,
    amplitude: f32,
    left_gain: f32,
    right_gain: f32,
}

#[cfg(test)]
impl PreparedSampleRegion {
    fn contains(self, note: u8, velocity: u8) -> bool {
        (self.key_start..=self.key_end).contains(&note)
            && (self.velocity_start..=self.velocity_end).contains(&velocity)
    }
}

// Retained only as a legacy reference for existing compatibility witnesses.
#[cfg(test)]
pub(crate) use legacy::SoundFontVoiceEngine;
#[cfg(test)]
mod legacy {
    use super::*;
    use crate::kernel::midi_message::{MidiMessage, MidiMessageKind};
    use crate::synth::prepared_instrument::PreparedInstrumentError;
    use crate::synth::voice_envelope::VoiceEnvelope;
    use crate::synth::voice_envelope_state::VoiceEnvelopeState;
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct SoundFontSampleVoice {
        note: Option<u8>,
        region: Option<PreparedSampleRegion>,
        position: f64,
        velocity: f32,
        age: u64,
        envelope: VoiceEnvelopeState,
    }

    impl SoundFontSampleVoice {
        const IDLE: Self = Self {
            note: None,
            region: None,
            position: 0.0,
            velocity: 0.0,
            age: 0,
            envelope: VoiceEnvelopeState::IDLE,
        };

        fn clear(&mut self) {
            *self = Self::IDLE;
        }
    }

    /// One Patch-local engine instance owning all prepared SoundFont note voices.
    pub(crate) struct SoundFontVoiceEngine<const VOICES: usize> {
        bank: Arc<PreparedSoundFontBank>,
        voices: [SoundFontSampleVoice; VOICES],
        sample_rate: f32,
        max_frames: usize,
        preset_id: SoundFontPresetId,
        expression: f32,
        pressure: f32,
        pitch_bend_semitones: f64,
        next_age: u64,
    }

    impl<const VOICES: usize> SoundFontVoiceEngine<VOICES> {
        pub(crate) fn new(
            bank: Arc<PreparedSoundFontBank>,
            sample_rate: f32,
            max_frames: usize,
            preset_id: SoundFontPresetId,
        ) -> Result<Self, ()> {
            if VOICES == 0
                || !sample_rate.is_finite()
                || sample_rate <= 0.0
                || max_frames == 0
                || !bank.has_preset(preset_id)
            {
                return Err(());
            }
            ENGINES_CREATED.fetch_add(1, Ordering::Relaxed);
            ENGINES_ACTIVE.fetch_add(1, Ordering::Relaxed);
            Ok(Self {
                bank,
                voices: [SoundFontSampleVoice::IDLE; VOICES],
                sample_rate,
                max_frames,
                preset_id,
                expression: 1.0,
                pressure: 1.0,
                pitch_bend_semitones: 0.0,
                next_age: 1,
            })
        }

        pub(crate) fn dispatch(
            &mut self,
            message: MidiMessage,
            envelope: VoiceEnvelope,
        ) -> Result<(), PreparedInstrumentError> {
            match message.kind() {
                MidiMessageKind::NoteOn if message.data2() > 0 => {
                    self.note_on(message.data1(), message.data2(), envelope)
                }
                MidiMessageKind::NoteOn | MidiMessageKind::NoteOff => {
                    self.note_off(message.data1(), envelope.release_milliseconds());
                    Ok(())
                }
                MidiMessageKind::ControlChange => {
                    match message.data1() {
                        0 | 32 => return Err(PreparedInstrumentError::DispatchRejected),
                        7 | 11 => self.expression = f32::from(message.data2()) / 127.0,
                        _ => {}
                    }
                    Ok(())
                }
                MidiMessageKind::ProgramChange | MidiMessageKind::PolyPressure => {
                    Err(PreparedInstrumentError::DispatchRejected)
                }
                MidiMessageKind::ChannelPressure => {
                    self.pressure = f32::from(message.data1()) / 127.0;
                    Ok(())
                }
                MidiMessageKind::PitchBend => {
                    let bend = i32::from(message.data1()) | (i32::from(message.data2()) << 7);
                    self.pitch_bend_semitones = f64::from(bend - MIDI_BEND_CENTER)
                        * PITCH_BEND_RANGE_SEMITONES
                        / f64::from(MIDI_BEND_CENTER);
                    Ok(())
                }
                MidiMessageKind::AllNotesOff => {
                    self.all_notes_off();
                    Ok(())
                }
            }
        }

        fn note_on(
            &mut self,
            note: u8,
            velocity: u8,
            envelope: VoiceEnvelope,
        ) -> Result<(), PreparedInstrumentError> {
            let preset_index = self
                .bank
                .preset_index(self.preset_id)
                .ok_or(PreparedInstrumentError::DispatchRejected)?;
            let region_count = self.bank.presets[preset_index].regions.len();
            let mut started = false;
            for region_index in 0..region_count {
                let region = self.bank.presets[preset_index].regions[region_index];
                if !region.contains(note, velocity) {
                    continue;
                }
                self.start_voice(note, velocity, region, envelope)?;
                started = true;
            }
            if started {
                Ok(())
            } else {
                Err(PreparedInstrumentError::DispatchRejected)
            }
        }

        fn start_voice(
            &mut self,
            note: u8,
            velocity: u8,
            region: PreparedSampleRegion,
            envelope: VoiceEnvelope,
        ) -> Result<(), PreparedInstrumentError> {
            if region.exclusive_class != 0 {
                for voice in &mut self.voices {
                    if voice
                        .region
                        .is_some_and(|active| active.exclusive_class == region.exclusive_class)
                    {
                        voice.clear();
                    }
                }
            }
            let index = self
                .voices
                .iter()
                .position(|voice| voice.envelope.is_idle())
                .or_else(|| {
                    self.voices
                        .iter()
                        .enumerate()
                        .min_by_key(|(index, voice)| (voice.age, *index))
                        .map(|(index, _)| index)
                })
                .ok_or(PreparedInstrumentError::DispatchRejected)?;
            let voice = &mut self.voices[index];
            voice.note = Some(note);
            voice.region = Some(region);
            voice.position = region.sample_start as f64;
            voice.velocity = f32::from(velocity) / 127.0;
            voice.age = self.next_age;
            voice.envelope.note_on(envelope, self.sample_rate);
            self.next_age = self.next_age.saturating_add(1);
            Ok(())
        }

        fn note_off(&mut self, note: u8, release_milliseconds: f32) {
            for voice in &mut self.voices {
                if voice.note == Some(note) && !voice.envelope.is_idle() {
                    voice
                        .envelope
                        .note_off(release_milliseconds, self.sample_rate);
                }
            }
        }

        pub(crate) fn render(&mut self, output: &mut [f32], frame_count: usize) {
            let frame_count = frame_count.min(self.max_frames).min(output.len() / 2);
            let wave_data = &self.bank.wave_data;
            let expression = self.expression * self.pressure;
            for voice in &mut self.voices {
                let Some(region) = voice.region else {
                    continue;
                };
                let pitch_change = 0.01
                    * f64::from(region.scale_tuning)
                    * (f64::from(voice.note.unwrap_or_default()) + self.pitch_bend_semitones
                        - region.root_key)
                    + f64::from(region.coarse_tune)
                    + 0.01 * f64::from(region.fine_tune);
                let increment = region.sample_rate / f64::from(self.sample_rate)
                    * 2.0_f64.powf(pitch_change / 12.0);
                let voice_gain = voice.velocity * region.amplitude * expression;
                let output = &mut output[..frame_count * 2];
                if voice.envelope.stage() == crate::synth::VoiceEnvelopeStage::Sustain {
                    render_voice::<true>(
                        voice,
                        region,
                        wave_data,
                        output,
                        self.sample_rate,
                        increment,
                        voice_gain,
                    );
                } else {
                    render_voice::<false>(
                        voice,
                        region,
                        wave_data,
                        output,
                        self.sample_rate,
                        increment,
                        voice_gain,
                    );
                }
            }
        }

        pub(crate) fn all_notes_off(&mut self) {
            for voice in &mut self.voices {
                voice.clear();
            }
        }

        #[cfg(test)]
        pub(crate) fn active_note_voice_count(&self) -> usize {
            self.voices
                .iter()
                .filter(|voice| !voice.envelope.is_idle())
                .count()
        }

        #[cfg(test)]
        pub(crate) fn note_voice_counts(&self, note: u8) -> (usize, usize) {
            self.voices
                .iter()
                .filter(|voice| voice.note == Some(note))
                .fold((0, 0), |(active, releasing), voice| {
                    (
                        active + usize::from(!voice.envelope.is_idle()),
                        releasing + usize::from(voice.envelope.is_releasing()),
                    )
                })
        }
    }

    // Sustain is constant until a discrete note-off is dispatched. Specializing
    // that case removes envelope stage transitions from the sample loop while the
    // general path retains exact Attack/Decay/Release behavior.
    fn render_voice<const SUSTAIN: bool>(
        voice: &mut SoundFontSampleVoice,
        region: PreparedSampleRegion,
        wave_data: &[i16],
        output: &mut [f32],
        sample_rate: f32,
        increment: f64,
        voice_gain: f32,
    ) {
        let looping = match region.loop_mode {
            PreparedLoopMode::NoLoop => false,
            PreparedLoopMode::Continuous => true,
            PreparedLoopMode::UntilNoteOff => !voice.envelope.is_releasing(),
        };
        for frame in output.chunks_exact_mut(2) {
            if looping && voice.position >= region.loop_end as f64 {
                let loop_length = (region.loop_end - region.loop_start) as f64;
                voice.position = region.loop_start as f64
                    + (voice.position - region.loop_start as f64).rem_euclid(loop_length);
            }
            let index = voice.position.floor() as usize;
            if index >= region.sample_end {
                voice.clear();
                break;
            }
            let next = if looping && index + 1 >= region.loop_end {
                region.loop_start
            } else {
                index + 1
            };
            let (Some(first), Some(second)) = (wave_data.get(index), wave_data.get(next)) else {
                voice.clear();
                break;
            };
            let fraction = (voice.position - index as f64) as f32;
            let sample = (f32::from(*first) + (f32::from(*second) - f32::from(*first)) * fraction)
                / 32_768.0;
            let envelope_gain = if SUSTAIN {
                voice.envelope.level()
            } else {
                voice.envelope.next_gain(sample_rate)
            };
            let sample = bounded_sample(sample * voice_gain * envelope_gain);
            frame[0] += sample * region.left_gain;
            frame[1] += sample * region.right_gain;
            voice.position += increment;
            if !SUSTAIN && voice.envelope.is_idle() {
                voice.clear();
                break;
            }
        }
    }

    impl<const VOICES: usize> Drop for SoundFontVoiceEngine<VOICES> {
        fn drop(&mut self) {
            ENGINES_DESTROYED.fetch_add(1, Ordering::Relaxed);
            ENGINES_ACTIVE.fetch_sub(1, Ordering::Relaxed);
        }
    }

    fn bounded_sample(sample: f32) -> f32 {
        if sample.is_finite() {
            sample.clamp(-1.0, 1.0)
        } else {
            0.0
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn sustain_render_matches_general_envelope_path_at_fractional_loop_and_end_boundaries() {
            let wave_data = [0, 8_000, 16_000, -4_000, -12_000, 4_000, 20_000, 0];
            for loop_mode in [
                PreparedLoopMode::NoLoop,
                PreparedLoopMode::Continuous,
                PreparedLoopMode::UntilNoteOff,
            ] {
                let region = PreparedSampleRegion {
                    key_start: 0,
                    key_end: 127,
                    velocity_start: 0,
                    velocity_end: 127,
                    sample_start: 0,
                    sample_end: 8,
                    loop_start: 2,
                    loop_end: 6,
                    loop_mode,
                    sample_rate: 48_000.0,
                    root_key: 60.0,
                    coarse_tune: 0,
                    fine_tune: 0,
                    scale_tuning: 100,
                    exclusive_class: 0,
                    amplitude: 1.0,
                    left_gain: 0.75,
                    right_gain: 0.5,
                };
                for gain in [0.0, 0.5, 1.0] {
                    for increment in [0.5, 1.0, 2.75] {
                        for position in [0.0, 5.5, 7.0] {
                            let mut specialized = SoundFontSampleVoice {
                                note: Some(60),
                                region: Some(region),
                                position,
                                velocity: 0.9,
                                age: 1,
                                envelope: VoiceEnvelopeState::IDLE,
                            };
                            specialized.envelope.note_on(
                                VoiceEnvelope::new(0.0, 0.0, gain, 5.0).unwrap(),
                                48_000.0,
                            );
                            let mut general = specialized;
                            let mut actual = [0.125; 128];
                            let mut expected = actual;
                            render_voice::<true>(
                                &mut specialized,
                                region,
                                &wave_data,
                                &mut actual,
                                48_000.0,
                                increment,
                                0.9,
                            );
                            render_voice::<false>(
                                &mut general,
                                region,
                                &wave_data,
                                &mut expected,
                                48_000.0,
                                increment,
                                0.9,
                            );
                            assert_eq!(actual, expected);
                            assert_eq!(specialized, general);
                        }
                    }
                }
            }
        }
    }
}
