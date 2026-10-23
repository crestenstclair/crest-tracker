//! Imported-song Mixer regression through the same semantic events as a controller.
use super::*;
use crest_synth::control::{
    GraphicalShellProjection, MixerControlId, SemanticControlId, SemanticControlValue, SurfaceId,
};
use crest_synth::mixer::mixer_track_parameters::{MixerTrackParameter, MixerTrackParameterKind};
use crest_synth::synth::ParameterValue;
use std::path::Path;

pub(super) struct Checkpoint {
    track: u8,
    parameter: MixerTrackParameter,
    adjustment: Option<(Direction, usize)>,
}

#[derive(Serialize)]
pub(super) struct Observation {
    track: u8,
    parameter: MixerTrackParameter,
    value: f64,
    beat: Beat,
    rendered_frames: u64,
    output_rms: f32,
    graph_revision: u64,
}

impl Scene {
    pub fn mixer(path: &Path) -> Result<Self, String> {
        use Direction::*;
        use SemanticAction::*;
        let mut scene = Self::song(path)?;
        scene.report.scene = "song-mixer";
        let finished = scene
            .steps
            .pop()
            .ok_or("Missing Song completion checkpoint")?;
        scene.steps.push(Step {
            label: "Open main menu while playing",
            event: Event::Activate("menu"),
            check: Check::Panel("mixer"),
        });
        for track in 0..4 {
            for parameter in MixerTrackParameter::MAIN {
                if track > 0 && parameter == MixerTrackParameter::Level {
                    scene.steps.push(Step {
                        label: "Move to next Mixer track",
                        event: Event::Action(Navigate(Right)),
                        check: Check::Mixer(Checkpoint {
                            track,
                            parameter: MixerTrackParameter::Solo,
                            adjustment: None,
                        }),
                    });
                    for parameter in [MixerTrackParameter::Mute, MixerTrackParameter::Pan] {
                        scene.steps.push(Step {
                            label: "Navigate track controls while playing",
                            event: Event::Action(Navigate(Up)),
                            check: Check::Mixer(Checkpoint {
                                track,
                                parameter,
                                adjustment: None,
                            }),
                        });
                    }
                }
                let focus = match (track, parameter) {
                    (0, MixerTrackParameter::Level) => Event::Activate("mixer"),
                    (_, MixerTrackParameter::Level) => Event::Action(Navigate(Up)),
                    _ => Event::Action(Navigate(Down)),
                };
                scene.steps.push(Step {
                    label: "Select Mixer control during playback",
                    event: focus,
                    check: Check::Mixer(Checkpoint {
                        track,
                        parameter,
                        adjustment: None,
                    }),
                });
                scene.steps.push(Step {
                    label: "Hold Edit in Mixer",
                    event: Event::Action(SetInteractionMode(InteractionMode::Adjust)),
                    check: Check::Mixer(Checkpoint {
                        track,
                        parameter,
                        adjustment: None,
                    }),
                });
                let directions = match parameter {
                    MixerTrackParameter::Level => [Down, Up],
                    _ => [Right, Left],
                };
                for direction in directions {
                    let toggle = parameter.descriptor().kind() == MixerTrackParameterKind::Toggle;
                    let repeats = if toggle { 1 } else { 2 };
                    let batch = if toggle { 1 } else { 4 };
                    for _ in 0..repeats {
                        scene.steps.push(Step {
                            label: "Adjust Mixer without interrupting playback",
                            event: Event::Repeat(Adjust(direction), batch),
                            check: Check::Mixer(Checkpoint {
                                track,
                                parameter,
                                adjustment: Some((direction, batch)),
                            }),
                        });
                    }
                }
                scene.steps.push(Step {
                    label: "Release Edit in Mixer",
                    event: Event::Action(SetInteractionMode(InteractionMode::Navigate)),
                    check: Check::Mixer(Checkpoint {
                        track,
                        parameter,
                        adjustment: None,
                    }),
                });
            }
        }
        scene.steps.extend([
            Step {
                label: "Return from Mixer to main menu while playing",
                event: Event::Action(NavigatePage(Down)),
                check: Check::Panel("home"),
            },
            Step {
                label: "Return to Song with playback intact",
                event: Event::Activate("home"),
                check: Check::Playing,
            },
            finished,
        ]);
        Ok(scene)
    }
}

