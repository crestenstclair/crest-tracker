//! Admission and atomic initialization at the Tracker/Synth boundary.
//! Musical curves remain in Tracker; native capability and pitch rules stay in Synth.
use super::*;
use crest_synth::synth::{OwnedExpressionState, OwnedVoiceInitialization};
use std::collections::BTreeSet;

#[derive(Clone, Debug)]
pub(super) struct Dimension {
    bend: u16,
    semitones: u8,
    cents: u8,
    pressure: Option<u8>,
    timbre: Option<u8>,
    pedal: u8,
    rpn: (u8, u8),
}
impl Default for Dimension {
    fn default() -> Self {
        Self {
            bend: 8192,
            semitones: 2,
            cents: 0,
            pressure: None,
            timbre: None,
            pedal: 0,
            rpn: (127, 127),
        }
    }
}
impl Dimension {
    fn apply(&mut self, control: Control, master: bool, pitch: u8) -> Result<(), String> {
        match control {
            Control::Bend { value } => self.bend = value,
            Control::Pressure { value } => self.pressure = Some(value),
            Control::PolyPressure { pitch: key, value } if !master && key == pitch => {
                self.pressure = Some(value)
            }
            Control::Cc { number: 74, value } if !master => self.timbre = Some(value),
            Control::Cc { number: 64, value } => self.pedal = value,
            Control::Cc { number: 101, value } if matches!(value, 0 | 127) => self.rpn.0 = value,
            Control::Cc { number: 100, value } if matches!(value, 0 | 127) => self.rpn.1 = value,
            Control::Cc { number: 6, value } if self.rpn == (0, 0) => self.semitones = value,
            Control::Cc { number: 38, value } if self.rpn == (0, 0) && value <= 99 => {
                self.cents = value
            }
            _ => {
                return Err(format!(
                    "Unsupported {} owned expression: {control:?}",
                    if master { "master" } else { "member" }
                ))
            }
        }
        // Constructor validates raw MIDI ranges independently of native pitch.
        self.native()?;
        Ok(())
    }
    fn native(&self) -> Result<OwnedExpressionState, String> {
        OwnedExpressionState::new(
            self.bend,
            self.semitones,
            self.cents,
            self.pressure,
            self.timbre,
            self.pedal,
        )
        .map_err(|e| e.to_string())
    }
    fn pitch(&self) -> (u16, u8, u8) {
        (self.bend, self.semitones, self.cents)
    }
}
#[derive(Clone, Debug)]
pub(super) struct VoiceState {
    pub pitch: u8,
    pub channel: Channel,
    member: Dimension,
    master: Dimension,
}
impl VoiceState {
    pub fn new(pitch: u8, channel: Channel) -> Self {
        Self {
            pitch,
            channel,
            member: Dimension::default(),
            master: Dimension::default(),
        }
    }
    pub fn apply(&mut self, control: Control, master: bool) -> Result<(), String> {
        if master {
            self.master.apply(control, true, self.pitch)
        } else {
            self.member.apply(control, false, self.pitch)
        }
    }
    pub fn validate(&self, sound: &Sound<'_>, patch: PatchId) -> Result<(), String> {
        sound
            .owned_expression_profile(patch)
            .map_err(|e| e.to_string())?
            .validate_combined_pitch(self.pitch, self.member.pitch(), self.master.pitch())
            .map_err(|e| e.to_string())
    }
    pub fn initial(&self, velocity: u8) -> Result<OwnedVoiceInitialization, String> {
        if self.member.rpn != (127, 127) || self.master.rpn != (127, 127) {
            return Err(
                "Initial expression must close its RPN selection with CC101/100=127".into(),
            );
        }
        OwnedVoiceInitialization::new(
            midi(self.channel, MidiMessageKind::NoteOn, self.pitch, velocity)?,
            self.member.native()?,
            self.master.native()?,
        )
        .map_err(|e| e.to_string())
    }
}
pub(super) fn midi(
    channel: Channel,
    kind: MidiMessageKind,
    a: u8,
    b: u8,
) -> Result<MidiMessage, String> {
    MidiMessage::try_new(
        MidiChannel::new(channel.get() - 1).map_err(|e| e.to_string())?,
        kind,
        a,
        b,
    )
    .map_err(|e| e.to_string())
}
pub(super) fn control_midi(channel: Channel, control: Control) -> Result<MidiMessage, String> {
    let (kind, a, b) = match control {
        Control::Bend { value } => (
            MidiMessageKind::PitchBend,
            (value & 127) as u8,
            (value >> 7) as u8,
        ),
        Control::Pressure { value } => (MidiMessageKind::ChannelPressure, value, 0),
        Control::PolyPressure { pitch, value } => (MidiMessageKind::PolyPressure, pitch, value),
        Control::Cc { number, value } => (MidiMessageKind::ControlChange, number, value),
        Control::Program { value } => (MidiMessageKind::ProgramChange, value, 0),
    };
    midi(channel, kind, a, b)
}

