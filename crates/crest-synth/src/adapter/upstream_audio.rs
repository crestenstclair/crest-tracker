//! Shared ownership and capability adapters for the vendored upstream DSP catalog.
//! All metadata, instances, voices, and scratch are prepared off callback.
use crate::kernel::midi_message::{MidiMessage, MidiMessageKind};
use crate::kernel::{MidiChannel, PatchId};
use crate::real_time::{RtPatchParameters, RtPostEffectParameters};
use crate::synth::prepared_instrument::{MidiSource, MidiVoiceId};
use crate::synth::voice_envelope_state::{VoiceEnvelopeStage, VoiceEnvelopeState};
use crate::synth::*;
use std::ffi::{c_char, c_void, CStr};
use std::ptr::NonNull;
use std::sync::OnceLock;

const MIDI_CHANNEL_COUNT: usize = MidiChannel::MAX as usize + 1;

extern "C" {
    fn crest_audio_count() -> usize;
    fn crest_audio_id(index: usize) -> *const c_char;
    fn crest_audio_name(index: usize) -> *const c_char;
    fn crest_audio_is_instrument(index: usize) -> bool;
    fn crest_audio_create_scheduled(
        index: usize,
        rate: f32,
        frames: usize,
        phase: u32,
    ) -> *mut c_void;
    fn crest_audio_load_sample(
        handle: *mut c_void,
        bytes: *const u8,
        size: usize,
        frames: usize,
    ) -> bool;
    fn crest_audio_load_ir(
        handle: *mut c_void,
        pcm: *const f32,
        frames: usize,
        channels: usize,
        rate: f32,
    ) -> bool;
    fn crest_audio_load(handle: *mut c_void, bytes: *const u8, size: usize) -> bool;
    fn crest_audio_latency(handle: *mut c_void) -> usize;
    fn crest_audio_destroy(handle: *mut c_void);
    fn crest_audio_param_count(handle: *mut c_void) -> usize;
    fn crest_audio_param_label(handle: *mut c_void, index: usize) -> *const c_char;
    fn crest_audio_param_default(handle: *mut c_void, index: usize) -> f32;
    fn crest_audio_param_min(handle: *mut c_void, index: usize) -> f32;
    fn crest_audio_param_max(handle: *mut c_void, index: usize) -> f32;
    fn crest_audio_param_stepped(handle: *mut c_void, index: usize) -> bool;
    fn crest_audio_set(handle: *mut c_void, values: *const f32, count: usize) -> bool;
    fn crest_audio_note(handle: *mut c_void, status: i32, data1: i32, data2: i32);
    fn crest_audio_reset(handle: *mut c_void);
    fn crest_audio_process(handle: *mut c_void, stereo: *mut f32, frames: usize) -> bool;
}

pub(crate) trait VoiceProcessor: Send {
    fn latency(&self) -> usize;
    fn set(&mut self, values: &[f32]) -> bool;
    fn note(&mut self, status: i32, a: u8, b: u8);
    fn reset(&mut self);
    fn process(&mut self, stereo: &mut [f32], frames: usize) -> bool;
}
struct Processor {
    handle: NonNull<c_void>,
    max_frames: usize,
}
// Exclusive instance ownership moves to audio after preparation. No upstream
// object is shared concurrently between the preparation worker and callback.
unsafe impl Send for Processor {}
impl Processor {
    fn new(index: usize, rate: f32, max_frames: usize) -> Option<Self> {
        Self::new_scheduled(index, rate, max_frames, 0)
    }
    fn new_scheduled(index: usize, rate: f32, max_frames: usize, phase: u32) -> Option<Self> {
        NonNull::new(unsafe { crest_audio_create_scheduled(index, rate, max_frames, phase) })
            .map(|handle| Self { handle, max_frames })
    }
    fn load_ir(&mut self, decoded: &DecodedSample) -> bool {
        unsafe {
            crest_audio_load_ir(
                self.handle.as_ptr(),
                decoded.interleaved().as_ptr(),
                decoded.metadata().frames() as usize,
                usize::from(decoded.metadata().channels()),
                decoded.metadata().sample_rate() as f32,
            )
        }
    }
    fn load(&mut self, bytes: &[u8]) -> bool {
        unsafe { crest_audio_load(self.handle.as_ptr(), bytes.as_ptr(), bytes.len()) }
    }
    fn latency(&self) -> usize {
        unsafe { crest_audio_latency(self.handle.as_ptr()) }
    }
    fn set(&mut self, values: &[f32]) -> bool {
        unsafe { crest_audio_set(self.handle.as_ptr(), values.as_ptr(), values.len()) }
    }
    fn note(&mut self, status: i32, a: u8, b: u8) {
        unsafe { crest_audio_note(self.handle.as_ptr(), status, i32::from(a), i32::from(b)) }
    }
    fn reset(&mut self) {
        unsafe { crest_audio_reset(self.handle.as_ptr()) }
    }
    fn process(&mut self, stereo: &mut [f32], frames: usize) -> bool {
        frames <= self.max_frames
            && stereo.len() == frames.saturating_mul(2)
            && unsafe { crest_audio_process(self.handle.as_ptr(), stereo.as_mut_ptr(), frames) }
    }
}
impl VoiceProcessor for Processor {
    fn latency(&self) -> usize {
        self.latency()
    }
    fn set(&mut self, values: &[f32]) -> bool {
        self.set(values)
    }
    fn note(&mut self, status: i32, a: u8, b: u8) {
        self.note(status, a, b)
    }
    fn reset(&mut self) {
        self.reset()
    }
    fn process(&mut self, stereo: &mut [f32], frames: usize) -> bool {
        self.process(stereo, frames)
    }
}
pub(crate) fn prepare_voice_bank<P: VoiceProcessor + 'static>(
    patch: &Patch,
    rate: f32,
    max_frames: usize,
    mut create: impl FnMut() -> Result<P, InstrumentPreparationError>,
) -> Result<Box<dyn PreparedInstrument>, InstrumentPreparationError> {
    let mut voices = Vec::new();
    voices
        .try_reserve_exact(usize::from(patch.voice_limit().value()))
        .map_err(|_| InstrumentPreparationError::StorageAllocationFailed {
            patch_id: patch.id(),
        })?;
    for _ in 0..patch.voice_limit().value() {
        voices.push(Voice {
            processor: create()?,
            source: MidiSource::Immediate,
            voice_id: None,
            expression: 1.0,
            pressure: 1.0,
            channel: patch.channel(),
            key_pressure: 1.0,
            envelope: VoiceEnvelopeState::IDLE,
            note: None,
            age: 0,
            held: false,
            delay: 0,
        });
    }
    Ok(Box::new(PreparedVoiceBank {
        patch_id: patch.id(),
        voices,
        scratch: vec![0.; max_frames * 2],
        gains: vec![0.; max_frames],
        rate,
        age: 0,
        controls: [[SourceMidiControls::DEFAULT; MIDI_CHANNEL_COUNT]; MidiSource::COUNT],
    }))
}
impl Drop for Processor {
    fn drop(&mut self) {
        unsafe { crest_audio_destroy(self.handle.as_ptr()) }
    }
}

#[derive(Clone)]
pub struct UpstreamInstrument {
    index: usize,
    descriptor: CapabilityDescriptor,
}
#[derive(Clone)]
pub struct UpstreamEffect {
    index: usize,
    descriptor: EffectCapabilityDescriptor,
}
struct Catalog {
    instruments: Vec<UpstreamInstrument>,
    effects: Vec<UpstreamEffect>,
}
#[derive(Clone, Debug, thiserror::Error)]
#[error("upstream audio catalog: {0}")]
pub struct CatalogError(String);

fn catalog() -> Result<&'static Catalog, CatalogError> {
    static CATALOG: OnceLock<Result<Catalog, CatalogError>> = OnceLock::new();
    CATALOG
        .get_or_init(build_catalog)
        .as_ref()
        .map_err(Clone::clone)
}
pub fn instrument_ports() -> Result<Vec<UpstreamInstrument>, CatalogError> {
    Ok(catalog()?.instruments.clone())
}
pub fn effect_ports() -> Result<Vec<UpstreamEffect>, CatalogError> {
    Ok(catalog()?.effects.clone())
}