impl Checkpoint {
    pub(super) fn dwell(&self) -> Duration {
        // Match the held-direction repeat interval, not a human-readable tour pause.
        Duration::from_millis(if self.adjustment.is_some() { 45 } else { 400 })
    }
}

pub(super) fn observe(
    runtime: &Runtime,
    projection: &GraphicalShellProjection,
    audio: AudioObservationSnapshot,
) -> Result<Observation, String> {
    let app = runtime.app.borrow();
    if !app.playing {
        return Err(format!("Mixer interrupted playback: {}", app.status));
    }
    let model = projection.semantic_model();
    if model.active_surface() != SurfaceId::MixerMain || crate::workspace::owns_input(&app) {
        return Err("Mixer scene did not reach native Mixer".into());
    }
    let control = model
        .focused_control()
        .ok_or("Mixer has no focused control")?;
    let SemanticControlId::Mixer(MixerControlId::Track {
        track_id,
        parameter,
    }) = control.path().control_id()
    else {
        return Err("Expected Mixer track control".into());
    };
    let value = match control.value() {
        SemanticControlValue::Scalar(value) => *value,
        SemanticControlValue::Parameter(ParameterValue::Toggle(value)) => u8::from(*value) as f64,
        _ => return Err("Expected numeric or toggle Mixer value".into()),
    };
    Ok(Observation {
        track: track_id.value(),
        parameter: *parameter,
        value,
        beat: app.play_beat.ok_or("Missing playback cursor in Mixer")?,
        rendered_frames: audio.rendered_frames(),
        output_rms: audio.output_rms(),
        graph_revision: runtime.graph_revision,
    })
}

pub(super) fn check(
    runtime: &Runtime,
    report: &Report,
    checkpoint: &Checkpoint,
    audio: AudioObservationSnapshot,
    projection: Option<&GraphicalShellProjection>,
) -> Result<bool, String> {
    let Some(projection) = projection else {
        return Ok(false);
    };
    if !runtime.app.borrow().requests.is_empty() {
        return Ok(false);
    }
    let current = observe(runtime, projection, audio)?;
    if current.track != checkpoint.track || current.parameter != checkpoint.parameter {
        return Err(format!(
            "Wrong Mixer focus: expected {} {:?}, got {} {:?}",
            checkpoint.track, checkpoint.parameter, current.track, current.parameter
        ));
    }
    if let Some(previous) = report.mixer.last() {
        if current.beat <= previous.beat || current.rendered_frames <= previous.rendered_frames {
            return Err("Playback cursor stopped advancing during Mixer edit".into());
        }
        if let Some((direction, count)) = checkpoint.adjustment {
            let descriptor = checkpoint.parameter.descriptor();
            let expected = if descriptor.kind() == MixerTrackParameterKind::Toggle {
                1.0 - previous.value
            } else {
                let step = match direction {
                    Direction::Left => -descriptor.fine_step(),
                    Direction::Right => descriptor.fine_step(),
                    Direction::Down => -descriptor.coarse_step(),
                    Direction::Up => descriptor.coarse_step(),
                };
                (previous.value + f64::from(step) * count as f64).clamp(
                    f64::from(descriptor.minimum()),
                    f64::from(descriptor.maximum()),
                )
            };
            if (current.value - expected).abs() > 0.00001 || current.value == previous.value {
                return Err(format!(
                    "Mixer edit did not apply: expected {expected}, got {}",
                    current.value
                ));
            }
        }
    }
    Ok(true)
}
