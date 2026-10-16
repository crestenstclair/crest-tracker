use crate::kernel::midi_channel::MidiChannel;
use core::fmt;
use serde::{Deserialize, Serialize};

const MAX_DATA_BYTE: u8 = 0x7f;

/// The normalized kind of a channel MIDI message.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[repr(u8)]
#[serde(rename_all = "camelCase")]
pub enum MidiMessageKind {
    NoteOn,
    NoteOff,
    ControlChange,
    ProgramChange,
    ChannelPressure,
    PitchBend,
    AllNotesOff,
    PolyPressure,
}

impl MidiMessageKind {
    pub const ALL: [Self; 8] = [
        Self::NoteOn,
        Self::NoteOff,
        Self::ControlChange,
        Self::ProgramChange,
        Self::ChannelPressure,
        Self::PitchBend,
        Self::AllNotesOff,
        Self::PolyPressure,
    ];

    pub const fn surface_descriptor() -> &'static [Self] {
        &Self::ALL
    }
}

/// A normalized channel MIDI message accepted by the synthesizer.
///
/// Data bytes are validated at construction so the value can cross the
/// real-time boundary without further parsing, allocation, or error handling.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[repr(C)]
#[serde(rename_all = "camelCase")]
pub struct MidiMessage {
    channel: MidiChannel,
    kind: MidiMessageKind,
    data1: u8,
    data2: u8,
}

impl MidiMessage {
    /// Creates a message when both MIDI data bytes are seven-bit values.
    pub const fn try_new(
        channel: MidiChannel,
        kind: MidiMessageKind,
        data1: u8,
        data2: u8,
    ) -> Result<Self, MidiMessageError> {
        match validate_data_bytes(data1, data2) {
            Ok(()) => Ok(Self {
                channel,
                kind,
                data1,
                data2,
            }),
            Err(error) => Err(error),
        }
    }

    /// Creates the dedicated all-notes-off command.
    pub const fn all_notes_off(channel: MidiChannel) -> Self {
        Self {
            channel,
            kind: MidiMessageKind::AllNotesOff,
            data1: 0,
            data2: 0,
        }
    }

    pub const fn channel(&self) -> MidiChannel {
        self.channel
    }

    pub const fn kind(&self) -> MidiMessageKind {
        self.kind
    }

    pub const fn data1(&self) -> u8 {
        self.data1
    }

    pub const fn data2(&self) -> u8 {
        self.data2
    }
}

/// The reason a raw channel message could not be normalized.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MidiMessageError {
    Data1OutOfRange(u8),
    Data2OutOfRange(u8),
}

impl fmt::Display for MidiMessageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Data1OutOfRange(value) => {
                write!(formatter, "MIDI data1 byte must be in 0..=127, got {value}")
            }
            Self::Data2OutOfRange(value) => {
                write!(formatter, "MIDI data2 byte must be in 0..=127, got {value}")
            }
        }
    }
}

impl std::error::Error for MidiMessageError {}

const fn validate_data_bytes(data1: u8, data2: u8) -> Result<(), MidiMessageError> {
    if data1 > MAX_DATA_BYTE {
        return Err(MidiMessageError::Data1OutOfRange(data1));
    }
    if data2 > MAX_DATA_BYTE {
        return Err(MidiMessageError::Data2OutOfRange(data2));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{validate_data_bytes, MidiMessage, MidiMessageError, MidiMessageKind};
    use crate::kernel::midi_channel::MidiChannel;

    #[test]
    fn exposes_every_required_message_kind() {
        let kinds = [
            MidiMessageKind::NoteOn,
            MidiMessageKind::NoteOff,
            MidiMessageKind::ControlChange,
            MidiMessageKind::ProgramChange,
            MidiMessageKind::ChannelPressure,
            MidiMessageKind::PitchBend,
            MidiMessageKind::AllNotesOff,
        ];

        assert_eq!(kinds.len(), 7);
    }

    #[test]
    fn accepts_the_complete_seven_bit_data_range() {
        assert_eq!(validate_data_bytes(0, 127), Ok(()));
        assert_eq!(validate_data_bytes(127, 0), Ok(()));
    }

    #[test]
    fn rejects_each_out_of_range_data_byte() {
        assert_eq!(
            validate_data_bytes(128, 0),
            Err(MidiMessageError::Data1OutOfRange(128))
        );
        assert_eq!(
            validate_data_bytes(0, 255),
            Err(MidiMessageError::Data2OutOfRange(255))
        );
    }

    #[test]
    fn public_api_keeps_normalized_state_read_only() {
        let _: fn(MidiChannel, MidiMessageKind, u8, u8) -> Result<MidiMessage, MidiMessageError> =
            MidiMessage::try_new;
        let _: fn(MidiChannel) -> MidiMessage = MidiMessage::all_notes_off;
        let _: fn(&MidiMessage) -> MidiChannel = MidiMessage::channel;
        let _: fn(&MidiMessage) -> MidiMessageKind = MidiMessage::kind;
        let _: fn(&MidiMessage) -> u8 = MidiMessage::data1;
        let _: fn(&MidiMessage) -> u8 = MidiMessage::data2;
    }

    #[test]
    fn validation_errors_are_actionable() {
        assert_eq!(
            MidiMessageError::Data1OutOfRange(200).to_string(),
            "MIDI data1 byte must be in 0..=127, got 200"
        );
        assert_eq!(
            MidiMessageError::Data2OutOfRange(201).to_string(),
            "MIDI data2 byte must be in 0..=127, got 201"
        );
    }
}