fn text(pointer: *const c_char) -> Result<String, CatalogError> {
    if pointer.is_null() {
        return Err(CatalogError("missing native metadata".into()));
    }
    unsafe { CStr::from_ptr(pointer) }
        .to_str()
        .map(str::to_owned)
        .map_err(|e| CatalogError(e.to_string()))
}
// Names follow the pinned upstream enums and preset tables, not ordinal IDs.
fn stepped_labels(native_id: &str, parameter: usize) -> Vec<String> {
    if (native_id, parameter) == ("stk.voicform", 0) {
        // Phonemes.cpp and VoicForm::controlChange: four formant scales,
        // each covering the same 32 phonemes in the original order.
        return ["0.9", "1.0", "1.1", "1.2"]
            .iter()
            .flat_map(|scale| {
                [
                    "eee", "ihh", "ehh", "aaa", "ahh", "aww", "ohh", "uhh", "uuu", "ooo", "rrr",
                    "lll", "mmm", "nnn", "nng", "ngg", "fff", "sss", "thh", "shh", "xxx", "hee",
                    "hoo", "hah", "bbb", "ddd", "jjj", "ggg", "vvv", "zzz", "thz", "zhh",
                ]
                .into_iter()
                .map(move |phoneme| format!("{phoneme} (formants {scale}×)"))
            })
            .collect();
    }
    let names: &[&str] = match (native_id, parameter) {
        // Registration order in plaits/dsp/voice.cc, including the three FM banks.
        ("mutable.plaits", 0) => &[
            "Virtual Analog + VCF",
            "Phase Distortion",
            "6-Operator FM Bank 1",
            "6-Operator FM Bank 2",
            "6-Operator FM Bank 3",
            "Wave Terrain",
            "String Machine",
            "Chiptune",
            "Virtual Analog",
            "Waveshaping",
            "2-Operator FM",
            "Granular Formant",
            "Additive",
            "Wavetable",
            "Chords",
            "Speech",
            "Swarm",
            "Filtered Noise",
            "Particle Noise",
            "String",
            "Modal Resonator",
            "Bass Drum",
            "Snare Drum",
            "Hi-Hat",
        ],
        ("mutable.rings" | "mutable.ringsresonator", 0) => &[
            "Modal Resonator",
            "Sympathetic Strings",
            "Inharmonic String",
            "FM Voice",
            "Quantized Sympathetic Strings",
            "String + Reverb",
        ],
        ("mutable.elements", 0) => &[
            "Modal Resonator",
            "Inharmonic String",
            "String Chords",
            "Ominous FM",
        ],
        // Original Rings chord table in rings/dsp/part.cc, polyphony 1.
        ("mutable.rings", 5) => &[
            "Octaves",
            "Minor 7",
            "Minor",
            "Minor add9",
            "Minor add11",
            "Fifths",
            "Major add11",
            "Major add9",
            "Major",
            "Major 7",
            "Sus4",
        ],
        // tides2/ramp_generator.h OutputMode and physical output channels.
        ("mutable.tides", 0) => &["Gates", "Amplitude", "Slope / Phase", "Frequency"],
        ("mutable.tides", 5 | 6) => &["Output 1", "Output 2", "Output 3", "Output 4"],
        ("mutable.analogsnare" | "mutable.syntheticsnare", 4)
        | (
            "mutable.cloudsgranular"
            | "mutable.cloudsstretch"
            | "mutable.cloudsdelay"
            | "mutable.cloudsspectral",
            9,
        )
        | ("mutable.warps", 6)
        | ("daisy.analogbassdrum", 5)
        | ("daisy.syntheticbassdrum", 6)
        | ("daisy.hihat", 4)
        | ("daisy.stringvoice" | "daisy.modalvoice", 3)
        | ("signalsmith.stretch", 2) => &["Off", "On"],
        // Warps' xmod, vocoder, and shifter carriers differ. The field label
        // declares that order; resources.h names the quadrature wave tables.
        ("mutable.warps", 2) => &[
            "Left Input",
            "Sine / Saw / Sine",
            "Triangle / Pulse / Harmonics",
            "Saw / Noise / Buzzy",
        ],
        // DaisySP Synthesis/oscillator.h Waveform enum.
        ("daisy.tremolo", 2) => &[
            "Sine",
            "Triangle",
            "Saw",
            "Ramp",
            "Square",
            "PolyBLEP Triangle",
            "PolyBLEP Saw",
            "PolyBLEP Square",
        ],
        // STK Shakers.h, ModalBar.h, and BandedWG.h preset order.
        ("stk.shakers", 0) => &[
            "Maraca",
            "Cabasa",
            "Sekere",
            "Tambourine",
            "Sleigh Bells",
            "Bamboo Chimes",
            "Sand Paper",
            "Coke Can",
            "Sticks",
            "Crunch",
            "Big Rocks",
            "Little Rocks",
            "Next Mug",
            "Penny + Mug",
            "Nickel + Mug",
            "Dime + Mug",
            "Quarter + Mug",
            "Franc + Mug",
            "Peso + Mug",
            "Guiro",
            "Wrench",
            "Water Drops",
            "Tuned Bamboo Chimes",
        ],
        ("stk.modalbar", 0) => &[
            "Marimba",
            "Vibraphone",
            "Agogo",
            "Wood 1",
            "Reso",
            "Wood 2",
            "Beats",
            "Two Fixed",
            "Clump",
        ],
        ("stk.bandedwg", 0) => &[
            "Uniform Bar",
            "Tuned Bar",
            "Glass Harmonica",
            "Tibetan Bowl",
        ],
        _ => &[],
    };
    names.iter().map(|name| (*name).to_owned()).collect()
}

// Find exact f32 selector boundaries using the upstream's monotone conversion.
// Decimal approximations of 2/7.9, for example, can name the wrong DSP mode.
fn normalized_label_bands(
    names: &[&str],
    select: impl Fn(f32) -> usize,
) -> Vec<ContinuousValueLabel> {
    let mut minimum = 0_u32;
    names
        .iter()
        .enumerate()
        .filter_map(|(index, name)| {
            let mut low = minimum;
            let mut high = 1.0_f32.to_bits() + 1;
            while low < high {
                let middle = low + (high - low) / 2;
                if select(f32::from_bits(middle)) <= index {
                    low = middle + 1;
                } else {
                    high = middle;
                }
            }
            let start = minimum;
            minimum = low;
            if name.is_empty() || start == low {
                return None;
            }
            Some(ContinuousValueLabel::new(
                ParameterRange::new(f32::from_bits(start).into(), f32::from_bits(low - 1).into())
                    .expect("ordered finite normalized bounds"),
                *name,
            ))
        })
        .collect()
}

fn continuous_labels(native_id: &str, parameter: usize) -> Vec<ContinuousValueLabel> {
    match (native_id, parameter) {
        // Biquad2Proc.cpp uses double constants after reading the float control.
        ("airwindows.biquad2", 0) => {
            normalized_label_bands(&["Lowpass", "Highpass", "Bandpass", "Notch"], |v| {
                ((f64::from(v) * 3.999 + 0.00001).ceil() as usize).saturating_sub(1)
            })
        }
        ("mda.jx10", 3) => normalized_label_bands(
            &[
                "Poly",
                "Poly",
                "Poly Legato",
                "Poly Glide",
                "Mono",
                "Mono",
                "Mono Legato",
                "Mono Glide",
            ],
            |v| (7.9_f32 * v) as usize,
        ),
        ("mda.jx10", 22) => normalized_label_bands(
            &[
                "−2 Octaves",
                "−1 Octave",
                "Unison",
                "+1 Octave",
                "+2 Octaves",
            ],
            // update() computes floor(param[22] * 4.9) in double precision.
            // Its getParameterDisplay uses float, which disagrees at some edges.
            |v| (4.9 * f64::from(v)).floor() as usize,
        ),
        ("mda.leslie", 0) => normalized_label_bands(&["Stop", "Slow", "Fast"], |v| {
            if v < 0.1_f32 {
                0
            } else if v < 0.5_f32 {
                1
            } else {
                2
            }
        }),
        ("mda.talkbox", 2) => {
            normalized_label_bands(&["Right", "Left"], |v| usize::from(v > 0.5_f32))
        }
        ("mda.epiano", 4) | ("mda.jx10", 20) => {
            let piano = native_id == "mda.epiano";
            let mut bands = normalized_label_bands(
                if piano {
                    &["Pan", "Tremolo"]
                } else {
                    &["PWM", "Vibrato"]
                },
                |v| usize::from(if piano { v > 0.5_f32 } else { v >= 0.5_f32 }),
            );
            bands[0] = bands[0].clone().with_amount(-200.0, 100.0, "%");
            bands[1] = bands[1].clone().with_amount(200.0, -100.0, "%");
            bands
        }
        ("mda.jx10", 10) => normalized_label_bands(&["Off", ""], |v| usize::from(v >= 0.05_f32)),
        ("mda.dynamics", 1) => normalized_label_bands(&["", "Limit", ""], |v| {
            if f64::from(v) <= 0.58 {
                0
            } else if f64::from(v) < 0.62 {
                1
            } else {
                2
            }
        }),
        ("mda.dynamics", 5) => {
            normalized_label_bands(&["", "Off"], |v| usize::from(f64::from(v) > 0.98))
        }
        ("mda.dynamics", 6) => {
            normalized_label_bands(&["Off", ""], |v| usize::from(f64::from(v) >= 0.02))
        }
        ("mda.dubdelay", 1 | 2) => {
            // DubDelay combines feedback protection and tone with signed amounts.
            let mut bands = normalized_label_bands(
                if parameter == 1 {
                    &["Saturating", "Limiting"]
                } else {
                    &["Low", "High"]
                },
                |v| usize::from(v > 0.5_f32),
            );
            let extent = if parameter == 1 { 110.0 } else { 100.0 };
            bands[0] = bands[0].clone().with_amount(-2.0 * extent, extent, "%");
            bands[1] = bands[1].clone().with_amount(2.0 * extent, -extent, "%");
            bands
        }
        _ => Vec::new(),
    }
}

