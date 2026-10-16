use super::*;
use crate::chord_bank::ChordBank;
use crate::harmony::{Chord, Pitch};

pub(super) struct StepOutput {
    pub events: Vec<TimedEvent>,
    pub jump: Option<u32>,
    pub cuts: Vec<(Beat, Target)>,
}
pub(super) fn validate(
    phrase: &Phrase,
    key: &Key,
    chords: &ChordBank,
    budget: PlaybackBudget,
) -> Result<(), PlaybackError> {
    let key = phrase.key.as_ref().unwrap_or(key);
    if let PhraseForm::Random { low, high, .. } = &phrase.form {
        if !(*low..=*high).any(|value| Pitch::new(value).is_ok_and(|pitch| key.contains(pitch))) {
            return Err(DocumentError::Invalid(
                "random phrase has no pitches in its scale and range",
            )
            .into());
        }
    }
    if phrase.events.len() > budget.events_per_window {
        return Err(PlaybackError::Budget("phrase events"));
    }
    for event in &phrase.events {
        validate_event(event, chords)?;
    }
    Ok(())
}
fn validate_event(event: &Event, chords: &ChordBank) -> Result<(), PlaybackError> {
    let Some(note) = &event.note else {
        return Ok(());
    };
    let pitches = resolve_pitches(note.pitch, &event.fx, chords)?;
    if (note.expression_duration.is_some()
        || !note.expression.is_empty()
        || !note.master_expression.is_empty())
        && (pitches.len() > 1
            || event
                .fx
                .iter()
                .flatten()
                .any(|v| matches!(v, Fx::Retrigger { .. })))
    {
        return Err(PlaybackError::Expression(
            "chord expansion and retrigger require separate expression members",
        ));
    }
    Ok(())
}
pub(super) fn step(
    phrase: &Phrase,
    key: &Key,
    chords: &ChordBank,
    row: u32,
    at: Beat,
    owner: Owner,
    budget: PlaybackBudget,
) -> Result<StepOutput, PlaybackError> {
    let grid = phrase.division.beat_length();
    let start = grid.checked_scale(u64::from(row), 1)?;
    let end = start.checked_add(grid)?;
    let mut output = StepOutput {
        events: Vec::new(),
        jump: None,
        cuts: Vec::new(),
    };
    for event in phrase
        .events
        .iter()
        .filter(|event| event.at >= start && event.at < end)
    {
        let mut event_owner = owner.clone();
        event_owner.event = event.id;
        let time = at.checked_add(event.at.checked_sub(start)?)?;
        if event.is_note_off() {
            output.cuts.push((time, event.target.clone()));
        }
        expand_event(
            event,
            time,
            event_owner,
            phrase.seed,
            &mut output,
            budget,
            chords,
        )?;
    }
    if let Some(event) = generated_event(phrase, key, row, &owner)? {
        expand_event(&event, at, owner, phrase.seed, &mut output, budget, chords)?;
    }
    Ok(output)
}
fn generated_event(
    phrase: &Phrase,
    key: &Key,
    row: u32,
    owner: &Owner,
) -> Result<Option<Event>, PlaybackError> {
    let id = EventId::new(u64::MAX - u64::from(row))?;
    let make =
        |pitch, velocity, gate: Beat, target: &Target| -> Result<Option<Event>, PlaybackError> {
            let duration = phrase
                .division
                .beat_length()
                .checked_scale(gate.numerator(), gate.denominator())?;
            Ok(Some(Event::note(
                id,
                Beat::ZERO,
                target.clone(),
                Note::new(pitch, velocity, duration)?,
            )))
        };
    match &phrase.form {
        PhraseForm::Euclidean {
            pitches,
            pulses,
            rotation,
            pitch,
            velocity,
            gate,
            target,
        } => {
            let shifted = (u64::from(row) + u64::from(phrase.steps) - u64::from(*rotation))
                % u64::from(phrase.steps);
            if shifted * u64::from(*pulses) % u64::from(phrase.steps) >= u64::from(*pulses) {
                return Ok(None);
            }
            let pulse = (shifted * u64::from(*pulses) / u64::from(phrase.steps)) as usize;
            make(
                pitches.get(pulse).copied().flatten().unwrap_or(*pitch),
                *velocity,
                *gate,
                target,
            )
        }
        PhraseForm::Random {
            seed,
            vary,
            density,
            low,
            high,
            velocity_low,
            velocity_high,
            gate,
            gate_high,
            target,
        } => {
            let random = random(
                *seed
                    ^ if *vary { owner.cycle } else { 0 }
                    ^ owner.phrase_use.get()
                    ^ owner.track.get().rotate_left(7)
                    ^ owner.chain_use.get().rotate_left(19)
                    ^ (u64::from(row) << 32),
            );
            if random % 100 >= u64::from(*density) {
                return Ok(None);
            }
            let key = phrase.key.as_ref().unwrap_or(key);
            let candidates: Vec<_> = (*low..=*high)
                .filter(|pitch| Pitch::new(*pitch).is_ok_and(|v| key.contains(v)))
                .collect();
            if candidates.is_empty() {
                return Err(DocumentError::Invalid(
                    "random phrase has no in-scale pitch candidates",
                )
                .into());
            }
            let pitch = candidates[(random.rotate_left(21) % candidates.len() as u64) as usize];
            let velocity = *velocity_low
                + (random.rotate_left(42) % u64::from(*velocity_high - *velocity_low + 1)) as u8;
            let gate = match gate_high {
                Some(high) => gate.checked_add(
                    high.checked_sub(*gate)?
                        .checked_scale(random.rotate_left(13) % 1025, 1024)?,
                )?,
                None => *gate,
            };
            make(pitch, velocity, gate, target)
        }
        _ => Ok(None),
    }
}
fn expand_event(
    event: &Event,
    at: Beat,
    mut owner: Owner,
    seed: u64,
    output: &mut StepOutput,
    budget: PlaybackBudget,
    chords: &ChordBank,
) -> Result<(), PlaybackError> {
    owner.event = event.id;
    for control in &event.controls {
        push(
            output,
            event,
            &owner,
            at,
            Message::Control { control: *control },
            budget,
        )?;
    }
    for (slot, fx) in event
        .fx
        .iter()
        .enumerate()
        .filter_map(|(slot, fx)| fx.as_ref().map(|fx| (slot, fx)))
    {
        match fx {
            Fx::Control { control } => push(
                output,
                event,
                &owner,
                at,
                Message::Control { control: *control },
                budget,
            )?,
            Fx::Send {
                bus,
                amount,
                duration,
            } => {
                push(
                    output,
                    event,
                    &owner,
                    at,
                    Message::Send {
                        bus: *bus,
                        amount: *amount,
                        slot: slot as u8,
                    },
                    budget,
                )?;
                push(
                    output,
                    event,
                    &owner,
                    at.checked_add(*duration)?,
                    Message::EndSend { slot: slot as u8 },
                    budget,
                )?;
            }
            Fx::Tempo { tempo } => push(
                output,
                event,
                &owner,
                at,
                Message::Tempo { tempo: *tempo },
                budget,
            )?,
            Fx::Jump { step } => output.jump = Some(*step),
            _ => {}
        }
    }
    let Some(note) = &event.note else {
        return Ok(());
    };
    let draw = random(
        seed ^ owner.track.get().rotate_left(7)
            ^ owner.chain_use.get().rotate_left(19)
            ^ owner.phrase_use.get().rotate_left(11)
            ^ owner.cycle.rotate_left(23)
            ^ event.id.get(),
    );
    if event
        .fx
        .iter()
        .flatten()
        .any(|fx| matches!(fx, Fx::Chance { percent } if draw % 100 >= u64::from(*percent)))
    {
        return Ok(());
    }
    let parameters = NoteParameters::resolve(note, &event.fx, at)?;
    let pitches = resolve_pitches(note.pitch, &event.fx, chords)?;
    if pitches
        .len()
        .saturating_mul(usize::from(parameters.repeats))
        > budget.events_per_window / 2
    {
        return Err(PlaybackError::Budget("expanded notes"));
    }
    for (voice, pitch) in pitches.into_iter().enumerate() {
        owner.voice = u16::try_from(voice).map_err(|_| PlaybackError::Budget("chord voices"))?;
        expand_voice(event, note, pitch, &owner, &parameters, output, budget)?;
    }
    Ok(())
}
struct NoteParameters {
    at: Beat,
    duration: Beat,
    velocity: u8,
    repeats: u16,
    spacing: Beat,
    expression_scale: Beat,
    expression_duration: Beat,
}
impl NoteParameters {
    fn resolve(note: &Note, effects: &[Option<Fx>; 3], at: Beat) -> Result<Self, PlaybackError> {
        let mut result = Self {
            at,
            duration: note.duration,
            velocity: note.velocity,
            repeats: 1,
            spacing: Beat::ZERO,
            expression_scale: Beat::ONE,
            expression_duration: note.expression_duration(),
        };
        for fx in effects.iter().flatten() {
            match fx {
                Fx::Delay { beats } => result.at = result.at.checked_add(*beats)?,
                Fx::Gate { fraction } => {
                    result.duration = note
                        .duration
                        .checked_scale(fraction.numerator(), fraction.denominator())?;
                    result.expression_scale = *fraction;
                    result.expression_duration = note
                        .expression_duration()
                        .checked_scale(fraction.numerator(), fraction.denominator())?;
                }
                Fx::Velocity { value } => result.velocity = *value,
                Fx::Retrigger { count, spacing } => {
                    result.repeats = *count;
                    result.spacing = *spacing;
                }
                _ => {}
            }
        }
        for fx in effects.iter().flatten() {
            if let Fx::Cut { after } = fx {
                result.duration = result.duration.min(*after);
                result.expression_duration = result.expression_duration.min(*after);
            }
        }
        Ok(result)
    }
}
fn expand_voice(
    event: &Event,
    note: &Note,
    pitch: u8,
    owner: &Owner,
    parameters: &NoteParameters,
    output: &mut StepOutput,
    budget: PlaybackBudget,
) -> Result<(), PlaybackError> {
    if parameters.duration.is_zero() {
        return Ok(());
    }
    for repeat in 0..parameters.repeats {
        let at = parameters
            .at
            .checked_add(parameters.spacing.checked_scale(u64::from(repeat), 1)?)?;
        let duration = if repeat + 1 < parameters.repeats {
            parameters.duration.min(parameters.spacing)
        } else {
            parameters.duration
        };
        push(
            output,
            event,
            owner,
            at,
            Message::NoteOn {
                pitch,
                velocity: parameters.velocity,
            },
            budget,
        )?;
        if note.expression_duration.is_some() {
            push(
                output,
                event,
                owner,
                at,
                Message::ExpressionLifetime {
                    duration: parameters.expression_duration,
                },
                budget,
            )?;
        }
        expand_expression(
            event,
            note,
            owner,
            at,
            if note.expression_duration.is_some() {
                parameters.expression_duration
            } else {
                duration
            },
            parameters.expression_scale,
            output,
            budget,
        )?;
        push(
            output,
            event,
            owner,
            at.checked_add(duration)?,
            Message::NoteOff {
                pitch,
                velocity: note.release_velocity,
            },
            budget,
        )?;
        if note.expression_duration.is_some() {
            let end = at.checked_add(parameters.expression_duration)?;
            push(
                output,
                event,
                owner,
                end,
                Message::Expression {
                    control: Control::Cc {
                        number: 64,
                        value: 0,
                    },
                },
                budget,
            )?;
            if note.mpe.is_some() {
                push(
                    output,
                    event,
                    owner,
                    end,
                    Message::MasterExpression {
                        control: Control::Cc {
                            number: 64,
                            value: 0,
                        },
                    },
                    budget,
                )?;
            }
            push(output, event, owner, end, Message::EndExpression, budget)?;
        }
    }
    Ok(())
}
#[allow(clippy::too_many_arguments)]
fn expand_expression(
    event: &Event,
    note: &Note,
    owner: &Owner,
    at: Beat,
    duration: Beat,
    scale: Beat,
    output: &mut StepOutput,
    budget: PlaybackBudget,
) -> Result<(), PlaybackError> {
    let mut points: Vec<_> = note
        .expression
        .iter()
        .map(|point| (point, false))
        .chain(note.master_expression.iter().map(|point| (point, true)))
        .collect();
    // Stable ordering preserves legacy vector order and exact captured order
    // across both scopes at one sample. Terminal owned cleanup sorts last.
    points.sort_by_key(|(point, _)| (point.offset, point.sequence));
    for (point, master) in points {
        let offset = point
            .offset
            .checked_scale(scale.numerator(), scale.denominator())?;
        if offset <= duration {
            let message = if master {
                Message::MasterExpression {
                    control: point.control,
                }
            } else {
                Message::Expression {
                    control: point.control,
                }
            };
            push(
                output,
                event,
                owner,
                at.checked_add(offset)?,
                message,
                budget,
            )?;
        }
    }
    Ok(())
}
fn resolve_pitches(
    root: u8,
    effects: &[Option<Fx>; 3],
    chords: &ChordBank,
) -> Result<Vec<u8>, PlaybackError> {
    let mut root = Pitch::new(root)?;
    let mut intervals = vec![0];
    let mut inversion = 0;
    let mut open = false;
    let mut spread = 0;
    for fx in effects.iter().flatten() {
        match fx {
            Fx::Chord { intervals: shape } => {
                intervals = shape.clone();
                inversion = 0;
                open = false;
                spread = 0;
            }
            Fx::ChordRevision { revision } => {
                let definition = chords
                    .get(revision)
                    .ok_or(DocumentError::Missing {
                        kind: "chord revision",
                        id: revision.get(),
                    })?
                    .definition();
                intervals = definition.shape().intervals().to_vec();
                inversion = usize::from(definition.inversion());
                open = definition.open();
                spread = usize::from(definition.spread());
            }
            _ => {}
        }
    }
    // Shape defaults come first; explicit voicing/spread override them once,
    // independent of their visual slot relative to the selected chord.
    for fx in effects.iter().flatten() {
        match fx {
            Fx::Offset { semitones } => root = root.transpose(i16::from(*semitones))?,
            Fx::Voicing {
                inversion: value,
                open: mode,
            } => {
                inversion = usize::from(*value);
                open = *mode;
            }
            Fx::Spread { voices } => spread = usize::from(*voices),
            _ => {}
        }
    }
    for fx in effects.iter().flatten() {
        if let Fx::Add { intervals: added } = fx {
            intervals.extend(added);
        }
    }
    intervals.sort_unstable();
    intervals.dedup();
    Ok(Chord::new(intervals)?
        .resolve(root, inversion, open, spread)?
        .into_iter()
        .map(Pitch::midi)
        .collect())
}
fn push(
    output: &mut StepOutput,
    event: &Event,
    owner: &Owner,
    at: Beat,
    message: Message,
    budget: PlaybackBudget,
) -> Result<(), PlaybackError> {
    if output.events.len() >= budget.events_per_window {
        return Err(PlaybackError::Budget("step output"));
    }
    output.events.push(TimedEvent {
        at,
        target: event.target.clone(),
        owner: owner.clone(),
        message,
    });
    Ok(())
}
fn random(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e3779b97f4a7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
    value ^ (value >> 31)
}
