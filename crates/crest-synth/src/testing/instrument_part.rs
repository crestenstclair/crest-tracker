use crate::kernel::midi_channel::MidiChannel;
use crate::synth::sound_font_instrument::SoundFontInstrument;

const MIDI_CHANNEL_COUNT: usize = 16;

/// One stable MIDI instrument identity discovered while preparing the fixture.
///
/// The assigned channel is the part's stable zero-based index, so every
/// supported part owns a distinct MIDI render lane.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct InstrumentPart {
    index: usize,
    name: String,
    instrument: SoundFontInstrument,
    assigned_channel: MidiChannel,
}

impl InstrumentPart {
    /// Creates a part whose index and assigned channel are both in `0..16`.
    ///
    /// Panics when `index` would exhaust the sixteen available MIDI channels
    /// instead of reusing an existing part's render lane.
    pub fn new(index: usize, name: String, instrument: SoundFontInstrument) -> Self {
        assert!(
            index < MIDI_CHANNEL_COUNT,
            "InstrumentPart index {index} exhausts the 16 available MIDI channels"
        );
        let channel_number =
            u8::try_from(index).expect("a supported InstrumentPart index always fits in u8");
        let assigned_channel = MidiChannel::new(channel_number)
            .expect("a supported InstrumentPart index always produces a valid MIDI channel");

        Self {
            index,
            name,
            instrument,
            assigned_channel,
        }
    }

    /// Returns the part's stable zero-based discovery index.
    pub const fn index(&self) -> usize {
        self.index
    }

    /// Returns the fixture-provided instrument name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the exact SoundFont preset identity required by this part.
    pub const fn instrument(&self) -> SoundFontInstrument {
        self.instrument
    }

    /// Returns the MIDI channel equal to this part's stable index.
    pub const fn assigned_channel(&self) -> MidiChannel {
        self.assigned_channel
    }
}

#[cfg(test)]
mod tests {
    use super::InstrumentPart;
    use crate::synth::sound_font_instrument::SoundFontInstrument;

    fn instrument() -> SoundFontInstrument {
        SoundFontInstrument::new(128, 42, true).unwrap()
    }

    #[test]
    fn preserves_the_stable_part_identity() {
        let part = InstrumentPart::new(3, String::from("Percussion"), instrument());

        assert_eq!(part.index(), 3);
        assert_eq!(part.name(), "Percussion");
        assert_eq!(part.instrument(), instrument());
    }

    #[test]
    fn assigns_each_supported_index_to_the_same_unique_channel() {
        for index in [0, 1, 7, 14, 15] {
            let part = InstrumentPart::new(index, index.to_string(), instrument());

            assert_eq!(usize::from(part.assigned_channel().value()), index);
        }
    }

    #[test]
    #[should_panic(expected = "InstrumentPart index 16 exhausts the 16 available MIDI channels")]
    fn rejects_channel_exhaustion_instead_of_reusing_a_channel() {
        InstrumentPart::new(16, String::from("Seventeenth part"), instrument());
    }

    #[test]
    fn cloned_parts_retain_their_assignment_and_identity() {
        let original = InstrumentPart::new(2, String::from("Warm Pad"), instrument());
        let cloned = original.clone();

        assert_eq!(cloned, original);
        assert_eq!(cloned.assigned_channel().value(), 2);
    }
}