fn build_catalog() -> Result<Catalog, CatalogError> {
    let mut catalog = Catalog {
        instruments: Vec::new(),
        effects: Vec::new(),
    };
    for index in 0..unsafe { crest_audio_count() } {
        let id = text(unsafe { crest_audio_id(index) })?
            .to_lowercase()
            .replace('_', ".");
        if id == "sfizz.sample" {
            continue;
        }
        let label = text(unsafe { crest_audio_name(index) })?;
        let native = Processor::new(index, 48_000.0, 256)
            .ok_or_else(|| CatalogError(format!("could not prepare {id}")))?;
        let mut params = Vec::new();
        for i in 0..unsafe { crest_audio_param_count(native.handle.as_ptr()) } {
            let parameter = (|| -> Result<ParameterSpec, CapabilityError> {
                let label = text(unsafe { crest_audio_param_label(native.handle.as_ptr(), i) })
                    .map_err(|_| CapabilityError::EmptyLabel)?;
                let label = match (id.as_str(), i) {
                    ("mutable.warps", 0) => "Algorithm Morph".to_owned(),
                    ("mutable.warps", 2) => "Carrier (Xmod / Vocoder / Shifter)".to_owned(),
                    ("mda.jx10", 3) => "Voice / Glide Mode".to_owned(),
                    ("mda.jx10", 20) => "PWM / Vibrato".to_owned(),
                    _ => label,
                };
                let default =
                    unsafe { crest_audio_param_default(native.handle.as_ptr(), i) } as f64;
                let min = unsafe { crest_audio_param_min(native.handle.as_ptr(), i) } as f64;
                let max = unsafe { crest_audio_param_max(native.handle.as_ptr(), i) } as f64;
                let stepped = unsafe { crest_audio_param_stepped(native.handle.as_ptr(), i) };
                ParameterSpec::new_with_patch_interaction(
                    ParameterId::new(format!("{id}.parameter-{i}"))
                        .map_err(|_| CapabilityError::InvalidMetadataIdentifier(id.clone()))?,
                    label,
                    if stepped {
                        ParameterKind::Stepped
                    } else {
                        ParameterKind::Continuous
                    },
                    ParameterUpdate::Scalar,
                    PatchInteraction::ScalarEdit,
                    ParameterDefault::Value(if stepped {
                        ParameterValue::Stepped(default as i64)
                    } else {
                        ParameterValue::continuous(default)?
                    }),
                    Some(ParameterRange::new(min, max)?),
                    Vec::new(),
                    Some(if stepped { 1.0 } else { (max - min) / 100.0 }),
                    Some(if stepped { 1.0 } else { (max - min) / 10.0 }),
                    None,
                    if stepped { "integer" } else { "number" },
                    None,
                    None,
                )
            })()
            .map_err(|e| CatalogError(format!("{id} parameter {i}: {e}")))?;
            let parameter = parameter
                .with_stepped_labels(stepped_labels(&id, i))
                .and_then(|parameter| parameter.with_continuous_labels(continuous_labels(&id, i)))
                .map_err(|e| CatalogError(format!("{id} parameter {i}: {e}")))?;
            params.push(parameter);
        }
        let mut assets = Vec::new();
        if id == "nam.model" || id == "fft.convolver" {
            let (name, kind, reference) = if id == "nam.model" {
                (
                    super::model_assets::NAM_FILE,
                    AssetKind::NeuralModel,
                    super::model_assets::NAM_DEFAULT,
                )
            } else {
                (
                    super::model_assets::IR_FILE,
                    AssetKind::ImpulseResponse,
                    super::model_assets::IR_DEFAULT,
                )
            };
            params.insert(
                0,
                super::model_assets::spec(name, kind, reference)
                    .map_err(|e| CatalogError(e.to_string()))?,
            );
            assets.push(AssetRequirement::new(ParameterId::new(name).unwrap(), true));
        }
        if id == "sfizz.sampler" {
            let file = ParameterId::new(super::sfz_library::FILE).unwrap();
            params.push(
                ParameterSpec::new_with_patch_interaction(
                    file.clone(),
                    "SFZ Library",
                    ParameterKind::Asset,
                    ParameterUpdate::Structural,
                    PatchInteraction::ReadOnly,
                    ParameterDefault::Asset(
                        AssetReference::new(AssetKind::Sfz, super::sfz_library::DEFAULT)
                            .map_err(|e| CatalogError(e.to_string()))?,
                    ),
                    None,
                    Vec::new(),
                    None,
                    None,
                    None,
                    "asset",
                    None,
                    None,
                )
                .map_err(|e| CatalogError(e.to_string()))?,
            );
            assets.push(AssetRequirement::new(file, true));
        }
        let sections = if params.is_empty() {
            Vec::new()
        } else {
            vec![CapabilitySection::new("controls", "Parameters", params)
                .map_err(|e| CatalogError(e.to_string()))?]
        };
        if unsafe { crest_audio_is_instrument(index) } {
            let id = CapabilityId::new(format!("instrument.{id}"))
                .map_err(|e| CatalogError(e.to_string()))?;
            let descriptor = CapabilityDescriptor::new(
                id.clone(),
                label,
                id.as_str(),
                sections,
                assets,
                VoicePolicy::Configurable { default_voices: 16 },
                vec![
                    MidiMessageKind::NoteOn,
                    MidiMessageKind::NoteOff,
                    MidiMessageKind::ControlChange,
                    MidiMessageKind::PitchBend,
                    MidiMessageKind::ChannelPressure,
                    MidiMessageKind::PolyPressure,
                    MidiMessageKind::AllNotesOff,
                ],
            )
            .map_err(|e| CatalogError(e.to_string()))?
            .with_instrument_category(instrument_category(id.as_str()));
            let descriptor = if id.as_str() == super::dx7_library::CAPABILITY {
                super::dx7_library::Dx7Library::bundled()
                    .descriptor(
                        AssetReference::new(AssetKind::SysEx, super::dx7_library::BUNDLED)
                            .map_err(|e| CatalogError(e.to_string()))?,
                    )
                    .map_err(|e| CatalogError(e.to_string()))?
            } else {
                descriptor
            };
            catalog
                .instruments
                .push(UpstreamInstrument { index, descriptor });
        } else {
            let id = EffectCapabilityId::new(format!("effect.{id}"))
                .map_err(|e| CatalogError(e.to_string()))?;
            let descriptor =
                EffectCapabilityDescriptor::new(id.clone(), label, id.as_str(), sections, assets)
                    .map_err(|e| CatalogError(e.to_string()))?
                    .with_effect_category(effect_category(id.as_str()));
            catalog.effects.push(UpstreamEffect { index, descriptor });
        }
    }
    Ok(catalog)
}

fn effect_category(id: &str) -> EffectCategory {
    match id {
        "effect.airwindows.purestecho"
        | "effect.airwindows.doublelay"
        | "effect.airwindows.tapedelay2"
        | "effect.mda.dubdelay"
        | "effect.mutable.cloudsdelay" => EffectCategory::DelayAndEcho,
        "effect.airwindows.reverb" | "effect.airwindows.kplated" | "effect.fft.convolver" => {
            EffectCategory::ReverbAndIr
        }
        "effect.airwindows.stereochorus"
        | "effect.airwindows.vibrato"
        | "effect.airwindows.autopan"
        | "effect.mda.leslie"
        | "effect.daisy.flanger"
        | "effect.daisy.phaser"
        | "effect.daisy.tremolo" => EffectCategory::Modulation,
        "effect.airwindows.buttercomp2"
        | "effect.airwindows.softgate"
        | "effect.mda.dynamics"
        | "effect.mda.deesser"
        | "effect.daisy.limiter" => EffectCategory::Dynamics,
        "effect.airwindows.baxandall2"
        | "effect.airwindows.parametric"
        | "effect.airwindows.biquad2"
        | "effect.airwindows.capacitor2"
        | "effect.daisy.autowah" => EffectCategory::EqAndFilters,
        "effect.airwindows.density2"
        | "effect.airwindows.totape6"
        | "effect.airwindows.derez3"
        | "effect.nam.model" => EffectCategory::DriveAndAmp,
        "effect.signalsmith.stretch" | "effect.mda.talkbox" | "effect.mutable.warps" => {
            EffectCategory::PitchAndVoice
        }
        "effect.mutable.cloudsgranular"
        | "effect.mutable.cloudsstretch"
        | "effect.mutable.cloudsspectral" => EffectCategory::GranularAndSpectral,
        "effect.mutable.ringsresonator" => EffectCategory::Resonators,
        _ => EffectCategory::Other,
    }
}

