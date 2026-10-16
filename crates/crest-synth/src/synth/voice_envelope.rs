use core::fmt;
use core::ops::RangeInclusive;
use serde::{Deserialize, Serialize};

/// Identifies one canonical, Patch-owned ADSR value.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VoiceEnvelopeParameter {
    AttackMilliseconds,
    DecayMilliseconds,
    Sustain,
    ReleaseMilliseconds,
}

impl VoiceEnvelopeParameter {
    /// Returns the stable serialized and projected field name.
    pub const fn name(self) -> &'static str {
        match self {
            Self::AttackMilliseconds => "attackMilliseconds",
            Self::DecayMilliseconds => "decayMilliseconds",
            Self::Sustain => "sustain",
            Self::ReleaseMilliseconds => "releaseMilliseconds",
        }
    }

    /// Returns this field's production-owned bounds and edit steps.
    pub const fn descriptor(self) -> &'static VoiceEnvelopeParameterDescriptor {
        match self {
            Self::AttackMilliseconds => &VOICE_ENVELOPE_SURFACE_DESCRIPTOR[0],
            Self::DecayMilliseconds => &VOICE_ENVELOPE_SURFACE_DESCRIPTOR[1],
            Self::Sustain => &VOICE_ENVELOPE_SURFACE_DESCRIPTOR[2],
            Self::ReleaseMilliseconds => &VOICE_ENVELOPE_SURFACE_DESCRIPTOR[3],
        }
    }

    fn bounds(self) -> RangeInclusive<f32> {
        let descriptor = self.descriptor();
        descriptor.minimum()..=descriptor.maximum()
    }
}

impl fmt::Display for VoiceEnvelopeParameter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// The independent pre-dispatch oracle for one ADSR parameter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoiceEnvelopeParameterDescriptor {
    parameter: VoiceEnvelopeParameter,
    label: &'static str,
    unit: Option<&'static str>,
    minimum: f32,
    maximum: f32,
    fine_step: f32,
    coarse_step: f32,
}

impl VoiceEnvelopeParameterDescriptor {
    const fn new(
        parameter: VoiceEnvelopeParameter,
        label: &'static str,
        unit: Option<&'static str>,
        minimum: f32,
        maximum: f32,
        fine_step: f32,
        coarse_step: f32,
    ) -> Self {
        Self {
            parameter,
            label,
            unit,
            minimum,
            maximum,
            fine_step,
            coarse_step,
        }
    }

    pub const fn parameter(&self) -> VoiceEnvelopeParameter {
        self.parameter
    }

    pub const fn name(&self) -> &'static str {
        self.parameter.name()
    }

    pub const fn label(&self) -> &'static str {
        self.label
    }

    pub const fn unit(&self) -> Option<&'static str> {
        self.unit
    }

    pub const fn minimum(&self) -> f32 {
        self.minimum
    }

    pub const fn maximum(&self) -> f32 {
        self.maximum
    }

    pub const fn fine_step(&self) -> f32 {
        self.fine_step
    }

    pub const fn coarse_step(&self) -> f32 {
        self.coarse_step
    }

    pub fn contains(&self, value: f32) -> bool {
        value.is_finite() && (self.minimum..=self.maximum).contains(&value)
    }
}

const VOICE_ENVELOPE_SURFACE_DESCRIPTOR: [VoiceEnvelopeParameterDescriptor; 4] = [
    VoiceEnvelopeParameterDescriptor::new(
        VoiceEnvelopeParameter::AttackMilliseconds,
        "Attack",
        Some("ms"),
        0.0,
        10_000.0,
        1.0,
        100.0,
    ),
    VoiceEnvelopeParameterDescriptor::new(
        VoiceEnvelopeParameter::DecayMilliseconds,
        "Decay",
        Some("ms"),
        0.0,
        10_000.0,
        1.0,
        100.0,
    ),
    VoiceEnvelopeParameterDescriptor::new(
        VoiceEnvelopeParameter::Sustain,
        "Sustain",
        None,
        0.0,
        1.0,
        0.01,
        0.1,
    ),
    VoiceEnvelopeParameterDescriptor::new(
        VoiceEnvelopeParameter::ReleaseMilliseconds,
        "Release",
        Some("ms"),
        0.0,
        10_000.0,
        1.0,
        100.0,
    ),
];

/// The reason a canonical envelope could not be constructed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VoiceEnvelopeError {
    NonFinite {
        parameter: VoiceEnvelopeParameter,
    },
    OutOfRange {
        parameter: VoiceEnvelopeParameter,
        value: f32,
    },
}

impl fmt::Display for VoiceEnvelopeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::NonFinite { parameter } => write!(formatter, "{parameter} must be finite"),
            Self::OutOfRange { parameter, value } => {
                let bounds = parameter.bounds();
                write!(
                    formatter,
                    "{parameter} must be in {}..={}, got {value}",
                    bounds.start(),
                    bounds.end()
                )
            }
        }
    }
}

impl std::error::Error for VoiceEnvelopeError {}

/// The canonical Patch ADSR copied to every admitted prepared engine.
///
/// The neutral default preserves immediate note-on/note-off behavior until a
/// player edits the envelope: zero-time Attack/Decay/Release and full Sustain.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceEnvelope {
    attack_milliseconds: f32,
    decay_milliseconds: f32,
    sustain: f32,
    release_milliseconds: f32,
}