#[derive(Default, Clone)]
pub(super) struct Ledger {
    active: BTreeMap<Owner, VoiceState>,
}
pub(super) struct Window {
    pub initial: BTreeMap<usize, OwnedVoiceInitialization>,
    pub consumed: BTreeSet<usize>,
    pub external: BTreeMap<usize, (Vec<Control>, Vec<Control>)>,
}
impl Ledger {
    pub fn clear(&mut self) {
        self.active.clear();
    }
    /// Stages all admission before the caller queues any event from this window.
    /// Zero-offset snapshots join the attack as one bounded native command.
    pub fn prepare(
        &mut self,
        sound: &Sound<'_>,
        app: &App,
        events: &[TimedEvent],
        profiles: &[crate::midi_output::MpeOutputProfile],
    ) -> Result<Window, String> {
        let mut staged = self.clone();
        let mut window = Window {
            initial: BTreeMap::new(),
            consumed: BTreeSet::new(),
            external: BTreeMap::new(),
        };
        let lifetimes: BTreeSet<_> = events
            .iter()
            .filter(|e| matches!(e.message, Message::ExpressionLifetime { .. }))
            .map(|e| (e.owner.clone(), e.at))
            .collect();
        let mut initial_controls: BTreeMap<_, Vec<_>> = BTreeMap::new();
        for (index, event) in events.iter().enumerate() {
            if lifetimes.contains(&(event.owner.clone(), event.at)) {
                match event.message {
                    Message::Expression { control } => initial_controls
                        .entry((event.owner.clone(), event.at))
                        .or_default()
                        .push((index, control, false)),
                    Message::MasterExpression { control } => initial_controls
                        .entry((event.owner.clone(), event.at))
                        .or_default()
                        .push((index, control, true)),
                    _ => {}
                }
            }
        }
        for (index, event) in events.iter().enumerate() {
            if window.consumed.contains(&index) {
                continue;
            }
            match event.message {
                Message::NoteOn { pitch, velocity }
                    if lifetimes.contains(&(event.owner.clone(), event.at)) =>
                {
                    let mut state = VoiceState::new(pitch, event.target.channel());
                    let mut member_controls = Vec::new();
                    let mut master_controls = Vec::new();
                    if let Some(controls) =
                        initial_controls.remove(&(event.owner.clone(), event.at))
                    {
                        for (index, control, master) in controls {
                            state.apply(control, master)?;
                            if master {
                                master_controls.push(control);
                            } else {
                                member_controls.push(control);
                            }
                            window.consumed.insert(index);
                        }
                    }
                    validate_target(sound, app, &event.target, &state, profiles)?;
                    if matches!(
                        event.target,
                        Target::Channel {
                            endpoint: Some(_),
                            ..
                        }
                    ) {
                        window
                            .external
                            .insert(index, (member_controls, master_controls));
                    } else {
                        window.initial.insert(index, state.initial(velocity)?);
                    }
                    staged.active.insert(event.owner.clone(), state);
                }
                Message::Expression { control } | Message::MasterExpression { control } => {
                    if let Some(state) = staged.active.get_mut(&event.owner) {
                        state.apply(
                            control,
                            matches!(event.message, Message::MasterExpression { .. }),
                        )?;
                        validate_target(sound, app, &event.target, state, profiles)?;
                    } else if matches!(event.message, Message::MasterExpression { .. }) {
                        return Err("Master expression has no initialized attack".into());
                    }
                }
                Message::EndExpression => {
                    staged.active.remove(&event.owner);
                }
                _ => {}
            }
        }
        *self = staged;
        Ok(window)
    }
}
fn validate_target(
    sound: &Sound<'_>,
    app: &App,
    target: &Target,
    state: &VoiceState,
    profiles: &[crate::midi_output::MpeOutputProfile],
) -> Result<(), String> {
    if let Target::Channel {
        endpoint: Some(endpoint),
        ..
    } = target
    {
        return profiles
            .iter()
            .find(|profile| &profile.endpoint == endpoint)
            .ok_or("Configure an MPE output profile for this exact endpoint")?
            .validate();
    }
    let patches = internal_targets(sound, app, target)?;
    if patches.is_empty() {
        return Err("MPE destination has no installed instrument".into());
    }
    for patch in patches {
        state.validate(sound, patch)?;
    }
    Ok(())
}
pub(super) fn validate_song(
    sound: &Sound<'_>,
    app: &App,
    profiles: &[crate::midi_output::MpeOutputProfile],
) -> Result<(), String> {
    for phrase in app.editor.song().phrases.values() {
        for event in &phrase.events {
            if event.note.as_ref().is_some_and(|note| {
                note.mpe.is_some()
                    || note.expression_duration.is_some()
                    || !note.master_expression.is_empty()
            }) {
                let state = VoiceState::new(
                    event.note.as_ref().expect("note checked").pitch,
                    event.target.channel(),
                );
                validate_target(sound, app, &event.target, &state, profiles)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn initialization_retains_raw_opposing_bends_without_intermediate_pitch_admission() {
        let mut state = VoiceState::new(0, Channel::new(2).unwrap());
        state.apply(Control::Bend { value: 0 }, false).unwrap();
        state.apply(Control::Bend { value: 16383 }, true).unwrap();
        let initial = state.initial(100).unwrap();
        assert_eq!(initial.member().raw14(), 0);
        assert_eq!(initial.master().raw14(), 16383);
        assert_eq!(initial.member().pressure(), None);
    }
    #[test]
    fn master_timbre_and_unknown_rpn_data_are_explicit_errors() {
        let mut state = VoiceState::new(60, Channel::new(2).unwrap());
        assert!(state
            .apply(
                Control::Cc {
                    number: 74,
                    value: 1
                },
                true
            )
            .is_err());
        assert!(state
            .apply(
                Control::Cc {
                    number: 6,
                    value: 48
                },
                false
            )
            .is_err());
        for (number, value) in [
            (101, 0),
            (100, 0),
            (6, 48),
            (38, 12),
            (101, 127),
            (100, 127),
        ] {
            state.apply(Control::Cc { number, value }, false).unwrap();
        }
        let initial = state.initial(100).unwrap();
        assert_eq!(
            (initial.member().semitones(), initial.member().cents()),
            (48, 12)
        );
        assert!(state
            .apply(
                Control::Cc {
                    number: 38,
                    value: 0
                },
                false
            )
            .is_err());
    }
}