fn instrument_category(id: &str) -> InstrumentCategory {
    match id {
        "instrument.mda.piano"
        | "instrument.mda.epiano"
        | "instrument.stk.beethree"
        | "instrument.stk.rhodey"
        | "instrument.stk.wurley" => InstrumentCategory::KeysAndOrgans,
        "instrument.daisy.stringvoice" | "instrument.stk.bowed" | "instrument.stk.mandolin" => {
            InstrumentCategory::Strings
        }
        "instrument.stk.voicform"
        | "instrument.stk.clarinet"
        | "instrument.stk.flute"
        | "instrument.stk.brass" => InstrumentCategory::WindsAndVoices,
        "instrument.daisy.modalvoice"
        | "instrument.stk.bandedwg"
        | "instrument.stk.mesh2d"
        | "instrument.mutable.rings"
        | "instrument.mutable.elements" => InstrumentCategory::Resonators,
        "instrument.daisy.analogbassdrum"
        | "instrument.daisy.syntheticbassdrum"
        | "instrument.daisy.hihat"
        | "instrument.stk.shakers"
        | "instrument.stk.modalbar"
        | "instrument.mutable.peaksbass"
        | "instrument.mutable.peakssnare"
        | "instrument.mutable.peaksfm"
        | "instrument.mutable.peakshat"
        | "instrument.mutable.analogsnare"
        | "instrument.mutable.syntheticsnare" => InstrumentCategory::DrumsAndPercussion,
        "instrument.sfizz.sampler" | "instrument.sfizz.sample" => InstrumentCategory::Samplers,
        _ => InstrumentCategory::Synths,
    }
}
impl InstrumentCapabilityProvider for UpstreamInstrument {
    fn descriptor(&self) -> CapabilityDescriptor {
        self.descriptor.clone()
    }
    fn create_config(
        &self,
        values: &[ParameterAssignment],
        assets: &[AssetAssignment],
    ) -> Result<InstrumentConfig, CapabilityError> {
        self.descriptor.create_config(values, assets)
    }
}
impl EffectCapabilityProvider for UpstreamEffect {
    fn descriptor(&self) -> EffectCapabilityDescriptor {
        self.descriptor.clone()
    }
    fn create_config(
        &self,
        slot: EffectSlotId,
        values: &[ParameterAssignment],
        assets: &[AssetAssignment],
    ) -> Result<PostEffectConfig, EffectCapabilityError> {
        self.descriptor.create_config(slot, values, assets)
    }
}
impl EffectPreparer for UpstreamEffect {
    fn capability_id(&self) -> &EffectCapabilityId {
        self.descriptor.id()
    }
    fn prepare(
        &self,
        patch_id: PatchId,
        config: &PostEffectConfig,
        rate: f32,
        max_frames: usize,
    ) -> Result<Box<dyn PreparedPostEffect>, EffectPreparationError> {
        if !rate.is_finite() || rate < 8000.0 {
            return Err(EffectPreparationError::InvalidSampleRate);
        }
        if max_frames == 0 {
            return Err(EffectPreparationError::InvalidFrameCapacity);
        }
        if config.capability_id() != self.descriptor.id() {
            return Err(EffectPreparationError::UnsupportedCapability { patch_id });
        }
        let canonical = self
            .create_config(config.slot_id(), config.values(), config.asset_references())
            .map_err(|_| EffectPreparationError::InvalidConfiguration { patch_id })?;
        if &canonical != config {
            return Err(EffectPreparationError::InvalidConfiguration { patch_id });
        }
        let mut processor = Processor::new(self.index, rate, max_frames)
            .ok_or(EffectPreparationError::PreparationFailed { patch_id })?;
        for assignment in config.asset_references() {
            let reference = assignment.reference();
            if reference.locator() == super::model_assets::NAM_DEFAULT
                || reference.locator() == super::model_assets::IR_DEFAULT
            {
                continue;
            }
            let bytes = super::model_assets::read(reference)
                .map_err(|_| EffectPreparationError::PreparationFailed { patch_id })?;
            load_model(&mut processor, reference.kind(), &bytes)
                .map_err(|_| EffectPreparationError::PreparationFailed { patch_id })?;
        }
        Ok(Box::new(PreparedEffect {
            patch_id,
            slot_id: config.slot_id(),
            processor,
        }))
    }
}
struct PreparedEffect {
    patch_id: PatchId,
    slot_id: EffectSlotId,
    processor: Processor,
}
impl PreparedPostEffect for PreparedEffect {
    fn patch_id(&self) -> PatchId {
        self.patch_id
    }
    fn slot_id(&self) -> EffectSlotId {
        self.slot_id
    }
    fn process(
        &mut self,
        stereo: &mut [f32],
        frames: usize,
        params: &RtPostEffectParameters,
    ) -> Result<(), PreparedEffectError> {
        if params.slot_id() != Some(self.slot_id) {
            return Err(PreparedEffectError::SlotMismatch);
        }
        if !self.processor.set(params.scalars()) {
            return Err(PreparedEffectError::ScalarLayoutMismatch);
        }
        if !self.processor.process(stereo, frames) {
            return Err(PreparedEffectError::ProcessRejected);
        }
        Ok(())
    }
}
impl InstrumentPreparer for UpstreamInstrument {
    fn asset_descriptor(
        &self,
        config: &InstrumentConfig,
    ) -> Result<Option<CapabilityDescriptor>, InstrumentPreparationError> {
        if self.descriptor.id().as_str() != super::dx7_library::CAPABILITY {
            return Ok(None);
        }
        let reference = config
            .asset_reference(&ParameterId::new(super::dx7_library::FILE).unwrap())
            .ok_or(InstrumentPreparationError::AssetParseFailed)?;
        let library = super::dx7_library::load(reference)
            .map_err(|_| InstrumentPreparationError::AssetParseFailed)?;
        Ok(Some(library.descriptor(reference.clone()).map_err(
            |_| InstrumentPreparationError::AssetParseFailed,
        )?))
    }
    fn capability_id(&self) -> &CapabilityId {
        self.descriptor.id()
    }
    fn prepare(
        &self,
        patch: &Patch,
        rate: f32,
        max_frames: usize,
    ) -> Result<Box<dyn PreparedInstrument>, InstrumentPreparationError> {
        let patch_id = patch.id();
        if !rate.is_finite() || rate < 8000.0 {
            return Err(InstrumentPreparationError::InvalidSampleRate);
        }
        if max_frames == 0 {
            return Err(InstrumentPreparationError::InvalidFrameCapacity);
        }
        if patch.instrument_config().capability_id() != self.descriptor.id() {
            return Err(InstrumentPreparationError::UnsupportedCapability { patch_id });
        }
        let library = if self.descriptor.id().as_str() == super::dx7_library::CAPABILITY {
            let reference = patch
                .instrument_config()
                .asset_reference(&ParameterId::new(super::dx7_library::FILE).unwrap())
                .ok_or(InstrumentPreparationError::InvalidConfiguration { patch_id })?;
            Some((
                super::dx7_library::load(reference)
                    .map_err(|_| InstrumentPreparationError::AssetParseFailed)?,
                reference,
            ))
        } else {
            None
        };
        let descriptor = library
            .as_ref()
            .map(|(library, reference)| library.descriptor((*reference).clone()))
            .transpose()
            .map_err(|_| InstrumentPreparationError::InvalidConfiguration { patch_id })?;
        let canonical = descriptor
            .as_ref()
            .unwrap_or(&self.descriptor)
            .create_config(
                patch.instrument_config().values(),
                patch.instrument_config().asset_references(),
            )
            .map_err(|_| InstrumentPreparationError::InvalidConfiguration { patch_id })?;
        let patch_data = if let Some((library, _)) = &library {
            let Some(ParameterValue::Choice(id)) =
                canonical.value(&ParameterId::new(super::dx7_library::PRESET).unwrap())
            else {
                return Err(InstrumentPreparationError::InvalidConfiguration { patch_id });
            };
            Some(
                library
                    .presets
                    .iter()
                    .find(|p| &p.id == id)
                    .ok_or(InstrumentPreparationError::InvalidConfiguration { patch_id })?
                    .data,
            )
        } else {
            None
        };
        if &canonical != patch.instrument_config() {
            return Err(InstrumentPreparationError::InvalidConfiguration { patch_id });
        }
        let sfz = if self.descriptor.id().as_str() == super::sfz_library::CAPABILITY {
            let reference = canonical
                .asset_reference(&ParameterId::new(super::sfz_library::FILE).unwrap())
                .ok_or(InstrumentPreparationError::AssetParseFailed)?;
            Some(
                super::sfz_library::load(reference)
                    .map_err(|_| InstrumentPreparationError::AssetParseFailed)?,
            )
        } else {
            None
        };
        let mut voices = Vec::new();
        let capacity = usize::from(patch.voice_limit().value());
        voices
            .try_reserve_exact(capacity)
            .map_err(|_| InstrumentPreparationError::StorageAllocationFailed { patch_id })?;
        for voice in 0..capacity {
            // Stable preparation identity spreads FFT boundaries without
            // making output depend on unrelated instance creation or threads.
            let phase = patch_id
                .value()
                .wrapping_mul(0x85eb_ca6b)
                .wrapping_add((voice as u32).wrapping_mul(0x9e37_79b9));
            let mut processor = Processor::new_scheduled(self.index, rate, max_frames, phase)
                .ok_or(InstrumentPreparationError::PreparationFailed { patch_id })?;
            if let Some(data) = &patch_data {
                if !processor.load(data) {
                    return Err(InstrumentPreparationError::AssetParseFailed);
                }
            }
            if let Some(bytes) = &sfz {
                if !processor.load(bytes) {
                    return Err(InstrumentPreparationError::AssetParseFailed);
                }
            }
            voices.push(Voice {
                processor,
                source: MidiSource::Immediate,
                voice_id: None,
                expression: 1.0,
                pressure: 1.0,
                channel: patch.channel(),
                key_pressure: 1.0,
                envelope: VoiceEnvelopeState::IDLE,
                note: None,
                age: 0,
                held: false,
                delay: 0,
            });
        }
        Ok(Box::new(PreparedVoiceBank {
            patch_id,
            voices,
            scratch: vec![0.0; max_frames * 2],
            gains: vec![0.0; max_frames],
            rate,
            age: 0,
            controls: [[SourceMidiControls::DEFAULT; MIDI_CHANNEL_COUNT]; MidiSource::COUNT],
        }))
    }
}
struct Voice<P> {
    processor: P,
    source: MidiSource,
    voice_id: Option<MidiVoiceId>,
    channel: MidiChannel,
    expression: f32,
    pressure: f32,
    key_pressure: f32,
    envelope: VoiceEnvelopeState,
    note: Option<u8>,
    age: u64,
    held: bool,
    delay: usize,
}

impl<P> Voice<P> {
    fn belongs_to(
        &self,
        source: MidiSource,
        voice_id: Option<MidiVoiceId>,
        channel: MidiChannel,
    ) -> bool {
        self.source == source
            && self.channel == channel
            && voice_id.is_none_or(|id| self.voice_id == Some(id))
    }
}
struct PreparedVoiceBank<P> {
    patch_id: PatchId,
    voices: Vec<Voice<P>>,
    scratch: Vec<f32>,
    gains: Vec<f32>,
    rate: f32,
    age: u64,
    controls: [[SourceMidiControls; MIDI_CHANNEL_COUNT]; MidiSource::COUNT],
}

#[derive(Clone, Copy)]
struct SourceMidiControls {
    sustain: bool,
    bend: (u8, u8),
    expression: f32,
    pressure: f32,
}

impl SourceMidiControls {
    const DEFAULT: Self = Self {
        sustain: false,
        bend: (0, 64),
        expression: 1.0,
        pressure: 1.0,
    };
}

impl<P: VoiceProcessor> PreparedVoiceBank<P> {
    fn start_note(
        &mut self,
        source: MidiSource,
        voice_id: Option<MidiVoiceId>,
        channel: MidiChannel,
        note: u8,
        velocity: u8,
        params: &RtPatchParameters,
    ) -> Result<(), PreparedInstrumentError> {
        let voice = self
            .voices
            .iter_mut()
            .find(|voice| voice.envelope.is_idle())
            .ok_or(PreparedInstrumentError::DispatchRejected)?;
        let controls = self.controls[source.index()][usize::from(channel.value())];
        // Reassignment reuses the existing native processor. Reset first,
        // then restore the new owner's controls; no old source state survives.
        voice.processor.reset();
        if !voice.processor.set(params.instrument().values()) {
            return Err(PreparedInstrumentError::DispatchRejected);
        }
        voice.processor.note(0xe0, controls.bend.0, controls.bend.1);
        voice.processor.note(0x90, note, velocity);
        voice.source = source;
        voice.voice_id = voice_id;
        voice.channel = channel;
        voice.expression = controls.expression;
        voice.pressure = controls.pressure;
        voice.key_pressure = 1.0;
        voice.delay = voice.processor.latency();
        voice.envelope.note_on(*params.envelope(), self.rate);
        self.age = self.age.wrapping_add(1);
        voice.age = self.age;
        voice.note = Some(note);
        voice.held = true;
        Ok(())
    }