impl VoiceEnvelope {
    pub const DEFAULT: Self = Self {
        attack_milliseconds: 0.0,
        decay_milliseconds: 0.0,
        sustain: 1.0,
        release_milliseconds: 0.0,
    };
    pub const MIN_TIME_MILLISECONDS: f32 = 0.0;
    pub const MAX_TIME_MILLISECONDS: f32 = 10_000.0;
    pub const MIN_SUSTAIN: f32 = 0.0;
    pub const MAX_SUSTAIN: f32 = 1.0;

    /// Returns each ADSR field exactly once in canonical projection order.
    pub const fn surface_descriptor() -> &'static [VoiceEnvelopeParameterDescriptor] {
        &VOICE_ENVELOPE_SURFACE_DESCRIPTOR
    }

    pub fn new(
        attack_milliseconds: f32,
        decay_milliseconds: f32,
        sustain: f32,
        release_milliseconds: f32,
    ) -> Result<Self, VoiceEnvelopeError> {
        validate(
            VoiceEnvelopeParameter::AttackMilliseconds,
            attack_milliseconds,
        )?;
        validate(
            VoiceEnvelopeParameter::DecayMilliseconds,
            decay_milliseconds,
        )?;
        validate(VoiceEnvelopeParameter::Sustain, sustain)?;
        validate(
            VoiceEnvelopeParameter::ReleaseMilliseconds,
            release_milliseconds,
        )?;
        Ok(Self {
            attack_milliseconds,
            decay_milliseconds,
            sustain,
            release_milliseconds,
        })
    }

    pub const fn attack_milliseconds(&self) -> f32 {
        self.attack_milliseconds
    }

    pub const fn decay_milliseconds(&self) -> f32 {
        self.decay_milliseconds
    }

    pub const fn sustain(&self) -> f32 {
        self.sustain
    }

    pub const fn release_milliseconds(&self) -> f32 {
        self.release_milliseconds
    }

    pub const fn value(&self, parameter: VoiceEnvelopeParameter) -> f32 {
        match parameter {
            VoiceEnvelopeParameter::AttackMilliseconds => self.attack_milliseconds,
            VoiceEnvelopeParameter::DecayMilliseconds => self.decay_milliseconds,
            VoiceEnvelopeParameter::Sustain => self.sustain,
            VoiceEnvelopeParameter::ReleaseMilliseconds => self.release_milliseconds,
        }
    }

    /// Replaces one field transactionally after shared descriptor validation.
    pub fn with_value(
        mut self,
        parameter: VoiceEnvelopeParameter,
        value: f32,
    ) -> Result<Self, VoiceEnvelopeError> {
        validate(parameter, value)?;
        match parameter {
            VoiceEnvelopeParameter::AttackMilliseconds => self.attack_milliseconds = value,
            VoiceEnvelopeParameter::DecayMilliseconds => self.decay_milliseconds = value,
            VoiceEnvelopeParameter::Sustain => self.sustain = value,
            VoiceEnvelopeParameter::ReleaseMilliseconds => self.release_milliseconds = value,
        }
        Ok(self)
    }
}

impl Default for VoiceEnvelope {
    fn default() -> Self {
        Self::DEFAULT
    }
}

fn validate(parameter: VoiceEnvelopeParameter, value: f32) -> Result<(), VoiceEnvelopeError> {
    if !value.is_finite() {
        return Err(VoiceEnvelopeError::NonFinite { parameter });
    }
    if !parameter.bounds().contains(&value) {
        return Err(VoiceEnvelopeError::OutOfRange { parameter, value });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_neutral_bounded_and_descriptor_complete() {
        let envelope = VoiceEnvelope::default();
        assert_eq!(envelope, VoiceEnvelope::new(0.0, 0.0, 1.0, 0.0).unwrap());
        assert_eq!(VoiceEnvelope::surface_descriptor().len(), 4);
        for descriptor in VoiceEnvelope::surface_descriptor() {
            assert!(descriptor.contains(envelope.value(descriptor.parameter())));
        }
    }

    #[test]
    fn validates_every_field_and_replaces_only_the_selected_value() {
        let envelope = VoiceEnvelope::new(5.0, 150.0, 0.75, 400.0).unwrap();
        let changed = envelope
            .with_value(VoiceEnvelopeParameter::ReleaseMilliseconds, 700.0)
            .unwrap();
        assert_eq!(changed.release_milliseconds(), 700.0);
        assert_eq!(
            changed.attack_milliseconds(),
            envelope.attack_milliseconds()
        );
        assert_eq!(changed.decay_milliseconds(), envelope.decay_milliseconds());
        assert_eq!(changed.sustain(), envelope.sustain());

        assert!(matches!(
            VoiceEnvelope::new(f32::NAN, 0.0, 1.0, 0.0),
            Err(VoiceEnvelopeError::NonFinite {
                parameter: VoiceEnvelopeParameter::AttackMilliseconds
            })
        ));
        assert!(matches!(
            VoiceEnvelope::new(0.0, 0.0, 1.1, 0.0),
            Err(VoiceEnvelopeError::OutOfRange {
                parameter: VoiceEnvelopeParameter::Sustain,
                ..
            })
        ));
    }
}
