//! Common owned MIDI semantics for Braids and resident Sample voices.
//! Integer admission precedes engine conversion. The supported pitch interval
//! covers the MIDI keyboard through 127 + 127/128, avoiding clipping in Braids
//! and unbounded sample interpolation ratios. This is a negotiated host contract,
//! not a claim that sfizz itself cannot transpose further.
use crate::synth::{OwnedExpressionState, PreparedInstrumentError};
const PITCH_UNITS_PER_SEMITONE: i32 = 128;
const PITCH_BEND_RANGE_SEMITONES: i32 = 2;
const MIDI_BEND_CENTER: i32 = 8_192;
const MAX_OWNED_PITCH: i32 = 128 * PITCH_UNITS_PER_SEMITONE - 1;

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

/// Scalar state belongs to one prepared attack, never a reused member channel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct OwnedExpression {
    pub(crate) bend_raw: u16,
    pub(crate) bend_semitones: u8,
    pub(crate) bend_cents: u8,
    pub(crate) rpn_msb: Option<u8>,
    pub(crate) rpn_lsb: Option<u8>,
    pub(crate) timbre: Option<u8>,
    pub(crate) sustain: Option<bool>,
}

impl OwnedExpression {
    pub(crate) const fn new(bend_raw: u16) -> Self {
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

    pub(crate) fn from_initial(state: OwnedExpressionState) -> Self {
        Self {
            bend_raw: state.raw14(),
            bend_semitones: state.semitones(),
            bend_cents: state.cents(),
            rpn_msb: None,
            rpn_lsb: None,
            timbre: state.timbre(),
            sustain: Some(state.pedal() >= 64),
        }
    }

    pub(crate) fn bend_units(self, note: u8, other: Self) -> Result<i16, PreparedInstrumentError> {
        owned_combined_pitch_bend_units(
            note,
            (self.bend_raw, self.bend_semitones, self.bend_cents),
            (other.bend_raw, other.bend_semitones, other.bend_cents),
        )
        .ok_or(PreparedInstrumentError::DispatchRejected)
    }

    /// Exact admitted bend before the engine's own numeric conversion.
    pub(crate) fn bend_semitones(
        self,
        note: u8,
        other: Self,
    ) -> Result<f32, PreparedInstrumentError> {
        self.bend_units(note, other)?;
        let member = (self.bend_raw, self.bend_semitones, self.bend_cents);
        let master = (other.bend_raw, other.bend_semitones, other.bend_cents);
        let numerator = owned_bend_numerator(member)
            .ok_or(PreparedInstrumentError::DispatchRejected)?
            + owned_bend_numerator(master).ok_or(PreparedInstrumentError::DispatchRejected)?;
        Ok(
            (numerator as f64 / f64::from(MIDI_BEND_CENTER * 100 * PITCH_UNITS_PER_SEMITONE))
                as f32,
        )
    }

    pub(crate) fn with_bend(
        mut self,
        note: u8,
        raw: u16,
        other: Self,
    ) -> Result<Self, PreparedInstrumentError> {
        self.bend_raw = raw;
        self.bend_units(note, other)?;
        Ok(self)
    }

    pub(crate) fn with_controller(
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
            74 => self.timbre = Some(value),
            64 => self.sustain = Some(value >= 64),
            _ => return Err(PreparedInstrumentError::DispatchRejected),
        }
        self.bend_units(note, other)?;
        Ok(self)
    }
}