    fn release_note(
        &mut self,
        source: MidiSource,
        voice_id: Option<MidiVoiceId>,
        channel: MidiChannel,
        note: u8,
        params: &RtPatchParameters,
    ) {
        for voice in self.voices.iter_mut().filter(|voice| {
            voice.belongs_to(source, voice_id, channel) && voice.note == Some(note) && voice.held
        }) {
            voice.held = false;
            if !self.controls[source.index()][usize::from(channel.value())].sustain {
                voice
                    .envelope
                    .note_off(params.envelope().release_milliseconds(), self.rate);
                voice.processor.note(0x80, note, 0);
            }
        }
    }

    fn set_sustain(
        &mut self,
        source: MidiSource,
        channel: MidiChannel,
        enabled: bool,
        params: &RtPatchParameters,
    ) {
        self.controls[source.index()][usize::from(channel.value())].sustain = enabled;
        if enabled {
            return;
        }
        for voice in self
            .voices
            .iter_mut()
            .filter(|voice| voice.source == source && voice.channel == channel && !voice.held)
        {
            voice
                .envelope
                .note_off(params.envelope().release_milliseconds(), self.rate);
            if let Some(note) = voice.note {
                voice.processor.note(0x80, note, 0);
            }
        }
    }

    fn set_bend(
        &mut self,
        source: MidiSource,
        voice_id: Option<MidiVoiceId>,
        channel: MidiChannel,
        a: u8,
        b: u8,
    ) {
        if voice_id.is_none() {
            self.controls[source.index()][usize::from(channel.value())].bend = (a, b);
        }
        // Idle voices reset at allocation; every held/releasing matching
        // voice receives ordered bends without affecting other source/channels.
        for voice in self.voices.iter_mut().filter(|voice| {
            voice.belongs_to(source, voice_id, channel) && !voice.envelope.is_idle()
        }) {
            voice.processor.note(0xe0, a, b);
        }
    }

    fn silence_channel(&mut self, source: MidiSource, channel: MidiChannel) {
        for voice in self
            .voices
            .iter_mut()
            .filter(|voice| voice.source == source && voice.channel == channel)
        {
            voice.envelope = VoiceEnvelopeState::IDLE;
            voice.note = None;
            voice.voice_id = None;
            voice.held = false;
            voice.processor.reset();
        }
        self.controls[source.index()][usize::from(channel.value())].sustain = false;
    }
}

impl<P: VoiceProcessor> PreparedInstrument for PreparedVoiceBank<P> {
    fn patch_id(&self) -> PatchId {
        self.patch_id
    }

    fn active_note_count(&self) -> Option<u32> {
        Some(
            self.voices
                .iter()
                .filter(|voice| {
                    voice.note.is_some()
                        && !voice.envelope.is_idle()
                        && (voice.held
                            || (!voice.envelope.is_releasing()
                                && self.controls[voice.source.index()]
                                    [usize::from(voice.channel.value())]
                                .sustain))
                })
                .count() as u32,
        )
    }
    fn dispatch(
        &mut self,
        message: MidiMessage,
        params: &RtPatchParameters,
    ) -> Result<(), PreparedInstrumentError> {
        self.dispatch_from(MidiSource::Immediate, message, params)
    }

    fn dispatch_from(
        &mut self,
        source: MidiSource,
        message: MidiMessage,
        params: &RtPatchParameters,
    ) -> Result<(), PreparedInstrumentError> {
        self.dispatch_voice_from(source, None, message, params)
    }

    fn dispatch_voice_from(
        &mut self,
        source: MidiSource,
        voice_id: Option<MidiVoiceId>,
        message: MidiMessage,
        params: &RtPatchParameters,
    ) -> Result<(), PreparedInstrumentError> {
        if params.patch_id() != Some(self.patch_id) {
            return Err(PreparedInstrumentError::DispatchRejected);
        }
        let a = message.data1();
        let b = message.data2();
        let channel = message.channel();
        match message.kind() {
            MidiMessageKind::NoteOn if b > 0 => {
                self.start_note(source, voice_id, channel, a, b, params)?
            }
            MidiMessageKind::NoteOn | MidiMessageKind::NoteOff => {
                self.release_note(source, voice_id, channel, a, params)
            }
            MidiMessageKind::PitchBend => self.set_bend(source, voice_id, channel, a, b),
            MidiMessageKind::PolyPressure => {
                for voice in &mut self.voices {
                    if voice.belongs_to(source, voice_id, channel) && voice.note == Some(a) {
                        voice.key_pressure = f32::from(b) / 127.0;
                    }
                }
            }
            MidiMessageKind::ChannelPressure => {
                let pressure = f32::from(a) / 127.0;
                if voice_id.is_none() {
                    self.controls[source.index()][usize::from(channel.value())].pressure = pressure;
                }
                for voice in self
                    .voices
                    .iter_mut()
                    .filter(|voice| voice.belongs_to(source, voice_id, channel))
                {
                    voice.pressure = pressure;
                }
            }
            MidiMessageKind::ControlChange => match a {
                64 if voice_id.is_none() => self.set_sustain(source, channel, b >= 64, params),
                7 | 11 => {
                    let expression = f32::from(b) / 127.0;
                    if voice_id.is_none() {
                        self.controls[source.index()][usize::from(channel.value())].expression =
                            expression;
                    }
                    for voice in self
                        .voices
                        .iter_mut()
                        .filter(|voice| voice.belongs_to(source, voice_id, channel))
                    {
                        voice.expression = expression;
                    }
                }
                120 | 123 if voice_id.is_none() => self.silence_channel(source, channel),
                // There is no shared native CC74/timbre mapping. Tagged
                // unsupported controls must not silently modify a whole channel.
                _ if voice_id.is_some() => return Err(PreparedInstrumentError::DispatchRejected),
                // Native controls belong to capability parameters. Bank and
                // channel-mode CCs must not bypass asset or voice ownership.
                _ => {}
            },
            MidiMessageKind::AllNotesOff if voice_id.is_none() => {
                self.silence_channel(source, channel)
            }
            MidiMessageKind::AllNotesOff => return Err(PreparedInstrumentError::DispatchRejected),
            MidiMessageKind::ProgramChange => {
                return Err(PreparedInstrumentError::DispatchRejected)
            }
        }
        Ok(())
    }
    fn render(
        &mut self,
        stereo: &mut [f32],
        frames: usize,
        params: &RtPatchParameters,
    ) -> Result<(), crate::synth::PreparedInstrumentError> {
        stereo.fill(0.0);
        if stereo.len() != frames.saturating_mul(2) || stereo.len() > self.scratch.len() {
            return Err(PreparedInstrumentError::InvalidFrameCapacity);
        }
        for voice in &mut self.voices {
            if voice.envelope.is_idle() {
                continue;
            }
            let scratch = &mut self.scratch[..stereo.len()];
            scratch.fill(0.0);
            if !voice.processor.set(params.instrument().values()) {
                stereo.fill(0.0);
                return Err(PreparedInstrumentError::ScalarLayoutMismatch);
            }
            if !voice.processor.process(scratch, frames) {
                stereo.fill(0.0);
                return Err(PreparedInstrumentError::RenderRejected);
            }
            // Sustain has no sample-domain state transition. Hoisting its
            // constant gain also permits SIMD mixing without changing the
            // order in which independent voices contribute to the Patch.
            if voice.delay == 0 && voice.envelope.stage() == VoiceEnvelopeStage::Sustain {
                let gain =
                    voice.envelope.level() * voice.expression * voice.pressure * voice.key_pressure;
                for (input, output) in scratch.iter().zip(stereo.iter_mut()) {
                    *output += input * gain;
                }
                continue;
            }
            let gains = &mut self.gains[..frames];
            let delayed = voice.delay.min(frames);
            voice.delay -= delayed;
            gains[..delayed].fill(0.0);
            voice.envelope.fill_gains(&mut gains[delayed..], self.rate);
            for ((input, output), gain) in scratch
                .chunks_exact(2)
                .zip(stereo.chunks_exact_mut(2))
                .zip(gains.iter())
            {
                let gain = gain * voice.expression * voice.pressure * voice.key_pressure;
                output[0] += input[0] * gain;
                output[1] += input[1] * gain;
            }
        }

        Ok(())
    }
    fn all_notes_off(&mut self) {
        for voice in &mut self.voices {
            voice.envelope = VoiceEnvelopeState::IDLE;
            voice.note = None;
            voice.voice_id = None;
            voice.held = false;
            voice.processor.reset();
        }
        for controls in self.controls.iter_mut().flatten() {
            controls.sustain = false;
        }
    }

    fn all_notes_off_from(&mut self, source: MidiSource) -> Result<(), PreparedInstrumentError> {
        for voice in self
            .voices
            .iter_mut()
            .filter(|voice| voice.source == source)
        {
            voice.envelope = VoiceEnvelopeState::IDLE;
            voice.note = None;
            voice.voice_id = None;
            voice.held = false;
            voice.processor.reset();
        }
        for controls in &mut self.controls[source.index()] {
            controls.sustain = false;
        }
        Ok(())
    }
}

