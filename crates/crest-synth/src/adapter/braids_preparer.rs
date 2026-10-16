use crate::adapter::braids_capability::{
    BraidsCapability, BRAIDS_CAPABILITY_ID, BRAIDS_COLOR_PARAMETER_ID, BRAIDS_MODELS,
    BRAIDS_MODEL_PARAMETER_ID, BRAIDS_TIMBRE_PARAMETER_ID,
};
use crate::adapter::braids_native::{
    BraidsNativeError, BraidsVoiceBank, BRAIDS_INTERNAL_CHUNK_FRAMES, BRAIDS_MODEL_COUNT,
};
use crate::kernel::midi_message::{MidiMessage, MidiMessageKind};
use crate::kernel::patch_id::PatchId;
use crate::kernel::MidiChannel;
use crate::real_time::parameter_snapshot::RtPatchParameters;
use crate::synth::capability_id::CapabilityId;
use crate::synth::instrument_capability::ParameterValue;
use crate::synth::instrument_capability_provider::InstrumentCapabilityProvider;
use crate::synth::instrument_preparer::{InstrumentPreparationError, InstrumentPreparer};
use crate::synth::parameter_id::ParameterId;
use crate::synth::patch::Patch;
use crate::synth::prepared_instrument::{
    OwnedExpressionState, OwnedVoiceInitialization, PreparedInstrument, PreparedInstrumentError,
};
use crate::synth::voice_envelope_state::VoiceEnvelopeState;
use crate::synth::{MidiExpressionScope, MidiSource, MidiVoiceId};

pub const BRAIDS_HOST_SAMPLE_RATE: f32 = 48_000.0;
pub const BRAIDS_INTERNAL_SAMPLE_RATE: f32 = 96_000.0;
const HOST_FRAMES_PER_NATIVE_CHUNK: usize = BRAIDS_INTERNAL_CHUNK_FRAMES / 2;
const NORMALIZED_I16: f32 = 1.0 / 32_768.0;
const PITCH_UNITS_PER_SEMITONE: i32 = 128;
const PITCH_BEND_RANGE_SEMITONES: i32 = 2;
const MIDI_BEND_CENTER: i32 = 8_192;
// Upstream oscillator frequency tables saturate at 128 semitones. Owned
// expression rejects pitches outside this interval instead of clipping.
const MAX_OWNED_PITCH: i32 = 128 * PITCH_UNITS_PER_SEMITONE - 1;
const MIDI_CHANNEL_COUNT: usize = MidiChannel::MAX as usize + 1;

/// Shared admission for the renderer and its control-side capability contract.
/// Returns native bend units only for normalized MIDI and an unclipped pitch.
pub(crate) fn owned_pitch_bend_units(note: u8, raw: u16, semitones: u8, cents: u8) -> Option<i16> {
    owned_combined_pitch_bend_units(note, (raw, semitones, cents), (8_192, 2, 0))
}

/// Both dimensions retain their exact raw14 value and range. Sum first, then
/// admit the resulting pitch and quantize once to native 1/128-semitone units.
pub(crate) fn owned_combined_pitch_bend_units(
    note: u8,
    member: (u16, u8, u8),
    master: (u16, u8, u8),
) -> Option<i16> {
    if note > 127 {
        return None;
    }
    let numerator = owned_bend_numerator(member)? + owned_bend_numerator(master)?;
    let denominator = i64::from(MIDI_BEND_CENTER * 100);
    let pitch = i64::from(note) * i64::from(PITCH_UNITS_PER_SEMITONE) * denominator + numerator;
    if !(0..=i64::from(MAX_OWNED_PITCH) * denominator).contains(&pitch) {
        return None;
    }
    Some((numerator / denominator) as i16)
}

fn owned_bend_numerator((raw, semitones, cents): (u16, u8, u8)) -> Option<i64> {
    if raw > 16_383 || semitones > 127 || cents > 99 {
        return None;
    }
    let range_cents = i32::from(semitones) * 100 + i32::from(cents);
    Some(
        (i64::from(raw) - i64::from(MIDI_BEND_CENTER))
            * i64::from(range_cents)
            * i64::from(PITCH_UNITS_PER_SEMITONE),
    )
}

/// Control/worker-side preparer for the pinned Braids capability.
pub struct BraidsPreparer {
    capability_id: CapabilityId,
}

impl BraidsPreparer {
    pub fn new() -> Result<Self, InstrumentPreparationError> {
        let capability_id = CapabilityId::new(BRAIDS_CAPABILITY_ID)
            .map_err(|_| InstrumentPreparationError::AssetParseFailed)?;
        Ok(Self { capability_id })
    }

    fn prepare_patch(
        &self,
        patch: &Patch,
        sample_rate: f32,
        max_frames: usize,
    ) -> Result<PreparedBraidsInstrument, InstrumentPreparationError> {
        if sample_rate != BRAIDS_HOST_SAMPLE_RATE {
            return Err(InstrumentPreparationError::InvalidSampleRate);
        }
        if max_frames == 0 {
            return Err(InstrumentPreparationError::InvalidFrameCapacity);
        }
        PreparedBraidsConfig::try_from_patch(patch)?;
        let bank = BraidsVoiceBank::with_capacity(usize::from(patch.voice_limit().value()))
            .map_err(|_| InstrumentPreparationError::VoiceCapacityExceeded {
                patch_id: patch.id(),
            })?;
        Ok(PreparedBraidsInstrument {
            patch_id: patch.id(),
            bank,
            voices: vec![
                BraidsVoice::idle(patch.channel());
                usize::from(patch.voice_limit().value())
            ],
            expression: [[1.0; MIDI_CHANNEL_COUNT]; MidiSource::COUNT],
            pressure: [[1.0; MIDI_CHANNEL_COUNT]; MidiSource::COUNT],
            pitch_bend: [[0; MIDI_CHANNEL_COUNT]; MidiSource::COUNT],
            pitch_bend_raw: [[MIDI_BEND_CENTER as u16; MIDI_CHANNEL_COUNT]; MidiSource::COUNT],
            sustain: [[false; MIDI_CHANNEL_COUNT]; MidiSource::COUNT],
            next_age: 1,
            max_frames,
            native_scratch: [0; BRAIDS_INTERNAL_CHUNK_FRAMES],
            maximum_native_chunk_observed: 0,
        })
    }
}

impl InstrumentPreparer for BraidsPreparer {
    fn capability_id(&self) -> &CapabilityId {
        &self.capability_id
    }

    fn prepare(
        &self,
        patch: &Patch,
        sample_rate: f32,
        max_frames: usize,
    ) -> Result<Box<dyn PreparedInstrument>, InstrumentPreparationError> {
        self.prepare_patch(patch, sample_rate, max_frames)
            .map(|prepared| Box::new(prepared) as Box<dyn PreparedInstrument>)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct BraidsScalarParameters {
    model: u8,
    timbre: i16,
    color: i16,
}

impl BraidsScalarParameters {
    fn from_rt(parameters: &RtPatchParameters) -> Option<Self> {
        let values = parameters.instrument().values();
        if values.len() != 3 {
            return None;
        }
        let model = values[0];
        if model.fract() != 0.0 || !(0.0..f32::from(BRAIDS_MODEL_COUNT)).contains(&model) {
            return None;
        }
        let timbre = normalized_parameter(values[1])?;
        let color = normalized_parameter(values[2])?;
        Some(Self {
            model: model as u8,
            timbre,
            color,
        })
    }
}

fn normalized_parameter(value: f32) -> Option<i16> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Some((value * f32::from(i16::MAX)).round() as i16)
    } else {
        None
    }
}

/// Scalar state belongs to one prepared attack, never a reused member channel.
#[derive(Clone, Copy, Debug, PartialEq)]
struct OwnedExpression {
    bend_raw: u16,
    bend_semitones: u8,
    bend_cents: u8,
    rpn_msb: Option<u8>,
    rpn_lsb: Option<u8>,
    timbre: Option<i16>,
    sustain: Option<bool>,
}

impl OwnedExpression {
    const fn new(bend_raw: u16) -> Self {
        Self {
            bend_raw,
            bend_semitones: PITCH_BEND_RANGE_SEMITONES as u8,
            bend_cents: 0,
            rpn_msb: None,
            rpn_lsb: None,
            timbre: None,
            sustain: None,
        }
    }

    fn from_initial(state: OwnedExpressionState) -> Self {
        Self {
            bend_raw: state.raw14(),
            bend_semitones: state.semitones(),
            bend_cents: state.cents(),
            rpn_msb: None,
            rpn_lsb: None,
            timbre: state
                .timbre()
                .and_then(|value| normalized_parameter(f32::from(value) / 127.0)),
            sustain: Some(state.pedal() >= 64),
        }
    }

    fn bend_units(self, note: u8, other: Self) -> Result<i16, PreparedInstrumentError> {
        owned_combined_pitch_bend_units(
            note,
            (self.bend_raw, self.bend_semitones, self.bend_cents),
            (other.bend_raw, other.bend_semitones, other.bend_cents),
        )
        .ok_or(PreparedInstrumentError::DispatchRejected)
    }

    fn with_bend(
        mut self,
        note: u8,
        raw: u16,
        other: Self,
    ) -> Result<Self, PreparedInstrumentError> {
        self.bend_raw = raw;
        self.bend_units(note, other)?;
        Ok(self)
    }

