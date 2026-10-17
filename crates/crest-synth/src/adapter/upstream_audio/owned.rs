//! Owned expression for the existing resident native voice bank.
use super::*;
use crate::adapter::owned_expression::OwnedExpression;

#[derive(Clone, Copy)]
pub(super) struct OwnedVoiceExpression {
    pub member: OwnedExpression,
    pub master: OwnedExpression,
    pub master_pressure: f32,
}

pub(super) fn pressure(state: OwnedExpressionState) -> f32 {
    state
        .pressure()
        .map_or(1.0, |value| f32::from(value) / 127.0)
}

impl OwnedVoiceExpression {
    pub fn for_attack<P: VoiceProcessor>(
        processor: &P,
        id: Option<MidiVoiceId>,
        controls: SourceMidiControls,
        initial: Option<OwnedVoiceInitialization>,
    ) -> Result<Option<Self>, PreparedInstrumentError> {
        if !processor.supports_owned_expression() {
            return if initial.is_some() {
                Err(PreparedInstrumentError::UnsupportedExpressionScope)
            } else {
                Ok(None)
            };
        }
        Ok(id.map(|_| match initial {
            Some(initial) => Self {
                member: OwnedExpression::from_initial(initial.member()),
                master: OwnedExpression::from_initial(initial.master()),
                master_pressure: pressure(initial.master()),
            },
            None => Self {
                member: OwnedExpression::new(
                    u16::from(controls.bend.0) | (u16::from(controls.bend.1) << 7),
                ),
                master: OwnedExpression::new(8192),
                master_pressure: 1.0,
            },
        }))
    }

    pub fn validate(self, note: u8) -> Result<(), PreparedInstrumentError> {
        self.member.bend_units(note, self.master).map(|_| ())
    }

    pub fn apply<P: VoiceProcessor>(
        self,
        processor: &mut P,
        note: u8,
    ) -> Result<(), PreparedInstrumentError> {
        let bend = self.member.bend_semitones(note, self.master)?;
        if processor.owned_expression(bend, self.member.timbre.unwrap_or(127)) {
            Ok(())
        } else {
            Err(PreparedInstrumentError::DispatchRejected)
        }
    }

    fn updated(
        self,
        note: u8,
        scope: MidiExpressionScope,
        message: MidiMessage,
    ) -> Result<Self, PreparedInstrumentError> {
        let mut next = self;
        let (current, other) = match scope {
            MidiExpressionScope::Member => (self.member, self.master),
            MidiExpressionScope::Master => (self.master, self.member),
        };
        let dimension = match message.kind() {
            MidiMessageKind::PitchBend => current.with_bend(
                note,
                u16::from(message.data1()) | (u16::from(message.data2()) << 7),
                other,
            )?,
            MidiMessageKind::ControlChange => {
                if scope == MidiExpressionScope::Master
                    && !matches!(message.data1(), 6 | 38 | 64 | 100 | 101)
                {
                    return Err(PreparedInstrumentError::UnsupportedExpressionScope);
                }
                current.with_controller(note, message.data1(), message.data2(), other)?
            }
            MidiMessageKind::ChannelPressure if scope == MidiExpressionScope::Master => {
                next.master_pressure = f32::from(message.data1()) / 127.0;
                return Ok(next);
            }
            _ => return Err(PreparedInstrumentError::UnsupportedExpressionScope),
        };
        match scope {
            MidiExpressionScope::Member => next.member = dimension,
            MidiExpressionScope::Master => next.master = dimension,
        }
        Ok(next)
    }
}

impl<P: VoiceProcessor> PreparedVoiceBank<P> {
    pub(super) fn dispatch_owned_expression(
        &mut self,
        source: MidiSource,
        id: MidiVoiceId,
        scope: MidiExpressionScope,
        message: MidiMessage,
        params: &RtPatchParameters,
    ) -> Result<(), PreparedInstrumentError> {
        let voice = self
            .voices
            .iter_mut()
            .find(|voice| {
                voice.belongs_to(source, Some(id), message.channel()) && !voice.envelope.is_idle()
            })
            .ok_or(PreparedInstrumentError::DispatchRejected)?;
        let owned = voice
            .owned
            .ok_or(PreparedInstrumentError::UnsupportedExpressionScope)?;
        let note = voice
            .note
            .ok_or(PreparedInstrumentError::DispatchRejected)?;
        if scope == MidiExpressionScope::Member
            && message.kind() == MidiMessageKind::ControlChange
            && matches!(message.data1(), 7 | 11)
        {
            voice.expression = f32::from(message.data2()) / 127.0;
            return Ok(());
        }
        let next = owned.updated(note, scope, message)?;
        next.apply(&mut voice.processor, note)?;
        voice.owned = Some(next);
        let channel_sustain =
            self.controls[source.index()][usize::from(message.channel().value())].sustain;
        if !voice.held && !voice.sustained(channel_sustain) && !voice.envelope.is_releasing() {
            voice
                .envelope
                .note_off(params.envelope().release_milliseconds(), self.rate);
            voice.processor.note(0x80, note, 0);
        }
        Ok(())
    }
}