fn load_model(
    processor: &mut Processor,
    kind: AssetKind,
    bytes: &[u8],
) -> Result<(), SampleAssetError> {
    let loaded = match kind {
        AssetKind::NeuralModel => processor.load(bytes),
        AssetKind::ImpulseResponse => {
            let decoded = super::wav_sample_decoder::WavSampleDecoder
                .decode(&AssetFileId::new("impulse.wav")?, bytes)?;
            processor.load_ir(&decoded)
        }
        _ => false,
    };
    if loaded {
        Ok(())
    } else {
        Err(SampleAssetError::MalformedModel)
    }
}
pub(crate) fn validate_model_asset(kind: AssetKind, bytes: &[u8]) -> Result<(), SampleAssetError> {
    let id = match kind {
        AssetKind::NeuralModel => "effect.nam.model",
        AssetKind::ImpulseResponse => "effect.fft.convolver",
        _ => return Err(SampleAssetError::UnsupportedContainer),
    };
    let ports = effect_ports().map_err(|_| SampleAssetError::Unavailable)?;
    let port = ports
        .iter()
        .find(|p| p.descriptor.id().as_str() == id)
        .ok_or(SampleAssetError::Unavailable)?;
    let mut processor =
        Processor::new(port.index, 48000.0, 256).ok_or(SampleAssetError::Unavailable)?;
    load_model(&mut processor, kind, bytes)
}

pub(crate) fn validate_sfz(bytes: &[u8]) -> Result<(), SampleAssetError> {
    let ports = instrument_ports().map_err(|_| SampleAssetError::Unavailable)?;
    let port = ports
        .iter()
        .find(|p| p.descriptor.id().as_str() == super::sfz_library::CAPABILITY)
        .ok_or(SampleAssetError::Unavailable)?;
    let mut processor =
        Processor::new(port.index, 48000.0, 256).ok_or(SampleAssetError::Unavailable)?;
    if processor.load(bytes) {
        Ok(())
    } else {
        Err(SampleAssetError::MalformedModel)
    }
}

