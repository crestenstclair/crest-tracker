use core::fmt;

/// The reason a SoundFont instrument selector could not be constructed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SoundFontInstrumentError {
    /// The supplied MIDI program is outside 0..=127.
    ProgramOutOfRange(u8),
}

impl SoundFontInstrumentError {
    /// Returns the rejected MIDI program number.
    pub const fn program(self) -> u8 {
        match self {
            Self::ProgramOutOfRange(program) => program,
        }
    }
}

impl fmt::Display for SoundFontInstrumentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "SoundFont program {} is out of range; expected 0..=127",
            self.program()
        )
    }
}

impl std::error::Error for SoundFontInstrumentError {}

/// The preset selector derived from one MIDI instrument part.
///
/// Percussion is part of the value's identity, so a percussion instrument and
/// melodic instrument with the same bank and program remain distinct.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SoundFontInstrument {
    bank: u16,
    program: u8,
    percussion: bool,
}

impl SoundFontInstrument {
    /// The first valid MIDI program number.
    pub const MIN_PROGRAM: u8 = 0;

    /// The last valid MIDI program number.
    pub const MAX_PROGRAM: u8 = 127;

    /// Creates a validated SoundFont preset selector.
    pub const fn new(
        bank: u16,
        program: u8,
        percussion: bool,
    ) -> Result<Self, SoundFontInstrumentError> {
        if program <= Self::MAX_PROGRAM {
            Ok(Self {
                bank,
                program,
                percussion,
            })
        } else {
            Err(SoundFontInstrumentError::ProgramOutOfRange(program))
        }
    }

    /// Returns the complete SoundFont bank number.
    pub const fn bank(&self) -> u16 {
        self.bank
    }

    /// Returns the validated MIDI program number.
    pub const fn program(&self) -> u8 {
        self.program
    }

    /// Reports whether this selector identifies a percussion instrument.
    pub const fn percussion(&self) -> bool {
        self.percussion
    }

    /// Resolves the fixture's MIDI percussion convention to the canonical SF2
    /// numeric preset address before Patch configuration.
    pub const fn preset_id(&self) -> crate::synth::SoundFontPresetId {
        let bank = if self.percussion {
            self.bank | 128
        } else {
            self.bank & !128
        };
        // Construction already validates the program range.
        match crate::synth::SoundFontPresetId::new(bank, self.program) {
            Ok(id) => id,
            Err(_) => unreachable!(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{SoundFontInstrument, SoundFontInstrumentError};

    #[test]
    fn preserves_every_selector_field() {
        let instrument = SoundFontInstrument::new(128, 42, true).unwrap();

        assert_eq!(instrument.bank(), 128);
        assert_eq!(instrument.program(), 42);
        assert!(instrument.percussion());
    }

    #[test]
    fn accepts_both_program_boundaries_and_every_bank_value() {
        let minimum = SoundFontInstrument::new(0, SoundFontInstrument::MIN_PROGRAM, false).unwrap();
        let maximum =
            SoundFontInstrument::new(u16::MAX, SoundFontInstrument::MAX_PROGRAM, false).unwrap();

        assert_eq!(minimum.program(), 0);
        assert_eq!(maximum.bank(), u16::MAX);
        assert_eq!(maximum.program(), 127);
    }

    #[test]
    fn rejects_programs_outside_the_midi_range() {
        let error = SoundFontInstrument::new(0, 128, false).unwrap_err();

        assert_eq!(error, SoundFontInstrumentError::ProgramOutOfRange(128));
        assert_eq!(error.program(), 128);
        assert_eq!(
            error.to_string(),
            "SoundFont program 128 is out of range; expected 0..=127"
        );
        assert!(SoundFontInstrument::new(0, u8::MAX, false).is_err());
    }

    #[test]
    fn percussion_is_part_of_instrument_identity() {
        let melodic = SoundFontInstrument::new(0, 12, false).unwrap();
        let percussion = SoundFontInstrument::new(0, 12, true).unwrap();

        assert_ne!(melodic, percussion);
        assert!(melodic < percussion);
    }
}
