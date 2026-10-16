use core::fmt;
use serde::Serialize;

/// A validated zero-based MIDI channel number.
///
/// MIDI channel numbers are represented internally and at adapter boundaries in
/// the canonical `0..=15` range.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[repr(transparent)]
#[serde(transparent)]
pub struct MidiChannel(u8);

impl MidiChannel {
    /// The first valid zero-based MIDI channel.
    pub const MIN: u8 = 0;

    /// The last valid zero-based MIDI channel.
    pub const MAX: u8 = 15;

    /// Creates a MIDI channel when `value` is in the MIDI channel range.
    pub const fn new(value: u8) -> Result<Self, MidiChannelError> {
        if value <= Self::MAX {
            Ok(Self(value))
        } else {
            Err(MidiChannelError::OutOfRange(value))
        }
    }

    /// Returns the zero-based MIDI channel number.
    pub const fn value(self) -> u8 {
        self.0
    }
}

impl TryFrom<u8> for MidiChannel {
    type Error = MidiChannelError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<MidiChannel> for u8 {
    fn from(channel: MidiChannel) -> Self {
        channel.value()
    }
}

impl fmt::Display for MidiChannel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// The validation error returned for a MIDI channel outside `0..=15`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MidiChannelError {
    /// The supplied zero-based channel number is outside the MIDI range.
    OutOfRange(u8),
}

impl MidiChannelError {
    /// Returns the invalid channel number.
    pub const fn value(self) -> u8 {
        match self {
            Self::OutOfRange(value) => value,
        }
    }
}

impl fmt::Display for MidiChannelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "MIDI channel {} is out of range; expected 0..=15",
            self.value()
        )
    }
}

impl std::error::Error for MidiChannelError {}

#[cfg(test)]
mod tests {
    use super::{MidiChannel, MidiChannelError};

    #[test]
    fn accepts_both_ends_of_the_midi_channel_range() {
        assert_eq!(MidiChannel::new(MidiChannel::MIN).unwrap().value(), 0);
        assert_eq!(MidiChannel::new(MidiChannel::MAX).unwrap().value(), 15);
    }

    #[test]
    fn rejects_values_above_the_midi_channel_range() {
        let error = MidiChannel::new(16).unwrap_err();

        assert_eq!(error, MidiChannelError::OutOfRange(16));
        assert_eq!(error.value(), 16);
        assert_eq!(
            error.to_string(),
            "MIDI channel 16 is out of range; expected 0..=15"
        );
        assert!(MidiChannel::new(u8::MAX).is_err());
    }

    #[test]
    fn primitive_conversions_preserve_the_validated_value() {
        let channel = MidiChannel::try_from(9).unwrap();

        assert_eq!(u8::from(channel), 9);
        assert_eq!(channel.to_string(), "9");
    }
}