pub(crate) fn prepare_sample_bank(
    patch: &Patch,
    rate: f32,
    max_frames: usize,
    pcm: &PreparedSamplePcm,
) -> Result<Box<dyn PreparedInstrument>, InstrumentPreparationError> {
    use base64::Engine;
    let failure = InstrumentPreparationError::PreparationFailed {
        patch_id: patch.id(),
    };
    let mut bytes = std::io::Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(
            &mut bytes,
            hound::WavSpec {
                channels: pcm.channels(),
                sample_rate: pcm.sample_rate(),
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )
        .map_err(|_| failure)?;
        for value in pcm.interleaved() {
            writer.write_sample(*value).map_err(|_| failure)?;
        }
        writer.finalize().map_err(|_| failure)?;
    }
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes.into_inner());
    let sfz=format!("<sample> name=crest.wav base64data={encoded}\n<region> sample=crest.wav pitch_keycenter=60 ampeg_attack=0 ampeg_decay=0 ampeg_sustain=100 ampeg_release=100\n");
    let index = (0..unsafe { crest_audio_count() })
        .find(|&i| text(unsafe { crest_audio_id(i) }).is_ok_and(|id| id == "sfizz_Sample"))
        .ok_or(failure)?;
    prepare_voice_bank(patch, rate, max_frames, || {
        let processor = Processor::new(index, rate, max_frames).ok_or(failure)?;
        if !unsafe {
            crest_audio_load_sample(
                processor.handle.as_ptr(),
                sfz.as_ptr(),
                sfz.len(),
                pcm.frames(),
            )
        } {
            return Err(failure);
        }
        Ok(processor)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::midi_channel::MidiChannel;
    use crate::mixer::global_parameters::GlobalParameters;
    use crate::mixer::mixer_state::MixerState;
    use crate::mixer::patch_output::PatchOutput;
    use crate::real_time::{GraphRevision, ParameterSnapshot};

    #[derive(Default)]
    struct SourceProbeProcessor {
        bend: (u8, u8),
        resets: u32,
        releases: u32,
    }

    impl VoiceProcessor for SourceProbeProcessor {
        fn latency(&self) -> usize {
            0
        }
        fn set(&mut self, _: &[f32]) -> bool {
            true
        }
        fn note(&mut self, status: i32, a: u8, b: u8) {
            match status {
                0xe0 => self.bend = (a, b),
                0x80 => self.releases += 1,
                _ => {}
            }
        }
        fn reset(&mut self) {
            self.bend = (0, 64);
            self.resets += 1;
        }
        fn process(&mut self, audio: &mut [f32], _: usize) -> bool {
            audio.fill(1.0);
            true
        }
    }

    fn source_bank() -> (PreparedVoiceBank<SourceProbeProcessor>, RtPatchParameters) {
        let patch_id = PatchId::new(1).unwrap();
        let parameters = RtPatchParameters::projected(
            patch_id,
            PatchOutput::default(),
            VoiceEnvelope::new(0.0, 0.0, 1.0, 40.0).unwrap(),
            crate::real_time::RtInstrumentParameters::EMPTY,
        );
        let bank = PreparedVoiceBank {
            patch_id,
            voices: (0..4)
                .map(|_| Voice {
                    processor: SourceProbeProcessor::default(),
                    source: MidiSource::Immediate,
                    voice_id: None,
                    expression: 1.0,
                    pressure: 1.0,
                    channel: MidiChannel::new(0).unwrap(),
                    key_pressure: 1.0,
                    envelope: VoiceEnvelopeState::IDLE,
                    note: None,
                    age: 0,
                    held: false,
                    delay: 0,
                })
                .collect(),
            scratch: vec![0.0; 128],
            gains: vec![0.0; 64],
            rate: 48_000.0,
            age: 0,
            controls: [[SourceMidiControls::DEFAULT; MIDI_CHANNEL_COUNT]; MidiSource::COUNT],
        };
        (bank, parameters)
    }

    fn source_message(kind: MidiMessageKind, a: u8, b: u8) -> MidiMessage {
        MidiMessage::try_new(MidiChannel::new(0).unwrap(), kind, a, b).unwrap()
    }

    #[test]
    fn identified_same_pitch_notes_release_independently_and_monitor_cancel_is_local() {
        let (mut bank, parameters) = source_bank();
        let first = MidiVoiceId::new(1).unwrap();
        let second = MidiVoiceId::new(2).unwrap();
        for (source, id) in [
            (MidiSource::Scheduled, first),
            (MidiSource::Scheduled, second),
            (MidiSource::Monitor, first),
            (MidiSource::Immediate, first),
        ] {
            bank.dispatch_voice_from(
                source,
                Some(id),
                source_message(MidiMessageKind::NoteOn, 60, 100),
                &parameters,
            )
            .unwrap();
        }
        // Wrong pitch and unknown identity cannot release either overlap.
        for (id, pitch) in [(first, 61), (MidiVoiceId::new(99).unwrap(), 60)] {
            bank.dispatch_voice_from(
                MidiSource::Scheduled,
                Some(id),
                source_message(MidiMessageKind::NoteOff, pitch, 0),
                &parameters,
            )
            .unwrap();
        }
        assert!(bank.voices.iter().all(|voice| voice.held));
        bank.dispatch_voice_from(
            MidiSource::Scheduled,
            Some(first),
            source_message(MidiMessageKind::NoteOff, 60, 0),
            &parameters,
        )
        .unwrap();
        assert!(!bank.voices[0].held);
        assert!(bank.voices[1..].iter().all(|voice| voice.held));
        let mut output = [0.0; 128];
        for _ in 0..32 {
            bank.render(&mut output, 64, &parameters).unwrap();
        }
        assert!(output.iter().all(|sample| *sample == 3.0));
        bank.all_notes_off_from(MidiSource::Monitor).unwrap();
        bank.render(&mut output, 64, &parameters).unwrap();
        assert!(output.iter().all(|sample| *sample == 2.0));
        assert!(bank.voices[1].held);
        assert!(bank.voices[3].held);
        bank.all_notes_off_from(MidiSource::Scheduled).unwrap();
        bank.render(&mut output, 64, &parameters).unwrap();
        assert!(output.iter().all(|sample| *sample == 1.0));
    }

    #[test]
    fn identified_expression_does_not_change_other_notes_or_channel_defaults() {
        let (mut bank, parameters) = source_bank();
        let first = MidiVoiceId::new(1).unwrap();
        let second = MidiVoiceId::new(2).unwrap();
        for (source, id) in [
            (MidiSource::Scheduled, first),
            (MidiSource::Scheduled, second),
            (MidiSource::Monitor, first),
        ] {
            bank.dispatch_voice_from(
                source,
                Some(id),
                source_message(MidiMessageKind::NoteOn, 60, 100),
                &parameters,
            )
            .unwrap();
        }
        for event in [
            source_message(MidiMessageKind::PitchBend, 127, 127),
            source_message(MidiMessageKind::ChannelPressure, 64, 0),
            source_message(MidiMessageKind::PolyPressure, 60, 32),
            source_message(MidiMessageKind::ControlChange, 11, 80),
        ] {
            bank.dispatch_voice_from(MidiSource::Scheduled, Some(first), event, &parameters)
                .unwrap();
        }
        assert_eq!(bank.voices[0].processor.bend, (127, 127));
        assert_eq!(bank.voices[1].processor.bend, (0, 64));
        assert_eq!(bank.voices[2].processor.bend, (0, 64));
        let mut output = [0.0; 128];
        bank.render(&mut output, 64, &parameters).unwrap();
        let expected = 2.0 + (64.0 / 127.0) * (32.0 / 127.0) * (80.0 / 127.0);
        assert!(output
            .iter()
            .all(|sample| (*sample - expected).abs() < 1e-6));
        for controller in [64, 74, 123] {
            assert_eq!(
                bank.dispatch_voice_from(
                    MidiSource::Scheduled,
                    Some(first),
                    source_message(MidiMessageKind::ControlChange, controller, 0),
                    &parameters
                ),
                Err(PreparedInstrumentError::DispatchRejected)
            );
        }
        bank.dispatch_voice_from(
            MidiSource::Scheduled,
            Some(MidiVoiceId::new(3).unwrap()),
            source_message(MidiMessageKind::NoteOn, 60, 100),
            &parameters,
        )
        .unwrap();
        assert_eq!(bank.voices[3].processor.bend, (0, 64));
        assert_eq!(bank.voices[3].expression, 1.0);
        assert_eq!(bank.voices[3].pressure, 1.0);
        bank.dispatch_from(
            MidiSource::Scheduled,
            source_message(MidiMessageKind::ChannelPressure, 90, 0),
            &parameters,
        )
        .unwrap();
        assert_eq!(bank.voices[0].pressure, 90.0 / 127.0);
        assert_eq!(bank.voices[1].pressure, 90.0 / 127.0);
        assert_eq!(bank.voices[2].pressure, 1.0);
        assert_eq!(bank.voices[3].pressure, 90.0 / 127.0);
        // Untagged MIDI keeps the established release-all-same-pitch policy.
        bank.dispatch_from(
            MidiSource::Scheduled,
            source_message(MidiMessageKind::NoteOff, 60, 0),
            &parameters,
        )
        .unwrap();
        assert!(!bank.voices[0].held && !bank.voices[1].held && !bank.voices[3].held);
        assert!(bank.voices[2].held);
    }

    #[test]
    fn identified_sustain_count_keeps_overlap_and_excludes_earlier_release_tail() {
        let (mut bank, parameters) = source_bank();
        let first = MidiVoiceId::new(1).unwrap();
        let second = MidiVoiceId::new(2).unwrap();
        for id in [first, second] {
            bank.dispatch_voice_from(
                MidiSource::Scheduled,
                Some(id),
                source_message(MidiMessageKind::NoteOn, 60, 100),
                &parameters,
            )
            .unwrap();
        }
        assert_eq!(bank.active_note_count(), Some(2));
        let off = source_message(MidiMessageKind::NoteOff, 60, 0);
        bank.dispatch_voice_from(MidiSource::Scheduled, Some(first), off, &parameters)
            .unwrap();
        assert_eq!(bank.active_note_count(), Some(1));
        bank.dispatch_from(
            MidiSource::Scheduled,
            source_message(MidiMessageKind::ControlChange, 64, 127),
            &parameters,
        )
        .unwrap();
        assert_eq!(bank.active_note_count(), Some(1));
        bank.dispatch_voice_from(MidiSource::Scheduled, Some(second), off, &parameters)
            .unwrap();
        assert_eq!(bank.active_note_count(), Some(1));
        assert!(!bank.voices[1].held && !bank.voices[1].envelope.is_releasing());
        bank.dispatch_from(
            MidiSource::Scheduled,
            source_message(MidiMessageKind::ControlChange, 64, 0),
            &parameters,
        )
        .unwrap();
        assert_eq!(bank.active_note_count(), Some(0));
    }

    #[test]
    fn source_ownership_isolates_same_pitch_release_sustain_and_cancellation() {
        let (mut bank, parameters) = source_bank();
        let note_on = source_message(MidiMessageKind::NoteOn, 60, 100);
        bank.dispatch(note_on, &parameters).unwrap();
        bank.dispatch_from(MidiSource::Scheduled, note_on, &parameters)
            .unwrap();
        bank.dispatch_from(
            MidiSource::Scheduled,
            source_message(MidiMessageKind::ControlChange, 64, 127),
            &parameters,
        )
        .unwrap();
        bank.dispatch_from(
            MidiSource::Scheduled,
            source_message(MidiMessageKind::NoteOff, 60, 0),
            &parameters,
        )
        .unwrap();
        assert!(bank.voices[0].held);
        assert!(!bank.voices[1].held);
        assert!(!bank.voices[1].envelope.is_releasing());
        bank.dispatch(
            source_message(MidiMessageKind::ControlChange, 64, 0),
            &parameters,
        )
        .unwrap();
        assert_eq!(bank.voices[1].processor.releases, 0);
        bank.dispatch_from(
            MidiSource::Scheduled,
            source_message(MidiMessageKind::ControlChange, 64, 0),
            &parameters,
        )
        .unwrap();
        assert!(bank.voices[1].envelope.is_releasing());
        assert_eq!(bank.voices[0].processor.releases, 0);
        let live_resets = bank.voices[0].processor.resets;
        bank.all_notes_off_from(MidiSource::Scheduled).unwrap();
        assert!(bank.voices[1].envelope.is_idle());
        assert!(bank.voices[0].held);
        assert_eq!(bank.voices[0].processor.resets, live_resets);
        let mut output = [0.0; 128];
        bank.render(&mut output, 64, &parameters).unwrap();
        assert!(output.iter().all(|sample| *sample == 1.0));
    }

    #[test]
    fn source_controls_follow_owned_voices_and_restore_on_slot_reassignment() {
        let (mut bank, parameters) = source_bank();
        bank.dispatch(
            source_message(MidiMessageKind::PitchBend, 0, 60),
            &parameters,
        )
        .unwrap();
        for message in [
            source_message(MidiMessageKind::PitchBend, 17, 76),
            source_message(MidiMessageKind::ControlChange, 11, 32),
            source_message(MidiMessageKind::ChannelPressure, 64, 0),
        ] {
            bank.dispatch_from(MidiSource::Scheduled, message, &parameters)
                .unwrap();
        }
        bank.dispatch_from(
            MidiSource::Scheduled,
            source_message(MidiMessageKind::NoteOn, 60, 100),
            &parameters,
        )
        .unwrap();
        bank.dispatch(
            source_message(MidiMessageKind::NoteOn, 60, 100),
            &parameters,
        )
        .unwrap();
        assert_eq!(bank.voices[0].processor.bend, (17, 76));
        assert_eq!(bank.voices[1].processor.bend, (0, 60));
        bank.dispatch_from(
            MidiSource::Scheduled,
            source_message(MidiMessageKind::PitchBend, 100, 10),
            &parameters,
        )
        .unwrap();
        assert_eq!(bank.voices[0].processor.bend, (100, 10));
        assert_eq!(bank.voices[1].processor.bend, (0, 60));
        let mut output = [0.0; 128];
        bank.render(&mut output, 64, &parameters).unwrap();
        let expected = 1.0 + (32.0 / 127.0) * (64.0 / 127.0);
        assert!(output
            .iter()
            .all(|sample| (*sample - expected).abs() < 1e-7));
        bank.all_notes_off_from(MidiSource::Scheduled).unwrap();
        bank.dispatch(
            source_message(MidiMessageKind::NoteOn, 62, 100),
            &parameters,
        )
        .unwrap();
        assert_eq!(bank.voices[0].source, MidiSource::Immediate);
        assert_eq!(bank.voices[0].processor.bend, (0, 60));
        assert_eq!(bank.voices.len(), 4);
        bank.render(&mut output, 64, &parameters).unwrap();
        assert!(output.iter().all(|sample| *sample == 2.0));
    }

    #[test]
    fn channel_bends_replay_on_admission_and_stay_with_owned_voices() {
        let (mut bank, parameters) = source_bank();
        let message = |channel, kind, a, b| {
            MidiMessage::try_new(MidiChannel::new(channel).unwrap(), kind, a, b).unwrap()
        };
        for (channel, bend) in [(2, (17, 76)), (15, (0, 60))] {
            bank.dispatch_from(
                MidiSource::Scheduled,
                message(channel, MidiMessageKind::PitchBend, bend.0, bend.1),
                &parameters,
            )
            .unwrap();
            bank.dispatch_from(
                MidiSource::Scheduled,
                message(channel, MidiMessageKind::NoteOn, 60, 100),
                &parameters,
            )
            .unwrap();
        }
        bank.dispatch(message(2, MidiMessageKind::NoteOn, 60, 100), &parameters)
            .unwrap();
        assert_eq!(bank.voices[0].processor.bend, (17, 76));
        assert_eq!(bank.voices[1].processor.bend, (0, 60));
        assert_eq!(bank.voices[2].processor.bend, (0, 64));

        bank.dispatch_from(
            MidiSource::Scheduled,
            message(2, MidiMessageKind::NoteOff, 60, 0),
            &parameters,
        )
        .unwrap();
        bank.dispatch_from(
            MidiSource::Scheduled,
            message(2, MidiMessageKind::PitchBend, 127, 127),
            &parameters,
        )
        .unwrap();
        assert!(bank.voices[0].envelope.is_releasing());
        assert!(bank.voices[1].held);
        assert!(bank.voices[2].held);
        assert_eq!(bank.voices[0].processor.bend, (127, 127));
        assert_eq!(bank.voices[1].processor.bend, (0, 60));
        assert_eq!(bank.voices[2].processor.bend, (0, 64));

        bank.dispatch_from(
            MidiSource::Scheduled,
            message(2, MidiMessageKind::AllNotesOff, 0, 0),
            &parameters,
        )
        .unwrap();
        bank.dispatch_from(
            MidiSource::Scheduled,
            message(15, MidiMessageKind::NoteOn, 62, 100),
            &parameters,
        )
        .unwrap();
        assert_eq!(bank.voices[0].channel.value(), 15);
        assert_eq!(bank.voices[0].processor.bend, (0, 60));
        assert_eq!(bank.voices.len(), 4);
    }

    #[test]
    fn source_cancellation_stops_sustained_notes_without_clearing_other_source_state() {
        let (mut bank, parameters) = source_bank();
        for source in MidiSource::ALL {
            bank.dispatch_from(
                source,
                source_message(MidiMessageKind::ControlChange, 64, 127),
                &parameters,
            )
            .unwrap();
            bank.dispatch_from(
                source,
                source_message(MidiMessageKind::NoteOn, 60, 100),
                &parameters,
            )
            .unwrap();
            bank.dispatch_from(
                source,
                source_message(MidiMessageKind::NoteOff, 60, 0),
                &parameters,
            )
            .unwrap();
        }
        bank.all_notes_off_from(MidiSource::Scheduled).unwrap();
        assert!(bank.controls[MidiSource::Immediate.index()][0].sustain);
        assert!(!bank.controls[MidiSource::Scheduled.index()][0].sustain);
        assert!(!bank.voices[0].envelope.is_idle());
        assert!(bank.voices[1].envelope.is_idle());
        bank.all_notes_off();
        assert!(bank.voices.iter().all(|voice| voice.envelope.is_idle()));
        assert!(bank
            .controls
            .iter()
            .flatten()
            .all(|controls| !controls.sustain));
    }

    #[test]
    fn bends_reach_held_and_releasing_voices_and_replay_after_idle_reset() {
        #[derive(Default)]
        struct Recorder(Vec<(i32, u8, u8)>);
        impl VoiceProcessor for Recorder {
            fn latency(&self) -> usize {
                0
            }
            fn set(&mut self, _: &[f32]) -> bool {
                true
            }
            fn note(&mut self, status: i32, a: u8, b: u8) {
                self.0.push((status, a, b));
            }
            fn reset(&mut self) {
                self.0.push((-1, 0, 0));
            }
            fn process(&mut self, stereo: &mut [f32], _: usize) -> bool {
                stereo.fill(1.0);
                true
            }
        }
        let id = PatchId::new(1).unwrap();
        let params = RtPatchParameters::projected(
            id,
            PatchOutput::default(),
            VoiceEnvelope::new(0.0, 0.0, 1.0, 40.0).unwrap(),
            crate::real_time::RtInstrumentParameters::EMPTY,
        );
        let mut bank = PreparedVoiceBank {
            patch_id: id,
            voices: (0..3)
                .map(|_| Voice {
                    processor: Recorder::default(),
                    source: MidiSource::Immediate,
                    voice_id: None,
                    expression: 1.0,
                    pressure: 1.0,
                    channel: MidiChannel::new(0).unwrap(),
                    key_pressure: 1.0,
                    envelope: VoiceEnvelopeState::IDLE,
                    note: None,
                    age: 0,
                    held: false,
                    delay: 0,
                })
                .collect(),
            scratch: vec![0.0; 128],
            gains: vec![0.0; 64],
            rate: 48_000.0,
            age: 0,
            controls: [[SourceMidiControls::DEFAULT; MIDI_CHANNEL_COUNT]; MidiSource::COUNT],
        };
        let message =
            |kind, a, b| MidiMessage::try_new(MidiChannel::new(0).unwrap(), kind, a, b).unwrap();
        bank.dispatch(message(MidiMessageKind::NoteOn, 60, 100), &params)
            .unwrap();
        bank.dispatch(message(MidiMessageKind::NoteOn, 61, 100), &params)
            .unwrap();
        bank.render(&mut [0.0; 128], 64, &params).unwrap();
        bank.dispatch(message(MidiMessageKind::NoteOff, 60, 0), &params)
            .unwrap();
        assert!(bank.voices[0].envelope.is_releasing());
        assert!(bank.voices[1].held);
        for voice in &mut bank.voices {
            voice.processor.0.clear();
        }
        let bends = [(0xe0, 0, 0), (0xe0, 127, 127), (0xe0, 17, 76)];
        for &(_, a, b) in &bends {
            bank.dispatch(message(MidiMessageKind::PitchBend, a, b), &params)
                .unwrap();
        }
        assert_eq!(bank.voices[0].processor.0, bends);
        assert_eq!(bank.voices[1].processor.0, bends);
        assert!(bank.voices[2].processor.0.is_empty());
        bank.dispatch(message(MidiMessageKind::NoteOn, 62, 100), &params)
            .unwrap();
        assert_eq!(
            bank.voices[2].processor.0,
            [(-1, 0, 0), (0xe0, 17, 76), (0x90, 62, 100)]
        );
    }

    #[test]
    fn every_upstream_effect_prepares_and_renders_through_its_public_port() {
        let ports = effect_ports().unwrap();
        assert!(ports.len() >= 21);
        for port in ports {
            let descriptor = port.descriptor();
            let config = descriptor
                .default_config(EffectSlotId::new(1).unwrap())
                .unwrap();
            let scalars = descriptor
                .scalar_parameters()
                .map(|p| p.scalar_value(config.value(p.id()).unwrap()).unwrap())
                .collect::<Vec<_>>();
            let parameters = RtPostEffectParameters::new(config.slot_id(), &scalars).unwrap();
            let mut effect = port
                .prepare(PatchId::new(1).unwrap(), &config, 48_000.0, 256)
                .unwrap();
            let mut energy = 0.0f64;
            for block in 0..128 {
                let mut audio = [0.0; 512];
                for (i, frame) in audio.chunks_exact_mut(2).enumerate() {
                    frame[0] = (((block * 256 + i) as f32) * 0.03).sin() * 0.1;
                    frame[1] = (((block * 256 + i) as f32) * 0.07).sin() * 0.1;
                }
                effect
                    .process(&mut audio, 256, &parameters)
                    .unwrap_or_else(|e| panic!("{}: {e}", descriptor.id()));
                assert!(audio.iter().all(|x| x.is_finite()), "{}", descriptor.id());
                energy += audio.iter().map(|&x| f64::from(x * x)).sum::<f64>();
            }
            assert!(energy > 1e-10, "{} produced silence", descriptor.id());
        }
    }

    #[test]
    fn elements_modulation_offset_rejects_unsupported_values_and_renders_a_full_lfo_cycle() {
        let port = instrument_ports()
            .unwrap()
            .into_iter()
            .find(|port| port.descriptor().id().as_str() == "instrument.mutable.elements")
            .unwrap();
        let descriptor = port.descriptor();
        let id = ParameterId::new("mutable.elements.parameter-16").unwrap();
        let values = descriptor
            .parameters()
            .filter_map(|spec| match spec.default_value() {
                ParameterDefault::Value(value) => {
                    Some(ParameterAssignment::new(spec.id().clone(), value.clone()))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let default = port
            .create_config(&values, &descriptor.default_assets())
            .unwrap();
        assert!(default
            .with_scalar_value(&descriptor, &id, ParameterValue::Continuous(1.0))
            .is_err());
        let range = descriptor.parameter(&id).unwrap().range().unwrap();
        for offset in [range.minimum(), range.maximum()] {
            let config = default
                .with_scalar_value(&descriptor, &id, ParameterValue::Continuous(offset))
                .unwrap();
            let mut patch = Patch::new(
                PatchId::new(1).unwrap(),
                "Elements".into(),
                config,
                MidiChannel::new(0).unwrap(),
                PatchOutput::default(),
            );
            patch.set_voice_limit(1).unwrap();
            let registry = CapabilityRegistry::new(vec![descriptor.clone()]).unwrap();
            let snapshot = ParameterSnapshot::project_patches(
                1,
                GraphRevision::INITIAL,
                GlobalParameters::new(0.0).unwrap(),
                MixerState::default(),
                &[patch.clone()],
                &registry,
            )
            .unwrap();
            let params = &snapshot.patches()[0];
            let mut engine = port.prepare(&patch, 48_000.0, 256).unwrap();
            engine
                .dispatch(
                    MidiMessage::try_new(
                        MidiChannel::new(0).unwrap(),
                        MidiMessageKind::NoteOn,
                        60,
                        100,
                    )
                    .unwrap(),
                    params,
                )
                .unwrap();
            let mut energy = 0.0f64;
            // More than the default 0.5 Hz LFO's complete two-second cycle.
            for _ in 0..512 {
                let mut output = [0.0f32; 512];
                engine.render(&mut output, 256, params).unwrap();
                assert!(output.iter().all(|value| value.is_finite()));
                energy += output
                    .iter()
                    .map(|value| f64::from(value * value))
                    .sum::<f64>();
            }
            assert!(energy > 1e-8, "Elements must sound at offset {offset}");
        }
    }

    #[test]
    fn every_upstream_instrument_plays_and_obeys_patch_release() {
        for port in instrument_ports().unwrap() {
            let descriptor = port.descriptor();
            eprintln!("rendering {}", descriptor.id());
            let values = descriptor
                .parameters()
                .filter_map(|p| match p.default_value() {
                    ParameterDefault::Value(v) => {
                        Some(ParameterAssignment::new(p.id().clone(), v.clone()))
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            let config = port
                .create_config(&values, &descriptor.default_assets())
                .unwrap();
            let mut patch = Patch::new(
                PatchId::new(1).unwrap(),
                "Upstream".into(),
                config,
                MidiChannel::new(0).unwrap(),
                PatchOutput::default(),
            );
            patch.set_voice_limit(2).unwrap();
            let registry = CapabilityRegistry::new(vec![descriptor.clone()]).unwrap();
            let snapshot = ParameterSnapshot::project_patches(
                1,
                GraphRevision::INITIAL,
                GlobalParameters::new(0.0).unwrap(),
                MixerState::default(),
                &[patch.clone()],
                &registry,
            )
            .unwrap();
            let params = &snapshot.patches()[0];
            let mut engine = port.prepare(&patch, 48_000.0, 256).unwrap();
            engine
                .dispatch(
                    MidiMessage::try_new(
                        MidiChannel::new(0).unwrap(),
                        MidiMessageKind::NoteOn,
                        60,
                        100,
                    )
                    .unwrap(),
                    params,
                )
                .unwrap();
            let mut energy = 0.0f64;
            let mut audio = [0.0; 512];
            for _ in 0..128 {
                engine.render(&mut audio, 256, params).unwrap();
                assert!(audio.iter().all(|x| x.is_finite()), "{}", descriptor.id());
                energy += audio.iter().map(|&x| f64::from(x * x)).sum::<f64>();
            }
            assert!(energy > 1e-10, "{} produced silence", descriptor.id());
            engine
                .dispatch(
                    MidiMessage::try_new(
                        MidiChannel::new(0).unwrap(),
                        MidiMessageKind::NoteOff,
                        60,
                        0,
                    )
                    .unwrap(),
                    params,
                )
                .unwrap();
            engine.render(&mut audio, 256, params).unwrap();
            assert!(
                audio.iter().all(|x| *x == 0.0),
                "{} ignored neutral release",
                descriptor.id()
            );
        }
    }
}