    fn with_controller(
        mut self,
        note: u8,
        controller: u8,
        value: u8,
        other: Self,
    ) -> Result<Self, PreparedInstrumentError> {
        match controller {
            101 | 100 if matches!(value, 0 | 127) => {
                if controller == 101 {
                    self.rpn_msb = Some(value);
                } else {
                    self.rpn_lsb = Some(value);
                }
            }
            6 | 38 if self.rpn_msb == Some(0) && self.rpn_lsb == Some(0) => {
                if controller == 6 {
                    self.bend_semitones = value;
                } else if value <= 99 {
                    self.bend_cents = value;
                } else {
                    return Err(PreparedInstrumentError::DispatchRejected);
                }
            }
            74 => self.timbre = normalized_parameter(f32::from(value) / 127.0),
            64 => self.sustain = Some(value >= 64),
            _ => return Err(PreparedInstrumentError::DispatchRejected),
        }
        self.bend_units(note, other)?;
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct BraidsVoice {
    source: MidiSource,
    voice_id: Option<MidiVoiceId>,
    channel: MidiChannel,
    held: bool,
    expression: f32,
    pressure: f32,
    pitch_bend: i16,
    owned: OwnedExpression,
    master: OwnedExpression,
    master_pressure: f32,
    key_pressure: f32,
    note: Option<u8>,
    velocity: f32,
    age: u64,
    envelope: VoiceEnvelopeState,
}

impl BraidsVoice {
    const fn idle(channel: MidiChannel) -> Self {
        Self {
            source: MidiSource::Immediate,
            voice_id: None,
            channel,
            held: false,
            expression: 1.0,
            pressure: 1.0,
            pitch_bend: 0,
            owned: OwnedExpression::new(MIDI_BEND_CENTER as u16),
            master: OwnedExpression::new(MIDI_BEND_CENTER as u16),
            master_pressure: 1.0,
            key_pressure: 1.0,
            note: None,
            velocity: 0.0,
            age: 0,
            envelope: VoiceEnvelopeState::IDLE,
        }
    }

    fn clear(&mut self) {
        *self = Self::idle(self.channel);
    }

    fn sustained(&self, channel_sustain: bool) -> bool {
        self.owned.sustain.unwrap_or(channel_sustain) || self.master.sustain.unwrap_or(false)
    }

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

struct PreparedBraidsInstrument {
    patch_id: PatchId,
    bank: BraidsVoiceBank,
    voices: Vec<BraidsVoice>,
    expression: [[f32; MIDI_CHANNEL_COUNT]; MidiSource::COUNT],
    pressure: [[f32; MIDI_CHANNEL_COUNT]; MidiSource::COUNT],
    pitch_bend: [[i16; MIDI_CHANNEL_COUNT]; MidiSource::COUNT],
    pitch_bend_raw: [[u16; MIDI_CHANNEL_COUNT]; MidiSource::COUNT],
    sustain: [[bool; MIDI_CHANNEL_COUNT]; MidiSource::COUNT],
    next_age: u64,
    max_frames: usize,
    native_scratch: [i16; BRAIDS_INTERNAL_CHUNK_FRAMES],
    maximum_native_chunk_observed: usize,
}

impl PreparedBraidsInstrument {
    fn note_on(
        &mut self,
        source: MidiSource,
        voice_id: Option<MidiVoiceId>,
        message: MidiMessage,
        parameters: &RtPatchParameters,
        scalars: BraidsScalarParameters,
    ) -> Result<(), PreparedInstrumentError> {
        let channel = message.channel();
        let note = message.data1();
        let velocity = message.data2();
        let channel_index = usize::from(channel.value());
        let owned = OwnedExpression::new(self.pitch_bend_raw[source.index()][channel_index]);
        let master = OwnedExpression::new(MIDI_BEND_CENTER as u16);
        let bend = if voice_id.is_some() {
            owned.bend_units(note, master)?
        } else {
            self.pitch_bend[source.index()][channel_index]
        };
        let mut voice = BraidsVoice::idle(channel);
        voice.note = Some(note);
        voice.source = source;
        voice.voice_id = voice_id;
        voice.channel = channel;
        voice.expression = self.expression[source.index()][usize::from(channel.value())];
        voice.pressure = self.pressure[source.index()][usize::from(channel.value())];
        voice.pitch_bend = bend;
        voice.owned = owned;
        voice.master = master;
        voice.master_pressure = 1.0;
        voice.held = true;
        voice.key_pressure = 1.0;
        voice.velocity = f32::from(velocity) / 127.0;
        self.start_voice(voice, parameters, scalars)
    }

    /// Admission and native configuration occur only after the caller has
    /// constructed and validated the complete candidate voice. Failed scalar,
    /// combined-pitch, identity or capacity checks cannot leave a partial note.
    fn start_voice(
        &mut self,
        mut voice: BraidsVoice,
        parameters: &RtPatchParameters,
        scalars: BraidsScalarParameters,
    ) -> Result<(), PreparedInstrumentError> {
        if voice.voice_id.is_some_and(|id| {
            self.voices.iter().any(|active| {
                active.source == voice.source
                    && active.voice_id == Some(id)
                    && !active.envelope.is_idle()
            })
        }) {
            return Err(PreparedInstrumentError::DispatchRejected);
        }
        let note = voice
            .note
            .ok_or(PreparedInstrumentError::DispatchRejected)?;
        let voice_index = self
            .voices
            .iter()
            .position(|active| active.envelope.is_idle())
            .ok_or(PreparedInstrumentError::DispatchRejected)?;
        self.bank
            .reset(voice_index)
            .and_then(|()| {
                self.bank.configure(
                    voice_index,
                    scalars.model,
                    note_pitch(note, voice.pitch_bend),
                    voice.owned.timbre.unwrap_or(scalars.timbre),
                    scalars.color,
                )
            })
            .and_then(|()| self.bank.strike(voice_index))
            .map_err(|_| PreparedInstrumentError::DispatchRejected)?;
        voice.age = self.next_age;
        voice
            .envelope
            .note_on(*parameters.envelope(), BRAIDS_HOST_SAMPLE_RATE);
        self.voices[voice_index] = voice;
        self.next_age = self.next_age.saturating_add(1);
        Ok(())
    }

    fn note_off(
        &mut self,
        source: MidiSource,
        voice_id: Option<MidiVoiceId>,
        channel: MidiChannel,
        note: u8,
        parameters: &RtPatchParameters,
    ) {
        for voice in &mut self.voices {
            if voice.belongs_to(source, voice_id, channel)
                && voice.note == Some(note)
                && !voice.envelope.is_idle()
            {
                voice.held = false;
                if voice.sustained(self.sustain[source.index()][usize::from(channel.value())]) {
                    continue;
                }
                voice.envelope.note_off(
                    parameters.envelope().release_milliseconds(),
                    BRAIDS_HOST_SAMPLE_RATE,
                );
            }
        }
    }

    fn render_voice_chunk(
        &mut self,
        voice_index: usize,
        output: &mut [f32],
        host_offset: usize,
        host_frames: usize,
        scalars: BraidsScalarParameters,
    ) -> Result<(), BraidsNativeError> {
        let note = match self.voices[voice_index].note {
            Some(note) => note,
            None => return Ok(()),
        };
        let native_frames = host_frames * 2;
        self.maximum_native_chunk_observed = self.maximum_native_chunk_observed.max(native_frames);
        self.bank.configure(
            voice_index,
            scalars.model,
            note_pitch(note, self.voices[voice_index].pitch_bend),
            self.voices[voice_index]
                .owned
                .timbre
                .unwrap_or(scalars.timbre),
            scalars.color,
        )?;
        self.bank
            .render(voice_index, &mut self.native_scratch[..native_frames])?;

        let voice = &mut self.voices[voice_index];
        let expression = voice.velocity
            * voice.expression
            * voice.pressure
            * voice.key_pressure
            * voice.master_pressure;
        for host_frame in 0..host_frames {
            let native_frame = host_frame * 2;
            let decimated = (f32::from(self.native_scratch[native_frame])
                + f32::from(self.native_scratch[native_frame + 1]))
                * (0.5 * NORMALIZED_I16);
            let gain = voice.envelope.next_gain(BRAIDS_HOST_SAMPLE_RATE) * expression;
            let sample = bounded_sample(decimated * gain);
            let output_frame = (host_offset + host_frame) * 2;
            output[output_frame] += sample;
            output[output_frame + 1] += sample;
        }
        if voice.envelope.is_idle() {
            voice.clear();
            self.bank.reset(voice_index)?;
        }
        Ok(())
    }

    fn clear_all_voices(&mut self) {
        for (index, voice) in self.voices.iter_mut().enumerate() {
            voice.clear();
            let _ = self.bank.reset(index);
        }
    }

    fn owned_voice_index(
        &self,
        source: MidiSource,
        id: MidiVoiceId,
        channel: MidiChannel,
    ) -> Result<usize, PreparedInstrumentError> {
        self.voices
            .iter()
            .position(|voice| {
                voice.belongs_to(source, Some(id), channel)
                    && voice.note.is_some()
                    && !voice.envelope.is_idle()
            })
            .ok_or(PreparedInstrumentError::DispatchRejected)
    }

    fn owned_controller(
        &mut self,
        source: MidiSource,
        id: MidiVoiceId,
        scope: MidiExpressionScope,
        message: MidiMessage,
        parameters: &RtPatchParameters,
    ) -> Result<(), PreparedInstrumentError> {
        let index = self.owned_voice_index(source, id, message.channel())?;
        let voice = &mut self.voices[index];
        let note = voice
            .note
            .ok_or(PreparedInstrumentError::DispatchRejected)?;
        let (current, other) = match scope {
            MidiExpressionScope::Member => (voice.owned, voice.master),
            MidiExpressionScope::Master => (voice.master, voice.owned),
        };
        let owned = current.with_controller(note, message.data1(), message.data2(), other)?;
        let bend = owned.bend_units(note, other)?;
        match scope {
            MidiExpressionScope::Member => voice.owned = owned,
            MidiExpressionScope::Master => voice.master = owned,
        }
        voice.pitch_bend = bend;
        if message.data1() == 64
            && !voice.held
            && !voice
                .sustained(self.sustain[source.index()][usize::from(message.channel().value())])
        {
            voice.envelope.note_off(
                parameters.envelope().release_milliseconds(),
                BRAIDS_HOST_SAMPLE_RATE,
            );
        }
        Ok(())
    }

    fn owned_pitch_bend(
        &mut self,
        source: MidiSource,
        id: MidiVoiceId,
        scope: MidiExpressionScope,
        message: MidiMessage,
    ) -> Result<(), PreparedInstrumentError> {
        let index = self.owned_voice_index(source, id, message.channel())?;
        let voice = &mut self.voices[index];
        let note = voice
            .note
            .ok_or(PreparedInstrumentError::DispatchRejected)?;
        let raw = u16::from(message.data1()) | (u16::from(message.data2()) << 7);
        let (current, other) = match scope {
            MidiExpressionScope::Member => (voice.owned, voice.master),
            MidiExpressionScope::Master => (voice.master, voice.owned),
        };
        let owned = current.with_bend(note, raw, other)?;
        let bend = owned.bend_units(note, other)?;
        match scope {
            MidiExpressionScope::Member => voice.owned = owned,
            MidiExpressionScope::Master => voice.master = owned,
        }
        voice.pitch_bend = bend;
        Ok(())
    }

    fn apply_pitch_bend(
        &mut self,
        source: MidiSource,
        id: Option<MidiVoiceId>,
        message: MidiMessage,
    ) -> Result<(), PreparedInstrumentError> {
        let raw = u16::from(message.data1()) | (u16::from(message.data2()) << 7);
        let channel = message.channel();
        if let Some(id) = id {
            return self.owned_pitch_bend(source, id, MidiExpressionScope::Member, message);
        }
        // Validate the complete channel update before publishing any part of it.
        for voice in &self.voices {
            if voice.belongs_to(source, None, channel)
                && voice.voice_id.is_some()
                && !voice.envelope.is_idle()
            {
                if let Some(note) = voice.note {
                    voice.owned.with_bend(note, raw, voice.master)?;
                }
            }
        }
        let bend = pitch_bend(message.data1(), message.data2());
        self.pitch_bend[source.index()][usize::from(channel.value())] = bend;
        self.pitch_bend_raw[source.index()][usize::from(channel.value())] = raw;
        for voice in &mut self.voices {
            if !voice.belongs_to(source, None, channel) || voice.envelope.is_idle() {
                continue;
            }
            if let Some(note) = voice.note.filter(|_| voice.voice_id.is_some()) {
                voice.owned.bend_raw = raw;
                voice.pitch_bend = voice.owned.bend_units(note, voice.master)?;
            } else {
                voice.pitch_bend = bend;
            }
        }
        Ok(())
    }

    fn silence_channel(
        &mut self,
        source: MidiSource,
        channel: MidiChannel,
    ) -> Result<(), PreparedInstrumentError> {
        self.sustain[source.index()][usize::from(channel.value())] = false;
        for (index, voice) in self.voices.iter_mut().enumerate() {
            if voice.source == source && voice.channel == channel {
                voice.clear();
                self.bank
                    .reset(index)
                    .map_err(|_| PreparedInstrumentError::DispatchRejected)?;
            }
        }
        Ok(())
    }

    #[cfg(test)]
    fn active_voice_count(&self) -> usize {
        self.voices
            .iter()
            .filter(|voice| !voice.envelope.is_idle())
            .count()
    }
}

impl PreparedInstrument for PreparedBraidsInstrument {
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
                                && voice.sustained(
                                    self.sustain[voice.source.index()]
                                        [usize::from(voice.channel.value())],
                                )))
                })
                .count() as u32,
        )
    }

    fn dispatch(
        &mut self,
        message: MidiMessage,
        parameters: &RtPatchParameters,
    ) -> Result<(), PreparedInstrumentError> {
        self.dispatch_from(MidiSource::Immediate, message, parameters)
    }

    fn dispatch_from(
        &mut self,
        source: MidiSource,
        message: MidiMessage,
        parameters: &RtPatchParameters,
    ) -> Result<(), PreparedInstrumentError> {
        self.dispatch_voice_from(source, None, message, parameters)
    }

    fn dispatch_voice_from(
        &mut self,
        source: MidiSource,
        voice_id: Option<MidiVoiceId>,
        message: MidiMessage,
        parameters: &RtPatchParameters,
    ) -> Result<(), PreparedInstrumentError> {
        if parameters.patch_id() != Some(self.patch_id) {
            return Err(PreparedInstrumentError::DispatchRejected);
        }
        let scalars = BraidsScalarParameters::from_rt(parameters)
            .ok_or(PreparedInstrumentError::DispatchRejected)?;
        let channel = message.channel();
        let channel_index = usize::from(channel.value());
        match message.kind() {
            MidiMessageKind::NoteOn if message.data2() > 0 => {
                self.note_on(source, voice_id, message, parameters, scalars)
            }
            MidiMessageKind::NoteOn | MidiMessageKind::NoteOff => {
                self.note_off(source, voice_id, channel, message.data1(), parameters);
                Ok(())
            }
            MidiMessageKind::ControlChange => {
                if voice_id.is_some() && !matches!(message.data1(), 7 | 11) {
                    return self.owned_controller(
                        source,
                        voice_id.ok_or(PreparedInstrumentError::DispatchRejected)?,
                        MidiExpressionScope::Member,
                        message,
                        parameters,
                    );
                }
                if message.data1() == 64 {
                    self.sustain[source.index()][channel_index] = message.data2() >= 64;
                    if !self.sustain[source.index()][channel_index] {
                        for voice in &mut self.voices {
                            if voice.source == source
                                && voice.channel == channel
                                && !voice.held
                                && !voice.sustained(false)
                            {
                                voice.envelope.note_off(
                                    parameters.envelope().release_milliseconds(),
                                    BRAIDS_HOST_SAMPLE_RATE,
                                );
                            }
                        }
                    }
                }
                if matches!(message.data1(), 7 | 11) {
                    let expression = f32::from(message.data2()) / 127.0;
                    if voice_id.is_none() {
                        self.expression[source.index()][channel_index] = expression;
                    }
                    for voice in self
                        .voices
                        .iter_mut()
                        .filter(|voice| voice.belongs_to(source, voice_id, channel))
                    {
                        voice.expression = expression;
                    }
                }
                if matches!(message.data1(), 120 | 123) {
                    return self.silence_channel(source, channel);
                }
                Ok(())
            }
            MidiMessageKind::PolyPressure => {
                for voice in &mut self.voices {
                    if voice.belongs_to(source, voice_id, channel)
                        && voice.note == Some(message.data1())
                    {
                        voice.key_pressure = f32::from(message.data2()) / 127.0;
                    }
                }
                Ok(())
            }
            MidiMessageKind::ChannelPressure => {
                let pressure = f32::from(message.data1()) / 127.0;
                if voice_id.is_none() {
                    self.pressure[source.index()][channel_index] = pressure;
                }
                for voice in self
                    .voices
                    .iter_mut()
                    .filter(|voice| voice.belongs_to(source, voice_id, channel))
                {
                    voice.pressure = pressure;
                }
                Ok(())
            }
            MidiMessageKind::PitchBend => self.apply_pitch_bend(source, voice_id, message),
            MidiMessageKind::AllNotesOff if voice_id.is_none() => {
                self.silence_channel(source, channel)
            }
            MidiMessageKind::AllNotesOff => Err(PreparedInstrumentError::DispatchRejected),
            MidiMessageKind::ProgramChange => Err(PreparedInstrumentError::UnsupportedMidiKind {
                kind: MidiMessageKind::ProgramChange,
            }),
        }
    }

    fn dispatch_scoped_voice_from(
        &mut self,
        source: MidiSource,
        id: MidiVoiceId,
        scope: MidiExpressionScope,
        message: MidiMessage,
        parameters: &RtPatchParameters,
    ) -> Result<(), PreparedInstrumentError> {
        if scope == MidiExpressionScope::Member {
            return self.dispatch_voice_from(source, Some(id), message, parameters);
        }
        if parameters.patch_id() != Some(self.patch_id)
            || BraidsScalarParameters::from_rt(parameters).is_none()
        {
            return Err(PreparedInstrumentError::DispatchRejected);
        }
        // Master data is explicitly tagged to an attack's stored member channel.
        // It never broadcasts or replaces that member's expression state.
        match message.kind() {
            MidiMessageKind::ControlChange
                if matches!(message.data1(), 6 | 38 | 64 | 100 | 101) =>
            {
                self.owned_controller(source, id, scope, message, parameters)
            }
            MidiMessageKind::PitchBend => self.owned_pitch_bend(source, id, scope, message),
            MidiMessageKind::ChannelPressure => {
                let index = self.owned_voice_index(source, id, message.channel())?;
                self.voices[index].master_pressure = f32::from(message.data1()) / 127.0;
                Ok(())
            }
            _ => Err(PreparedInstrumentError::UnsupportedExpressionScope),
        }
    }

    fn dispatch_initialized_voice_from(
        &mut self,
        source: MidiSource,
        id: MidiVoiceId,
        initial: OwnedVoiceInitialization,
        parameters: &RtPatchParameters,
    ) -> Result<(), PreparedInstrumentError> {
        if parameters.patch_id() != Some(self.patch_id) {
            return Err(PreparedInstrumentError::DispatchRejected);
        }
        let scalars = BraidsScalarParameters::from_rt(parameters)
            .ok_or(PreparedInstrumentError::DispatchRejected)?;
        let message = initial.message();
        let owned = OwnedExpression::from_initial(initial.member());
        let master = OwnedExpression::from_initial(initial.master());
        let bend = owned.bend_units(message.data1(), master)?;
        let mut voice = BraidsVoice::idle(message.channel());
        voice.source = source;
        voice.voice_id = Some(id);
        voice.note = Some(message.data1());
        voice.held = true;
        voice.velocity = f32::from(message.data2()) / 127.0;
        // Explicit initialization is independent of previous source/channel
        // controllers, including expression, pressure, bend and pedal state.
        voice.pressure = initial
            .member()
            .pressure()
            .map_or(1.0, |value| f32::from(value) / 127.0);
        voice.master_pressure = initial
            .master()
            .pressure()
            .map_or(1.0, |value| f32::from(value) / 127.0);
        voice.owned = owned;
        voice.master = master;
        voice.pitch_bend = bend;
        self.start_voice(voice, parameters, scalars)
    }

    fn render(
        &mut self,
        interleaved_stereo: &mut [f32],
        frame_count: usize,
        parameters: &RtPatchParameters,
    ) -> Result<(), crate::synth::PreparedInstrumentError> {
        if frame_count > self.max_frames
            || interleaved_stereo.len() != frame_count.saturating_mul(2)
        {
            return Err(PreparedInstrumentError::InvalidFrameCapacity);
        }
        interleaved_stereo[..frame_count * 2].fill(0.0);
        let Some(scalars) = BraidsScalarParameters::from_rt(parameters) else {
            return Err(PreparedInstrumentError::ScalarLayoutMismatch);
        };
        if parameters.patch_id() != Some(self.patch_id) {
            return Err(PreparedInstrumentError::DispatchRejected);
        }

        let mut host_offset = 0;
        while host_offset < frame_count {
            let host_frames = (frame_count - host_offset).min(HOST_FRAMES_PER_NATIVE_CHUNK);
            for voice_index in 0..self.voices.len() {
                if self
                    .render_voice_chunk(
                        voice_index,
                        interleaved_stereo,
                        host_offset,
                        host_frames,
                        scalars,
                    )
                    .is_err()
                {
                    interleaved_stereo.fill(0.0);
                    return Err(PreparedInstrumentError::RenderRejected);
                }
            }
            host_offset += host_frames;
        }

        Ok(())
    }

    fn all_notes_off(&mut self) {
        self.clear_all_voices();
        self.sustain.fill([false; MIDI_CHANNEL_COUNT]);
    }

    fn all_notes_off_from(&mut self, source: MidiSource) -> Result<(), PreparedInstrumentError> {
        self.sustain[source.index()].fill(false);
        for (index, voice) in self.voices.iter_mut().enumerate() {
            if voice.source == source {
                voice.clear();
                self.bank
                    .reset(index)
                    .map_err(|_| PreparedInstrumentError::DispatchRejected)?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PreparedBraidsConfig;

impl PreparedBraidsConfig {
    fn try_from_patch(patch: &Patch) -> Result<Self, InstrumentPreparationError> {
        let config = patch.instrument_config();
        if config.capability_id().as_str() != BRAIDS_CAPABILITY_ID
            || !config.asset_references().is_empty()
            || config.values().len() != 3
        {
            return Err(invalid_config(patch.id()));
        }
        let provider = BraidsCapability::new().map_err(|_| invalid_config(patch.id()))?;
        let canonical = provider
            .create_config(config.values(), config.asset_references())
            .map_err(|_| invalid_config(patch.id()))?;
        if canonical != *config {
            return Err(invalid_config(patch.id()));
        }

        let model_id = parameter_id(BRAIDS_MODEL_PARAMETER_ID, patch.id())?;
        let timbre_id = parameter_id(BRAIDS_TIMBRE_PARAMETER_ID, patch.id())?;
        let color_id = parameter_id(BRAIDS_COLOR_PARAMETER_ID, patch.id())?;
        let model_valid = matches!(
            config.value(&model_id),
            Some(ParameterValue::Choice(model)) if BRAIDS_MODELS.iter().any(|candidate| candidate.id == model)
        );
        let normalized = |id: &ParameterId| {
            matches!(
                config.value(id),
                Some(ParameterValue::Continuous(value)) if value.is_finite() && (0.0..=1.0).contains(value)
            )
        };
        if !model_valid || !normalized(&timbre_id) || !normalized(&color_id) {
            return Err(invalid_config(patch.id()));
        }
        Ok(Self)
    }
}

fn parameter_id(value: &str, patch_id: PatchId) -> Result<ParameterId, InstrumentPreparationError> {
    ParameterId::new(value).map_err(|_| invalid_config(patch_id))
}

const fn invalid_config(patch_id: PatchId) -> InstrumentPreparationError {
    InstrumentPreparationError::InvalidConfiguration { patch_id }
}

fn pitch_bend(lsb: u8, msb: u8) -> i16 {
    let value = i32::from(lsb) | (i32::from(msb) << 7);
    let units = (value - MIDI_BEND_CENTER)
        * (PITCH_UNITS_PER_SEMITONE * PITCH_BEND_RANGE_SEMITONES)
        / MIDI_BEND_CENTER;
    units.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

fn note_pitch(note: u8, bend: i16) -> i16 {
    (i32::from(note) * PITCH_UNITS_PER_SEMITONE + i32::from(bend)).clamp(0, i32::from(i16::MAX))
        as i16
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
    use crate::adapter::braids_capability::BraidsCapability;
    use crate::adapter::braids_native::{braids_lifecycle_counts, BRAIDS_INTERNAL_CHUNK_FRAMES};
    use crate::kernel::midi_channel::MidiChannel;
    use crate::mixer::patch_output::PatchOutput;
    use crate::real_time::parameter_snapshot::RtInstrumentParameters;
    use crate::synth::voice_envelope::VoiceEnvelope;

    fn patch(id: u32) -> Patch {
        Patch::new(
            PatchId::new(id).unwrap(),
            format!("Braids {id}"),
            BraidsCapability::new().unwrap().default_config().unwrap(),
            MidiChannel::new((id % 16) as u8).unwrap(),
            PatchOutput::default(),
        )
    }

    fn parameters(
        patch_id: PatchId,
        envelope: VoiceEnvelope,
        scalars: [f32; 3],
    ) -> RtPatchParameters {
        RtPatchParameters::projected(
            patch_id,
            PatchOutput::default(),
            envelope,
            RtInstrumentParameters::new(&scalars).unwrap(),
        )
    }

    fn message(kind: MidiMessageKind, data1: u8, data2: u8) -> MidiMessage {
        channel_message(1, kind, data1, data2)
    }

    fn channel_message(channel: u8, kind: MidiMessageKind, data1: u8, data2: u8) -> MidiMessage {
        MidiMessage::try_new(MidiChannel::new(channel).unwrap(), kind, data1, data2).unwrap()
    }

    fn owned_event(
        prepared: &mut PreparedBraidsInstrument,
        parameters: &RtPatchParameters,
        source: MidiSource,
        id: MidiVoiceId,
        kind: MidiMessageKind,
        data1: u8,
        data2: u8,
    ) {
        prepared
            .dispatch_voice_from(source, Some(id), message(kind, data1, data2), parameters)
            .unwrap();
    }

    fn assert_native_sum(
        mixed: &mut PreparedBraidsInstrument,
        parameters: &RtPatchParameters,
        references: &mut [PreparedBraidsInstrument; 3],
        reference_parameters: &[RtPatchParameters; 3],
    ) {
        let mut expected = [0.0_f32; 128];
        for (reference, parameters) in references.iter_mut().zip(reference_parameters) {
            let mut contribution = [0.0; 128];
            reference.render(&mut contribution, 64, parameters).unwrap();
            for (sum, sample) in expected.iter_mut().zip(contribution) {
                *sum += sample;
            }
        }
        let mut actual = [0.0; 128];
        mixed.render(&mut actual, 64, parameters).unwrap();
        assert!(actual
            .iter()
            .zip(expected)
            .all(|(actual, expected)| (*actual - expected).abs() < 1e-6));
    }

    fn scoped_event(
        prepared: &mut PreparedBraidsInstrument,
        parameters: &RtPatchParameters,
        source: MidiSource,
        id: MidiVoiceId,
        scope: MidiExpressionScope,
        event: MidiMessage,
    ) {
        prepared
            .dispatch_scoped_voice_from(source, id, scope, event, parameters)
            .unwrap();
    }

    #[test]
    fn owned_initialized_opposing_bends_match_native_voices_without_channel_inheritance() {
        use MidiMessageKind::{
            ChannelPressure, ControlChange as Cc, NoteOff, NoteOn, PitchBend, PolyPressure,
        };
        let patch = patch(1);
        let envelope = VoiceEnvelope::new(0.0, 0.0, 1.0, 20.0).unwrap();
        let base = parameters(patch.id(), envelope, [3.0, 0.5, 0.5]);
        let prepare = || {
            BraidsPreparer::new()
                .unwrap()
                .prepare_patch(&patch, BRAIDS_HOST_SAMPLE_RATE, 64)
                .unwrap()
        };
        let mut mixed = prepare();
        let mut references = [prepare(), prepare(), prepare()];
        let reference_parameters = [
            parameters(patch.id(), envelope, [3.0, 25.0 / 127.0, 0.5]),
            base.clone(),
            base.clone(),
        ];
        for source in [MidiSource::Scheduled, MidiSource::Monitor] {
            for event in [
                message(Cc, 11, 7),
                message(ChannelPressure, 5, 0),
                message(PitchBend, 0, 127),
                message(Cc, 64, 127),
            ] {
                mixed.dispatch_from(source, event, &base).unwrap();
            }
        }
        // Both individual bends are outside Braids' pitch interval. Their
        // exact sums are valid, so no ordering of ordinary bend updates works.
        assert_eq!(owned_pitch_bend_units(60, 14_336, 120, 0), None);
        assert_eq!(owned_pitch_bend_units(60, 2_048, 120, 0), None);
        let first = OwnedVoiceInitialization::new(
            message(NoteOn, 60, 100),
            OwnedExpressionState::new(14_336, 120, 0, Some(96), Some(25), 0).unwrap(),
            OwnedExpressionState::new(2_048, 120, 0, Some(64), None, 127).unwrap(),
        )
        .unwrap();
        let second = OwnedVoiceInitialization::new(
            message(NoteOn, 60, 100),
            OwnedExpressionState::new(14_336, 116, 0, None, None, 0).unwrap(),
            OwnedExpressionState::new(2_048, 112, 0, None, None, 0).unwrap(),
        )
        .unwrap();
        let monitor = OwnedVoiceInitialization::new(
            message(NoteOn, 60, 100),
            OwnedExpressionState::default(),
            OwnedExpressionState::default(),
        )
        .unwrap();
        for (source, id, initial) in [
            (MidiSource::Scheduled, 1, first),
            (MidiSource::Scheduled, 2, second),
            (MidiSource::Monitor, 1, monitor),
        ] {
            mixed
                .dispatch_initialized_voice_from(
                    source,
                    MidiVoiceId::new(id).unwrap(),
                    initial,
                    &base,
                )
                .unwrap();
        }
        for ((reference, parameters), key) in references
            .iter_mut()
            .zip(&reference_parameters)
            .zip([60, 63, 60])
        {
            reference
                .dispatch(message(NoteOn, key, 100), parameters)
                .unwrap();
        }
        for event in [
            message(ChannelPressure, 96, 0),
            message(PolyPressure, 60, 64),
        ] {
            references[0]
                .dispatch(event, &reference_parameters[0])
                .unwrap();
        }
        assert_native_sum(&mut mixed, &base, &mut references, &reference_parameters);
        assert_eq!(
            mixed.pressure[MidiSource::Scheduled.index()][1],
            5.0 / 127.0
        );
        assert_eq!(mixed.voices[0].owned.rpn_msb, None);
        assert_eq!(mixed.voices[0].master.rpn_lsb, None);
        for event in [message(PitchBend, 0, 64), message(Cc, 6, 48)] {
            let before = mixed.voices.clone();
            assert!(mixed
                .dispatch_voice_from(MidiSource::Scheduled, MidiVoiceId::new(1), event, &base)
                .is_err());
            assert_eq!(mixed.voices, before);
        }
        for (source, id) in [
            (MidiSource::Scheduled, 1),
            (MidiSource::Scheduled, 2),
            (MidiSource::Monitor, 1),
        ] {
            mixed
                .dispatch_voice_from(source, MidiVoiceId::new(id), message(NoteOff, 60, 0), &base)
                .unwrap();
        }
        references[1]
            .dispatch(message(NoteOff, 63, 0), &base)
            .unwrap();
        references[2]
            .dispatch(message(NoteOff, 60, 0), &base)
            .unwrap();
        assert_eq!(mixed.active_note_count(), Some(1));
        assert_native_sum(&mut mixed, &base, &mut references, &reference_parameters);
        mixed
            .dispatch_scoped_voice_from(
                MidiSource::Scheduled,
                MidiVoiceId::new(1).unwrap(),
                MidiExpressionScope::Master,
                message(Cc, 64, 0),
                &base,
            )
            .unwrap();
        references[0]
            .dispatch(message(NoteOff, 60, 0), &reference_parameters[0])
            .unwrap();
        assert_eq!(mixed.active_note_count(), Some(0));
        assert_native_sum(&mut mixed, &base, &mut references, &reference_parameters);
    }

    #[test]
    fn owned_initialized_admission_rejects_before_partial_attack_or_existing_voice_mutation() {
        let patch = patch(1).with_voice_limit(1).unwrap();
        let base = parameters(patch.id(), VoiceEnvelope::DEFAULT, [3.0, 0.5, 0.5]);
        let mut prepared = BraidsPreparer::new()
            .unwrap()
            .prepare_patch(&patch, BRAIDS_HOST_SAMPLE_RATE, 64)
            .unwrap();
        let attack = message(MidiMessageKind::NoteOn, 60, 100);
        let positive = OwnedExpressionState::new(14_336, 120, 0, None, None, 0).unwrap();
        let invalid = OwnedVoiceInitialization::new(attack, positive, positive).unwrap();
        let neutral = OwnedVoiceInitialization::new(
            attack,
            OwnedExpressionState::default(),
            OwnedExpressionState::default(),
        )
        .unwrap();
        let before = prepared.voices.clone();
        let first = MidiVoiceId::new(1).unwrap();
        assert!(prepared
            .dispatch_initialized_voice_from(MidiSource::Scheduled, first, invalid, &base)
            .is_err());
        assert_eq!(prepared.voices, before);
        assert_eq!(prepared.next_age, 1);
        assert_eq!(prepared.active_note_count(), Some(0));
        let mut output = [1.0; 128];
        prepared.render(&mut output, 64, &base).unwrap();
        assert!(output.iter().all(|sample| *sample == 0.0));
        let wrong_patch = parameters(
            PatchId::new(99).unwrap(),
            VoiceEnvelope::DEFAULT,
            [3.0, 0.5, 0.5],
        );
        assert!(prepared
            .dispatch_initialized_voice_from(MidiSource::Scheduled, first, neutral, &wrong_patch)
            .is_err());
        assert_eq!(prepared.voices, before);
        prepared
            .dispatch_initialized_voice_from(MidiSource::Scheduled, first, neutral, &base)
            .unwrap();
        for id in [first, MidiVoiceId::new(2).unwrap()] {
            let before = prepared.voices.clone();
            assert!(prepared
                .dispatch_initialized_voice_from(MidiSource::Scheduled, id, neutral, &base)
                .is_err());
            assert_eq!(prepared.voices, before);
            assert_eq!(prepared.next_age, 2);
        }
    }

    #[test]
    fn owned_master_member_pitch_and_pressure_match_independent_native_voices() {
        use MidiExpressionScope::{Master, Member};
        use MidiMessageKind::{
            ChannelPressure, ControlChange as Cc, NoteOn, PitchBend, PolyPressure,
        };
        let patch = patch(1);
        let envelope = VoiceEnvelope::new(0.0, 0.0, 1.0, 20.0).unwrap();
        let base = parameters(patch.id(), envelope, [3.0, 0.5, 0.5]);
        let prepare = || {
            BraidsPreparer::new()
                .unwrap()
                .prepare_patch(&patch, BRAIDS_HOST_SAMPLE_RATE, 64)
                .unwrap()
        };
        let mut mixed = prepare();
        let mut references = [prepare(), prepare(), prepare()];
        let first = MidiVoiceId::new(1).unwrap();
        let second = MidiVoiceId::new(2).unwrap();
        for (source, id) in [
            (MidiSource::Scheduled, first),
            (MidiSource::Scheduled, second),
            (MidiSource::Monitor, first),
        ] {
            scoped_event(
                &mut mixed,
                &base,
                source,
                id,
                Member,
                message(NoteOn, 60, 100),
            );
        }
        for (id, scope, range) in [
            (first, Member, 12),
            (first, Master, 4),
            (second, Master, 12),
        ] {
            for (controller, value) in [(101, 0), (100, 0), (6, range)] {
                scoped_event(
                    &mut mixed,
                    &base,
                    MidiSource::Scheduled,
                    id,
                    scope,
                    message(Cc, controller, value),
                );
            }
        }
        for (id, scope, msb) in [
            (first, Member, 96),
            (first, Master, 32),
            (second, Member, 32),
            (second, Master, 96),
        ] {
            scoped_event(
                &mut mixed,
                &base,
                MidiSource::Scheduled,
                id,
                scope,
                message(PitchBend, 0, msb),
            );
        }
        for (id, scope, pressure) in [
            (first, Member, 96),
            (first, Master, 64),
            (second, Member, 32),
            (second, Master, 100),
        ] {
            scoped_event(
                &mut mixed,
                &base,
                MidiSource::Scheduled,
                id,
                scope,
                message(ChannelPressure, pressure, 0),
            );
        }
        scoped_event(
            &mut mixed,
            &base,
            MidiSource::Scheduled,
            first,
            Member,
            message(PolyPressure, 60, 80),
        );
        // Independent ordinary voices: 60 + 6 - 2 = 64; 60 - 1 + 6 = 65.
        // Their ordinary expression/pressure/key-pressure lanes provide the
        // same gain factors without using any scoped expression implementation.
        for (reference, key) in references.iter_mut().zip([64, 65, 60]) {
            reference
                .dispatch(message(NoteOn, key, 100), &base)
                .unwrap();
        }
        for event in [
            message(Cc, 11, 96),
            message(ChannelPressure, 80, 0),
            message(PolyPressure, 64, 64),
        ] {
            references[0].dispatch(event, &base).unwrap();
        }
        for event in [
            message(ChannelPressure, 32, 0),
            message(PolyPressure, 65, 100),
        ] {
            references[1].dispatch(event, &base).unwrap();
        }
        let reference_parameters = [base.clone(), base.clone(), base.clone()];
        assert_native_sum(&mut mixed, &base, &mut references, &reference_parameters);
        scoped_event(
            &mut mixed,
            &base,
            MidiSource::Scheduled,
            first,
            Master,
            message(Cc, 6, 8),
        );
        references[0]
            .dispatch(message(PitchBend, 0, 0), &base)
            .unwrap();
        assert_native_sum(&mut mixed, &base, &mut references, &reference_parameters);
        for event in [message(Cc, 6, 14), message(Cc, 38, 50)] {
            scoped_event(
                &mut mixed,
                &base,
                MidiSource::Scheduled,
                first,
                Member,
                event,
            );
        }
        // The stored +half member bend now contributes 7.25; master remains -4.
        references[0]
            .dispatch(message(PitchBend, 0, 40), &base)
            .unwrap();
        assert_native_sum(&mut mixed, &base, &mut references, &reference_parameters);
        assert_eq!(mixed.voices[0].owned.bend_raw, 12_288);
        assert_eq!(mixed.voices[0].master.bend_raw, 4_096);
        assert_eq!(mixed.voices[0].master.bend_semitones, 8);
        assert_eq!(mixed.voices[1].master.bend_semitones, 12);
        assert_eq!(mixed.voices[2].master_pressure, 1.0);
    }

    #[test]
    fn owned_master_member_sustain_survives_member_reuse_and_releases_only_after_both_pedals() {
        use MidiExpressionScope::{Master, Member};
        use MidiMessageKind::{ControlChange as Cc, NoteOff, NoteOn};
        let patch = patch(1);
        let base = parameters(
            patch.id(),
            VoiceEnvelope::new(0.0, 0.0, 1.0, 20.0).unwrap(),
            [3.0, 0.5, 0.5],
        );
        let prepare = || {
            BraidsPreparer::new()
                .unwrap()
                .prepare_patch(&patch, BRAIDS_HOST_SAMPLE_RATE, 64)
                .unwrap()
        };
        let mut mixed = prepare();
        let mut references = [prepare(), prepare(), prepare()];
        let first = MidiVoiceId::new(1).unwrap();
        let second = MidiVoiceId::new(2).unwrap();
        let reference_parameters = [base.clone(), base.clone(), base.clone()];
        // First attack is physically released under master pedal before its
        // member channel is reused. A third voice reuses the ID in Monitor.
        for ((source, id, pedal), reference) in [
            (MidiSource::Scheduled, first, Master),
            (MidiSource::Scheduled, second, Member),
            (MidiSource::Monitor, first, Master),
        ]
        .into_iter()
        .zip(&mut references)
        {
            scoped_event(
                &mut mixed,
                &base,
                source,
                id,
                Member,
                message(NoteOn, 60, 100),
            );
            scoped_event(&mut mixed, &base, source, id, pedal, message(Cc, 64, 127));
            scoped_event(
                &mut mixed,
                &base,
                source,
                id,
                Member,
                message(NoteOff, 60, 0),
            );
            reference.dispatch(message(NoteOn, 60, 100), &base).unwrap();
        }
        mixed
            .dispatch_from(MidiSource::Scheduled, message(Cc, 64, 0), &base)
            .unwrap();
        scoped_event(
            &mut mixed,
            &base,
            MidiSource::Scheduled,
            first,
            Member,
            message(Cc, 64, 0),
        );
        scoped_event(
            &mut mixed,
            &base,
            MidiSource::Scheduled,
            second,
            Master,
            message(Cc, 64, 0),
        );
        assert_eq!(mixed.active_note_count(), Some(3));
        assert_native_sum(&mut mixed, &base, &mut references, &reference_parameters);
        for (index, source, id, pedal) in [
            (0, MidiSource::Scheduled, first, Master),
            (1, MidiSource::Scheduled, second, Member),
            (2, MidiSource::Monitor, first, Master),
        ] {
            scoped_event(&mut mixed, &base, source, id, pedal, message(Cc, 64, 0));
            references[index]
                .dispatch(message(NoteOff, 60, 0), &base)
                .unwrap();
            assert_eq!(mixed.active_note_count(), Some(2 - index as u32));
            // Pedal-down during a release tail cannot revive the attack.
            scoped_event(&mut mixed, &base, source, id, pedal, message(Cc, 64, 127));
            assert_eq!(mixed.active_note_count(), Some(2 - index as u32));
            assert_native_sum(&mut mixed, &base, &mut references, &reference_parameters);
        }
        for _ in 0..16 {
            assert_native_sum(&mut mixed, &base, &mut references, &reference_parameters);
        }
        assert_eq!(mixed.active_voice_count(), 0);
    }

    #[test]
    fn owned_master_rejections_preserve_both_dimensions_and_exact_pitch_sums() {
        use MidiExpressionScope::{Master, Member};
        use MidiMessageKind::{
            ChannelPressure, ControlChange as Cc, NoteOff, NoteOn, PitchBend, PolyPressure,
            ProgramChange,
        };
        // Each contribution is 0.53125 native units; quantizing separately
        // would lose both. Opposite bends may cancel even at a pitch boundary.
        assert_eq!(
            owned_combined_pitch_bend_units(60, (8_209, 2, 0), (8_209, 2, 0)),
            Some(1)
        );
        assert_eq!(
            owned_combined_pitch_bend_units(0, (0, 2, 0), (12_288, 4, 0)),
            Some(0)
        );
        assert_eq!(
            owned_combined_pitch_bend_units(0, (8_191, 2, 0), (8_192, 2, 0)),
            None
        );
        assert_eq!(
            owned_combined_pitch_bend_units(127, (12_288, 1, 0), (12_224, 1, 0)),
            Some(127)
        );
        assert_eq!(
            owned_combined_pitch_bend_units(127, (12_288, 1, 0), (12_225, 1, 0)),
            None
        );
        let patch = patch(1);
        let base = parameters(patch.id(), VoiceEnvelope::DEFAULT, [3.0, 0.5, 0.5]);
        let mut prepared = BraidsPreparer::new()
            .unwrap()
            .prepare_patch(&patch, BRAIDS_HOST_SAMPLE_RATE, 64)
            .unwrap();
        let id = MidiVoiceId::new(1).unwrap();
        let source = MidiSource::Scheduled;
        scoped_event(
            &mut prepared,
            &base,
            source,
            id,
            Member,
            message(NoteOn, 60, 100),
        );
        for scope in [Member, Master] {
            for event in [
                message(Cc, 101, 0),
                message(Cc, 100, 0),
                message(Cc, 6, 100),
            ] {
                scoped_event(&mut prepared, &base, source, id, scope, event);
            }
        }
        scoped_event(
            &mut prepared,
            &base,
            source,
            id,
            Master,
            message(PitchBend, 0, 96),
        );
        for (scope, event) in [
            (Member, message(PitchBend, 0, 96)),
            (Master, message(PitchBend, 127, 127)),
            (Master, message(Cc, 38, 100)),
            (Master, message(Cc, 101, 1)),
            (Master, message(Cc, 74, 50)),
            (Master, message(Cc, 7, 50)),
            (Master, message(Cc, 120, 0)),
            (Master, message(NoteOn, 60, 100)),
            (Master, message(NoteOff, 60, 0)),
            (Master, message(PolyPressure, 60, 50)),
            (Master, message(ProgramChange, 1, 0)),
            (Master, channel_message(2, ChannelPressure, 50, 0)),
        ] {
            let before = prepared.voices.clone();
            assert!(prepared
                .dispatch_scoped_voice_from(source, id, scope, event, &base)
                .is_err());
            assert_eq!(prepared.voices, before);
        }
        for (source, id) in [
            (MidiSource::Monitor, id),
            (source, MidiVoiceId::new(99).unwrap()),
        ] {
            let before = prepared.voices.clone();
            assert!(prepared
                .dispatch_scoped_voice_from(
                    source,
                    id,
                    Master,
                    message(ChannelPressure, 50, 0),
                    &base
                )
                .is_err());
            assert_eq!(prepared.voices, before);
        }
        scoped_event(
            &mut prepared,
            &base,
            source,
            id,
            Member,
            message(PitchBend, 0, 32),
        );
        assert_eq!(prepared.voices[0].pitch_bend, 0);
        assert_eq!(prepared.voices[0].master.bend_raw, 12_288);
        assert_eq!(prepared.voices[0].owned.bend_raw, 4_096);
    }

    #[test]
    fn owned_rpn_bend_and_absolute_timbre_match_independent_native_voices() {
        use MidiMessageKind::{ControlChange as Cc, NoteOn, PitchBend};
        let patch = patch(1);
        let envelope = VoiceEnvelope::new(0.0, 0.0, 1.0, 20.0).unwrap();
        let base = parameters(patch.id(), envelope, [3.0, 0.5, 0.5]);
        let prepare = || {
            BraidsPreparer::new()
                .unwrap()
                .prepare_patch(&patch, BRAIDS_HOST_SAMPLE_RATE, 64)
                .unwrap()
        };
        let mut mixed = prepare();
        let mut references = [prepare(), prepare(), prepare()];
        let first = MidiVoiceId::new(1).unwrap();
        let second = MidiVoiceId::new(2).unwrap();
        for (source, id) in [
            (MidiSource::Scheduled, first),
            (MidiSource::Scheduled, second),
            (MidiSource::Monitor, first),
        ] {
            owned_event(&mut mixed, &base, source, id, NoteOn, 60, 100);
        }
        for (controller, value) in [(101, 0), (100, 0), (6, 12), (74, 25)] {
            owned_event(
                &mut mixed,
                &base,
                MidiSource::Scheduled,
                first,
                Cc,
                controller,
                value,
            );
        }
        owned_event(
            &mut mixed,
            &base,
            MidiSource::Scheduled,
            first,
            PitchBend,
            0,
            96,
        );
        owned_event(
            &mut mixed,
            &base,
            MidiSource::Scheduled,
            second,
            PitchBend,
            0,
            32,
        );
        owned_event(
            &mut mixed,
            &base,
            MidiSource::Scheduled,
            second,
            Cc,
            74,
            110,
        );
        let mut reference_parameters = [
            parameters(patch.id(), envelope, [3.0, 25.0 / 127.0, 0.5]),
            parameters(patch.id(), envelope, [3.0, 110.0 / 127.0, 0.5]),
            parameters(patch.id(), envelope, [3.0, 0.5, 0.5]),
        ];
        // These reference pitches are independent of the owned RPN code:
        // note 60 with +half of 12 semitones is 66; -half of 2 is 59.
        for ((reference, parameters), key) in references
            .iter_mut()
            .zip(&reference_parameters)
            .zip([66, 59, 60])
        {
            reference
                .dispatch(message(NoteOn, key, 100), parameters)
                .unwrap();
        }
        assert_native_sum(&mut mixed, &base, &mut references, &reference_parameters);
        owned_event(&mut mixed, &base, MidiSource::Scheduled, first, Cc, 6, 14);
        owned_event(&mut mixed, &base, MidiSource::Scheduled, first, Cc, 38, 50);
        // Raw bend stays 12288. Range 14.50 produces +7.25 semitones;
        // ordinary note 66 with +1.25 is the same absolute native pitch.
        assert_eq!(mixed.voices[0].owned.bend_raw, 12_288);
        assert_eq!(mixed.voices[0].pitch_bend, 7 * 128 + 32);
        references[0]
            .dispatch(message(PitchBend, 0, 104), &reference_parameters[0])
            .unwrap();
        assert_native_sum(&mut mixed, &base, &mut references, &reference_parameters);
        owned_event(&mut mixed, &base, MidiSource::Scheduled, first, Cc, 74, 0);
        reference_parameters[0] = parameters(patch.id(), envelope, [3.0, 0.0, 0.5]);
        let changed = parameters(patch.id(), envelope, [3.0, 1.0, 0.5]);
        reference_parameters[2] = parameters(patch.id(), envelope, [3.0, 1.0, 0.5]);
        assert_native_sum(&mut mixed, &changed, &mut references, &reference_parameters);
        assert_eq!(mixed.voices[1].owned.bend_semitones, 2);
        assert_eq!(mixed.voices[2].owned.timbre, None);
        assert_eq!(mixed.pitch_bend[MidiSource::Scheduled.index()][1], 0);
    }

    #[test]
    fn owned_sustain_survives_member_reuse_channel_pedal_and_latest_controls() {
        use MidiMessageKind::{ChannelPressure, ControlChange as Cc, NoteOff, NoteOn, PitchBend};
        let patch = patch(1);
        let envelope = VoiceEnvelope::new(0.0, 0.0, 1.0, 20.0).unwrap();
        let base = parameters(patch.id(), envelope, [3.0, 0.5, 0.5]);
        let prepare = || {
            BraidsPreparer::new()
                .unwrap()
                .prepare_patch(&patch, BRAIDS_HOST_SAMPLE_RATE, 64)
                .unwrap()
        };
        let mut mixed = prepare();
        let mut references = [prepare(), prepare(), prepare()];
        let mut reference_parameters = [base.clone(), base.clone(), base.clone()];
        let first = MidiVoiceId::new(1).unwrap();
        let second = MidiVoiceId::new(2).unwrap();
        for ((source, id), index) in [(MidiSource::Scheduled, first), (MidiSource::Monitor, first)]
            .into_iter()
            .zip([0, 2])
        {
            let reference = &mut references[index];
            owned_event(&mut mixed, &base, source, id, NoteOn, 60, 100);
            owned_event(&mut mixed, &base, source, id, Cc, 64, 127);
            owned_event(&mut mixed, &base, source, id, NoteOff, 60, 0);
            reference.dispatch(message(NoteOn, 60, 100), &base).unwrap();
            reference.dispatch(message(Cc, 64, 127), &base).unwrap();
            reference.dispatch(message(NoteOff, 60, 0), &base).unwrap();
        }
        // Reuse the member channel only after the first attack's physical release.
        owned_event(
            &mut mixed,
            &base,
            MidiSource::Scheduled,
            second,
            NoteOn,
            60,
            100,
        );
        references[1]
            .dispatch(message(NoteOn, 60, 100), &base)
            .unwrap();
        owned_event(&mut mixed, &base, MidiSource::Scheduled, second, Cc, 64, 0);
        for value in [0, 127, 0] {
            mixed
                .dispatch_from(MidiSource::Scheduled, message(Cc, 64, value), &base)
                .unwrap();
        }
        assert_eq!(mixed.active_note_count(), Some(3));
        for event in [message(PitchBend, 0, 96), message(ChannelPressure, 72, 0)] {
            mixed
                .dispatch_voice_from(MidiSource::Scheduled, Some(first), event, &base)
                .unwrap();
            references[0].dispatch(event, &base).unwrap();
        }
        owned_event(&mut mixed, &base, MidiSource::Scheduled, first, Cc, 74, 127);
        reference_parameters[0] = parameters(patch.id(), envelope, [3.0, 1.0, 0.5]);
        assert_native_sum(&mut mixed, &base, &mut references, &reference_parameters);
        // Member channel now serves the second attack; its pedal-up cannot
        // release the first attack, even with an intervening channel pedal-down.
        mixed
            .dispatch_from(MidiSource::Scheduled, message(Cc, 64, 127), &base)
            .unwrap();
        owned_event(
            &mut mixed,
            &base,
            MidiSource::Scheduled,
            second,
            NoteOff,
            60,
            0,
        );
        references[1]
            .dispatch(message(NoteOff, 60, 0), &base)
            .unwrap();
        assert_eq!(mixed.active_note_count(), Some(2));
        assert_native_sum(&mut mixed, &base, &mut references, &reference_parameters);
        owned_event(&mut mixed, &base, MidiSource::Scheduled, first, Cc, 64, 0);
        references[0].dispatch(message(Cc, 64, 0), &base).unwrap();
        assert_eq!(mixed.active_note_count(), Some(1));
        assert_native_sum(&mut mixed, &base, &mut references, &reference_parameters);
        mixed.all_notes_off_from(MidiSource::Scheduled).unwrap();
        references[0].all_notes_off();
        references[1].all_notes_off();
        assert_native_sum(&mut mixed, &base, &mut references, &reference_parameters);
        assert_eq!(mixed.active_note_count(), Some(1));
        owned_event(
            &mut mixed,
            &base,
            MidiSource::Scheduled,
            first,
            NoteOn,
            60,
            100,
        );
        assert_eq!(
            mixed.voices[0].owned,
            OwnedExpression::new(MIDI_BEND_CENTER as u16)
        );
        mixed.all_notes_off_from(MidiSource::Monitor).unwrap();
        assert_eq!(mixed.active_note_count(), Some(1));
        assert!(mixed.voices[0].held);
    }

    #[test]
    fn owned_invalid_rpn_and_pitch_bounds_reject_without_state_changes() {
        use MidiMessageKind::{ControlChange as Cc, NoteOn, PitchBend};
        let patch = patch(1);
        let parameters = parameters(patch.id(), VoiceEnvelope::DEFAULT, [3.0, 0.5, 0.5]);
        let mut prepared = BraidsPreparer::new()
            .unwrap()
            .prepare_patch(&patch, BRAIDS_HOST_SAMPLE_RATE, 64)
            .unwrap();
        let id = MidiVoiceId::new(1).unwrap();
        owned_event(
            &mut prepared,
            &parameters,
            MidiSource::Scheduled,
            id,
            NoteOn,
            60,
            100,
        );
        for event in [message(Cc, 6, 48), message(Cc, 38, 50), message(Cc, 101, 1)] {
            let before = prepared.voices.clone();
            assert!(prepared
                .dispatch_voice_from(MidiSource::Scheduled, Some(id), event, &parameters)
                .is_err());
            assert_eq!(prepared.voices, before);
        }
        for (controller, value) in [(101, 0), (100, 0), (6, 48)] {
            owned_event(
                &mut prepared,
                &parameters,
                MidiSource::Scheduled,
                id,
                Cc,
                controller,
                value,
            );
        }
        owned_event(
            &mut prepared,
            &parameters,
            MidiSource::Scheduled,
            id,
            PitchBend,
            127,
            127,
        );
        for event in [
            message(Cc, 6, 100),
            message(Cc, 38, 100),
            message(Cc, 100, 1),
            message(NoteOn, 61, 100),
        ] {
            let before = prepared.voices.clone();
            assert!(prepared
                .dispatch_voice_from(MidiSource::Scheduled, Some(id), event, &parameters)
                .is_err());
            assert_eq!(prepared.voices, before);
        }
        owned_event(
            &mut prepared,
            &parameters,
            MidiSource::Scheduled,
            id,
            Cc,
            101,
            127,
        );
        let before = prepared.voices.clone();
        assert!(prepared
            .dispatch_voice_from(
                MidiSource::Scheduled,
                Some(id),
                message(Cc, 6, 2),
                &parameters
            )
            .is_err());
        assert_eq!(prepared.voices, before);
        prepared.all_notes_off_from(MidiSource::Scheduled).unwrap();
        for (key, lsb, msb) in [(0, 0, 0), (0, 127, 63), (127, 127, 127)] {
            owned_event(
                &mut prepared,
                &parameters,
                MidiSource::Scheduled,
                id,
                NoteOn,
                key,
                100,
            );
            let before = prepared.voices.clone();
            assert!(prepared
                .dispatch_voice_from(
                    MidiSource::Scheduled,
                    Some(id),
                    message(PitchBend, lsb, msb),
                    &parameters
                )
                .is_err());
            assert_eq!(prepared.voices, before);
            // Raw channel updates also preflight all owned recipients.
            assert!(prepared
                .dispatch_from(
                    MidiSource::Scheduled,
                    message(PitchBend, lsb, msb),
                    &parameters
                )
                .is_err());
            assert_eq!(prepared.voices, before);
            assert_eq!(
                prepared.pitch_bend_raw[MidiSource::Scheduled.index()][1],
                MIDI_BEND_CENTER as u16
            );
            prepared.all_notes_off_from(MidiSource::Scheduled).unwrap();
        }
        // A new owned attack cannot inherit a clipped pitch. The ordinary
        // channel path retains its existing behavior at the same MIDI limit.
        prepared
            .dispatch_from(
                MidiSource::Scheduled,
                message(PitchBend, 127, 127),
                &parameters,
            )
            .unwrap();
        let before = prepared.voices.clone();
        assert!(prepared
            .dispatch_voice_from(
                MidiSource::Scheduled,
                Some(id),
                message(NoteOn, 127, 100),
                &parameters
            )
            .is_err());
        assert_eq!(prepared.voices, before);
        prepared
            .dispatch_from(
                MidiSource::Scheduled,
                message(NoteOn, 127, 100),
                &parameters,
            )
            .unwrap();
    }

    #[test]
    fn braids_preparer_requires_exact_rate_config_and_frame_capacity() {
        let preparer = BraidsPreparer::new().unwrap();
        let patch = patch(1);
        assert!(matches!(
            preparer.prepare(&patch, 44_100.0, 64),
            Err(InstrumentPreparationError::InvalidSampleRate)
        ));
        assert!(matches!(
            preparer.prepare(&patch, BRAIDS_HOST_SAMPLE_RATE, 0),
            Err(InstrumentPreparationError::InvalidFrameCapacity)
        ));

        let wrong = Patch::new(
            PatchId::new(2).unwrap(),
            "Wrong".to_owned(),
            crate::synth::instrument_capability::InstrumentConfig::from_parts(
                CapabilityId::new(BRAIDS_CAPABILITY_ID).unwrap(),
                patch.instrument_config().values()[..2].to_vec(),
                Vec::new(),
            ),
            MidiChannel::new(2).unwrap(),
            PatchOutput::default(),
        );
        assert!(matches!(
            preparer.prepare(&wrong, BRAIDS_HOST_SAMPLE_RATE, 64),
            Err(InstrumentPreparationError::InvalidConfiguration { .. })
        ));
    }

    #[test]
    fn braids_preparer_owns_one_bank_and_renders_finite_nonzero_audio_in_24_sample_chunks() {
        let before = braids_lifecycle_counts();
        let patch = patch(1);
        let mut prepared = BraidsPreparer::new()
            .unwrap()
            .prepare_patch(&patch, BRAIDS_HOST_SAMPLE_RATE, 64)
            .unwrap();
        let during = braids_lifecycle_counts();
        assert!(during.created > before.created);

        let parameters = parameters(patch.id(), VoiceEnvelope::DEFAULT, [0.0, 0.5, 0.5]);
        prepared
            .dispatch(message(MidiMessageKind::NoteOn, 60, 127), &parameters)
            .unwrap();
        let mut output = [0.0_f32; 128];
        prepared.render(&mut output, 64, &parameters).unwrap();
        assert!(output.iter().all(|sample| sample.is_finite()));
        assert!(output.iter().any(|sample| sample.abs() > 0.000_001));
        assert_eq!(
            prepared.maximum_native_chunk_observed,
            BRAIDS_INTERNAL_CHUNK_FRAMES
        );
        drop(prepared);
        assert!(braids_lifecycle_counts().destroyed > before.destroyed);
    }

    #[test]
    fn prepared_voice_capacity_refuses_exhaustion_without_stealing() {
        let patch = patch(1).with_voice_limit(16).unwrap();
        let mut prepared = BraidsPreparer::new()
            .unwrap()
            .prepare_patch(&patch, BRAIDS_HOST_SAMPLE_RATE, 64)
            .unwrap();
        let parameters = parameters(patch.id(), VoiceEnvelope::DEFAULT, [0.0, 0.5, 0.5]);
        for note in 48..64 {
            prepared
                .dispatch(message(MidiMessageKind::NoteOn, note, 100), &parameters)
                .unwrap();
        }
        assert_eq!(prepared.active_voice_count(), 16);
        assert_eq!(prepared.voices[0].note, Some(48));

        assert!(prepared
            .dispatch(message(MidiMessageKind::NoteOn, 80, 100), &parameters)
            .is_err());
        assert_eq!(prepared.active_voice_count(), 16);
        assert_eq!(prepared.voices[0].note, Some(48));
        assert!(prepared.voices.iter().all(|voice| voice.note != Some(80)));

        prepared
            .dispatch(message(MidiMessageKind::AllNotesOff, 0, 0), &parameters)
            .unwrap();
        assert_eq!(prepared.active_voice_count(), 0);
    }

    #[test]
    fn overlapping_notes_release_independently_and_midi_expression_is_local() {
        let patch = patch(1);
        let envelope = VoiceEnvelope::new(0.0, 0.0, 1.0, 10.0).unwrap();
        let parameters = parameters(patch.id(), envelope, [0.0, 0.5, 0.5]);
        let mut prepared = BraidsPreparer::new()
            .unwrap()
            .prepare_patch(&patch, BRAIDS_HOST_SAMPLE_RATE, 64)
            .unwrap();
        prepared
            .dispatch(message(MidiMessageKind::NoteOn, 60, 127), &parameters)
            .unwrap();
        prepared
            .dispatch(message(MidiMessageKind::NoteOn, 64, 127), &parameters)
            .unwrap();
        prepared
            .dispatch(message(MidiMessageKind::NoteOff, 60, 0), &parameters)
            .unwrap();
        assert!(prepared.voices[0].envelope.is_releasing());
        assert!(!prepared.voices[1].envelope.is_releasing());

        prepared
            .dispatch(message(MidiMessageKind::ControlChange, 11, 64), &parameters)
            .unwrap();
        prepared
            .dispatch(message(MidiMessageKind::PitchBend, 127, 127), &parameters)
            .unwrap();
        assert!(
            (prepared.expression[MidiSource::Immediate.index()][1] - (64.0 / 127.0)).abs()
                < f32::EPSILON
        );
        assert!(prepared.pitch_bend[MidiSource::Immediate.index()][1] > 0);
        assert!(matches!(
            prepared.dispatch(message(MidiMessageKind::ProgramChange, 1, 0), &parameters),
            Err(PreparedInstrumentError::UnsupportedMidiKind {
                kind: MidiMessageKind::ProgramChange
            })
        ));
    }

    #[test]
    fn scheduled_source_cancel_preserves_live_same_pitch_sustain_and_controls() {
        let patch = patch(1);
        let envelope = VoiceEnvelope::new(0.0, 0.0, 1.0, 10.0).unwrap();
        let parameters = parameters(patch.id(), envelope, [0.0, 0.5, 0.5]);
        let mut prepared = BraidsPreparer::new()
            .unwrap()
            .prepare_patch(&patch, BRAIDS_HOST_SAMPLE_RATE, 64)
            .unwrap();
        prepared
            .dispatch(
                message(MidiMessageKind::ControlChange, 64, 127),
                &parameters,
            )
            .unwrap();
        for source in [MidiSource::Immediate, MidiSource::Scheduled] {
            prepared
                .dispatch_from(
                    source,
                    message(MidiMessageKind::NoteOn, 60, 127),
                    &parameters,
                )
                .unwrap();
        }
        for control in [
            message(MidiMessageKind::ControlChange, 11, 64),
            message(MidiMessageKind::ChannelPressure, 32, 0),
            message(MidiMessageKind::PitchBend, 127, 127),
            message(MidiMessageKind::ControlChange, 64, 127),
            message(MidiMessageKind::NoteOff, 60, 0),
        ] {
            prepared
                .dispatch_from(MidiSource::Scheduled, control, &parameters)
                .unwrap();
        }

        let live = MidiSource::Immediate.index();
        let scheduled = MidiSource::Scheduled.index();
        assert_eq!(prepared.expression[live][1], 1.0);
        assert_eq!(prepared.pressure[live][1], 1.0);
        assert_eq!(prepared.pitch_bend[live][1], 0);
        assert!(prepared.expression[scheduled][1] < 1.0);
        assert!(prepared.pressure[scheduled][1] < 1.0);
        assert!(prepared.pitch_bend[scheduled][1] > 0);
        assert!(prepared.voices[0].held);
        assert!(!prepared.voices[1].held);
        assert!(!prepared.voices[1].envelope.is_releasing());

        prepared
            .dispatch(message(MidiMessageKind::NoteOff, 60, 0), &parameters)
            .unwrap();
        assert_eq!(prepared.active_voice_count(), 2);
        prepared.all_notes_off_from(MidiSource::Scheduled).unwrap();
        assert_eq!(prepared.active_voice_count(), 1);
        assert!(prepared.sustain[live][1]);
        assert!(!prepared.sustain[scheduled][1]);
        assert_eq!(prepared.voices[0].source, MidiSource::Immediate);
        assert!(!prepared.voices[0].held);
        assert!(!prepared.voices[0].envelope.is_releasing());

        let mut output = [0.0; 128];
        prepared.render(&mut output, 64, &parameters).unwrap();
        assert!(output.iter().all(|sample| sample.is_finite()));
        assert!(output.iter().any(|sample| sample.abs() > 0.000_001));
        prepared
            .dispatch(message(MidiMessageKind::ControlChange, 64, 0), &parameters)
            .unwrap();
        assert!(prepared.voices[0].envelope.is_releasing());
        for _ in 0..8 {
            prepared.render(&mut output, 64, &parameters).unwrap();
        }
        assert_eq!(prepared.active_voice_count(), 0);
        prepared.render(&mut output, 64, &parameters).unwrap();
        assert!(output.iter().all(|sample| *sample == 0.0));
    }

    #[test]
    fn same_pitch_channel_controls_sustain_and_cancel_preserve_other_owners() {
        let patch = patch(1);
        let parameters = parameters(
            patch.id(),
            VoiceEnvelope::new(0.0, 0.0, 1.0, 10.0).unwrap(),
            [3.0, 0.5, 0.5],
        );
        let mut prepared = BraidsPreparer::new()
            .unwrap()
            .prepare_patch(&patch, BRAIDS_HOST_SAMPLE_RATE, 64)
            .unwrap();
        prepared
            .dispatch(message(MidiMessageKind::NoteOn, 60, 127), &parameters)
            .unwrap();
        for (channel, note) in [(1, 60), (15, 60), (15, 62)] {
            prepared
                .dispatch_from(
                    MidiSource::Scheduled,
                    channel_message(channel, MidiMessageKind::NoteOn, note, 127),
                    &parameters,
                )
                .unwrap();
        }
        for control in [
            channel_message(15, MidiMessageKind::ControlChange, 11, 64),
            channel_message(15, MidiMessageKind::ChannelPressure, 32, 0),
            channel_message(15, MidiMessageKind::PitchBend, 127, 127),
            channel_message(15, MidiMessageKind::ControlChange, 64, 127),
            channel_message(15, MidiMessageKind::PolyPressure, 60, 32),
        ] {
            prepared
                .dispatch_from(MidiSource::Scheduled, control, &parameters)
                .unwrap();
        }
        let scheduled = MidiSource::Scheduled.index();
        for (source, channel) in [(MidiSource::Immediate.index(), 1), (scheduled, 1)] {
            assert_eq!(prepared.expression[source][channel], 1.0);
            assert_eq!(prepared.pressure[source][channel], 1.0);
            assert_eq!(prepared.pitch_bend[source][channel], 0);
            assert!(!prepared.sustain[source][channel]);
        }
        assert!(prepared.expression[scheduled][15] < 1.0);
        assert!(prepared.pressure[scheduled][15] < 1.0);
        assert!(prepared.pitch_bend[scheduled][15] > 0);
        assert!(prepared.voices[2].key_pressure < 1.0);
        for index in [0, 1, 3] {
            assert_eq!(prepared.voices[index].key_pressure, 1.0);
        }
        for channel in [1, 15] {
            prepared
                .dispatch_from(
                    MidiSource::Scheduled,
                    channel_message(channel, MidiMessageKind::NoteOff, 60, 0),
                    &parameters,
                )
                .unwrap();
        }
        assert!(prepared.voices[0].held);
        assert!(prepared.voices[1].envelope.is_releasing());
        assert!(!prepared.voices[2].held);
        assert!(!prepared.voices[2].envelope.is_releasing());
        prepared
            .dispatch_from(
                MidiSource::Scheduled,
                channel_message(1, MidiMessageKind::ControlChange, 64, 0),
                &parameters,
            )
            .unwrap();
        assert!(!prepared.voices[2].envelope.is_releasing());
        prepared
            .dispatch_from(
                MidiSource::Scheduled,
                channel_message(15, MidiMessageKind::ControlChange, 123, 0),
                &parameters,
            )
            .unwrap();
        assert_eq!(prepared.active_voice_count(), 2);
        assert!(prepared.voices[0].held);
        assert!(prepared.voices[1].envelope.is_releasing());

        for channel in [1, 15] {
            prepared
                .dispatch_from(
                    MidiSource::Scheduled,
                    channel_message(channel, MidiMessageKind::NoteOn, 64, 127),
                    &parameters,
                )
                .unwrap();
        }
        prepared.all_notes_off_from(MidiSource::Scheduled).unwrap();
        assert_eq!(prepared.active_voice_count(), 1);
        assert!(prepared.voices[0].held);
        assert!(prepared.sustain[scheduled].iter().all(|sustain| !sustain));
        let mut output = [0.0; 128];
        prepared.render(&mut output, 64, &parameters).unwrap();
        assert!(output.iter().any(|sample| sample.abs() > 0.000_001));
    }

    #[test]
    fn native_channel_expression_matches_independent_immediate_braids() {
        let patch = patch(1);
        let parameters = parameters(
            patch.id(),
            VoiceEnvelope::new(0.0, 0.0, 1.0, 10.0).unwrap(),
            [3.0, 0.5, 0.5],
        );
        let preparer = BraidsPreparer::new().unwrap();
        let prepare = || {
            preparer
                .prepare(&patch, BRAIDS_HOST_SAMPLE_RATE, 64)
                .unwrap()
        };
        let mut mixed = prepare();
        let mut first = prepare();
        let mut second = prepare();
        for event in [
            channel_message(1, MidiMessageKind::PitchBend, 127, 127),
            channel_message(1, MidiMessageKind::ControlChange, 11, 64),
            channel_message(1, MidiMessageKind::ChannelPressure, 80, 0),
        ] {
            mixed
                .dispatch_from(MidiSource::Scheduled, event, &parameters)
                .unwrap();
            first.dispatch(event, &parameters).unwrap();
        }
        for (channel, reference) in [(1, &mut first), (15, &mut second)] {
            let event = channel_message(channel, MidiMessageKind::NoteOn, 60, 100);
            mixed
                .dispatch_from(MidiSource::Scheduled, event, &parameters)
                .unwrap();
            reference.dispatch(event, &parameters).unwrap();
        }
        for event in [
            channel_message(1, MidiMessageKind::PolyPressure, 60, 32),
            channel_message(1, MidiMessageKind::ControlChange, 7, 100),
            channel_message(1, MidiMessageKind::PitchBend, 0, 0),
            channel_message(1, MidiMessageKind::AllNotesOff, 0, 0),
        ] {
            mixed
                .dispatch_from(MidiSource::Scheduled, event, &parameters)
                .unwrap();
            first.dispatch(event, &parameters).unwrap();
            let mut first_output = [0.0; 128];
            let mut second_output = [0.0; 128];
            let mut actual = [0.0; 128];
            first.render(&mut first_output, 64, &parameters).unwrap();
            second.render(&mut second_output, 64, &parameters).unwrap();
            mixed.render(&mut actual, 64, &parameters).unwrap();
            assert!(second_output.iter().any(|sample| sample.abs() > 0.000_001));
            assert!(actual
                .iter()
                .zip(first_output)
                .zip(second_output)
                .all(|((actual, first), second)| (*actual - first - second).abs() < 0.000_001));
        }
    }

    #[test]
    fn identified_same_pitch_audio_matches_independent_voices_through_release_and_cancel() {
        let patch = patch(1);
        let parameters = parameters(
            patch.id(),
            VoiceEnvelope::new(0.0, 0.0, 1.0, 10.0).unwrap(),
            [3.0, 0.5, 0.5],
        );
        let preparer = BraidsPreparer::new().unwrap();
        let prepare = || {
            preparer
                .prepare_patch(&patch, BRAIDS_HOST_SAMPLE_RATE, 64)
                .unwrap()
        };
        let mut mixed = prepare();
        let mut references = [prepare(), prepare(), prepare()];
        let first = MidiVoiceId::new(1).unwrap();
        let second = MidiVoiceId::new(2).unwrap();
        for ((source, id), reference) in [
            (MidiSource::Scheduled, first),
            (MidiSource::Scheduled, second),
            (MidiSource::Monitor, first),
        ]
        .into_iter()
        .zip(references.iter_mut())
        {
            let event = message(MidiMessageKind::NoteOn, 60, 100);
            mixed
                .dispatch_voice_from(source, Some(id), event, &parameters)
                .unwrap();
            reference.dispatch(event, &parameters).unwrap();
        }
        assert_eq!(mixed.active_note_count(), Some(3));
        let assert_audio =
            |mixed: &mut PreparedBraidsInstrument,
             references: &mut [PreparedBraidsInstrument; 3]| {
                let mut expected = [0.0_f32; 128];
                for reference in references {
                    let mut contribution = [0.0; 128];
                    reference
                        .render(&mut contribution, 64, &parameters)
                        .unwrap();
                    for (sum, sample) in expected.iter_mut().zip(contribution) {
                        *sum += sample;
                    }
                }
                let mut actual = [0.0; 128];
                mixed.render(&mut actual, 64, &parameters).unwrap();
                assert!(actual
                    .iter()
                    .zip(expected)
                    .all(|(actual, expected)| (*actual - expected).abs() < 1e-6));
            };
        for event in [
            message(MidiMessageKind::PitchBend, 127, 127),
            message(MidiMessageKind::ChannelPressure, 64, 0),
            message(MidiMessageKind::PolyPressure, 60, 32),
            message(MidiMessageKind::ControlChange, 11, 80),
            message(MidiMessageKind::NoteOff, 60, 0),
        ] {
            mixed
                .dispatch_voice_from(MidiSource::Scheduled, Some(first), event, &parameters)
                .unwrap();
            references[0].dispatch(event, &parameters).unwrap();
            assert_audio(&mut mixed, &mut references);
        }
        assert_eq!(mixed.active_note_count(), Some(2));
        assert!(mixed.voices[1].held && mixed.voices[2].held);
        assert_eq!(mixed.pressure[MidiSource::Scheduled.index()][1], 1.0);
        assert_eq!(mixed.expression[MidiSource::Scheduled.index()][1], 1.0);
        assert_eq!(mixed.pitch_bend[MidiSource::Scheduled.index()][1], 0);
        for _ in 0..8 {
            assert_audio(&mut mixed, &mut references);
        }
        assert_eq!(mixed.active_voice_count(), 2);
        for controller in [98, 99, 123] {
            assert_eq!(
                mixed.dispatch_voice_from(
                    MidiSource::Scheduled,
                    Some(second),
                    message(MidiMessageKind::ControlChange, controller, 0),
                    &parameters
                ),
                Err(PreparedInstrumentError::DispatchRejected)
            );
        }
        mixed.all_notes_off_from(MidiSource::Monitor).unwrap();
        references[2].all_notes_off();
        assert_audio(&mut mixed, &mut references);
        assert_eq!(mixed.active_note_count(), Some(1));
        assert!(mixed.voices[1].held);
        let event = message(MidiMessageKind::ChannelPressure, 90, 0);
        mixed
            .dispatch_from(MidiSource::Scheduled, event, &parameters)
            .unwrap();
        references[1].dispatch(event, &parameters).unwrap();
        assert_audio(&mut mixed, &mut references);
        assert_eq!(
            mixed.pressure[MidiSource::Scheduled.index()][1],
            90.0 / 127.0
        );
        mixed.all_notes_off_from(MidiSource::Scheduled).unwrap();
        let mut output = [1.0; 128];
        mixed.render(&mut output, 64, &parameters).unwrap();
        assert!(output.iter().all(|sample| *sample == 0.0));
    }

    #[test]
    fn identified_sustain_and_count_exclude_release_tails_and_keep_other_overlap() {
        let patch = patch(1);
        let parameters = parameters(
            patch.id(),
            VoiceEnvelope::new(0.0, 0.0, 1.0, 40.0).unwrap(),
            [3.0, 0.5, 0.5],
        );
        let mut prepared = BraidsPreparer::new()
            .unwrap()
            .prepare_patch(&patch, BRAIDS_HOST_SAMPLE_RATE, 64)
            .unwrap();
        let first = MidiVoiceId::new(1).unwrap();
        let second = MidiVoiceId::new(2).unwrap();
        for id in [first, second] {
            prepared
                .dispatch_voice_from(
                    MidiSource::Scheduled,
                    Some(id),
                    message(MidiMessageKind::NoteOn, 60, 100),
                    &parameters,
                )
                .unwrap();
        }
        let off = message(MidiMessageKind::NoteOff, 60, 0);
        prepared
            .dispatch_voice_from(MidiSource::Scheduled, Some(first), off, &parameters)
            .unwrap();
        assert_eq!(prepared.active_note_count(), Some(1));
        prepared
            .dispatch_from(
                MidiSource::Scheduled,
                message(MidiMessageKind::ControlChange, 64, 127),
                &parameters,
            )
            .unwrap();
        // Pedal down cannot re-admit the first note's release tail.
        assert_eq!(prepared.active_note_count(), Some(1));
        prepared
            .dispatch_voice_from(MidiSource::Scheduled, Some(second), off, &parameters)
            .unwrap();
        assert_eq!(prepared.active_note_count(), Some(1));
        assert!(!prepared.voices[1].held && !prepared.voices[1].envelope.is_releasing());
        prepared
            .dispatch_from(
                MidiSource::Scheduled,
                message(MidiMessageKind::ControlChange, 64, 0),
                &parameters,
            )
            .unwrap();
        assert_eq!(prepared.active_note_count(), Some(0));
    }

    #[test]
    fn model_timbre_and_color_each_change_the_native_render() {
        fn render(scalars: [f32; 3]) -> [f32; 128] {
            let patch = patch(1);
            let parameters = parameters(patch.id(), VoiceEnvelope::DEFAULT, scalars);
            let mut prepared = BraidsPreparer::new()
                .unwrap()
                .prepare_patch(&patch, BRAIDS_HOST_SAMPLE_RATE, 64)
                .unwrap();
            prepared
                .dispatch(message(MidiMessageKind::NoteOn, 60, 127), &parameters)
                .unwrap();
            let mut output = [0.0; 128];
            prepared.render(&mut output, 64, &parameters).unwrap();
            output
        }

        let baseline = render([0.0, 0.5, 0.5]);
        assert_ne!(baseline, render([1.0, 0.5, 0.5]));
        assert_ne!(baseline, render([0.0, 0.75, 0.5]));
        assert_ne!(baseline, render([0.0, 0.5, 0.75]));
    }
}
