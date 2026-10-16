use crate::kernel::patch_id::PatchId;
use crate::mixer::bus_id::{BusId, MAX_BUS_RETURNS};
use crate::mixer::global_parameters::GlobalParameters;
use crate::mixer::mix_engine::MixEngine;
use crate::mixer::mix_observation::MixObservation;
use crate::mixer::mixer_state::MixerState;
use crate::mixer::mixer_track_id::MixerTrackId;
use crate::mixer::mixer_track_parameters::MixerTrackParameters;
use crate::mixer::patch_output::PatchOutput;
use crate::real_time::{
    GraphRevision, ParameterSnapshot, PatchAudioBlock, RtBusReturnParameters, RtPatchParameters,
    RtPostEffectParameters,
};
use crate::synth::{EffectSlotId, PreparedEffectError, PreparedPostEffect};

const SAMPLE_RATE: f32 = 48_000.0;
const FRAME_COUNT: usize = 2;
const SAMPLE_COUNT: usize = FRAME_COUNT * 2;
const HALF_GAIN_DB: f32 = -6.020_600_3;
const EPSILON: f32 = 1.0e-6;
/// The floor reported when an off-target destination measures exact silence.
pub(crate) const SILENCE_DBFS: f32 = -200.0;
/// NFR-007 / SC-004: every off-target destination must sit below this level.
pub(crate) const ISOLATION_FLOOR_DBFS: f32 = -60.0;

/// Discriminating control-side measurements made by the production mixer.
///
/// The physical scene supplies generation-correlated window/device evidence;
/// these paired fixed-stem runs make the signal-order predicates falsifiable
/// without relying on oscillator phase or effect-tail timing.
///
/// There is no named shared-effects port to probe, so every send is measured
/// through [`MixObservation`]'s indexed per-bus measurements and sample-exact
/// unity returns installed on the production bus-return rack. The measurement
/// covers all eight destinations (SC-004, NFR-007) with numeric isolation
/// evidence, many-to-one accumulation, gate behavior (SC-005), and
/// unoccupied-return silence.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LiveMixerDspEvidence {
    pub(crate) shared_track_sum_exact: bool,
    pub(crate) track_level_controls_shared_sum: bool,
    pub(crate) patch_trim_isolated: bool,
    pub(crate) patch_reroute_isolated: bool,
    pub(crate) mute_wins: bool,
    pub(crate) any_solo_exact: bool,
    pub(crate) post_gate_sends_exact: bool,
    pub(crate) pre_gate_meters_exact: bool,
    /// One send raised toward each of the eight destinations in turn leaves
    /// every other destination at exact silence.
    pub(crate) send_isolation_exact: bool,
    /// Worst off-target destination level observed across all eight
    /// isolation runs, in dBFS ([`SILENCE_DBFS`] for exact silence).
    pub(crate) max_off_target_bus_dbfs: f32,
    /// Two tracks into one bus sum exactly, each scaled by its own send.
    pub(crate) accumulation_exact: bool,
    /// Worst wet contribution measured from muted and solo-excluded tracks
    /// (RMS; must be exactly zero — SC-005).
    pub(crate) gated_wet_contribution: f32,
    /// A raised send toward an unoccupied return accumulates at the bus but
    /// contributes silence, never its input (C-BR-6).
    pub(crate) unoccupied_return_silent: bool,
}

impl Default for LiveMixerDspEvidence {
    fn default() -> Self {
        Self {
            shared_track_sum_exact: false,
            track_level_controls_shared_sum: false,
            patch_trim_isolated: false,
            patch_reroute_isolated: false,
            mute_wins: false,
            any_solo_exact: false,
            post_gate_sends_exact: false,
            pre_gate_meters_exact: false,
            send_isolation_exact: false,
            max_off_target_bus_dbfs: 0.0,
            accumulation_exact: false,
            gated_wet_contribution: f32::INFINITY,
            unoccupied_return_silent: false,
        }
    }
}

impl LiveMixerDspEvidence {
    pub(crate) fn measure() -> Self {
        Self::try_measure().unwrap_or_default()
    }

    #[allow(clippy::too_many_lines)]
    fn try_measure() -> Option<Self> {
        let shared_track = track(3)?;
        let reroute_track = track(4)?;
        let first_stem = [0.2, 0.4, 0.2, 0.4];
        let second_stem = [0.3, 0.1, 0.3, 0.1];
        let shared_outputs = [
            PatchOutput::to_track(shared_track),
            PatchOutput::to_track(shared_track),
        ];

        let shared = run_mix(
            &shared_outputs,
            MixerState::default(),
            &[first_stem, second_stem],
            &[],
        )?;
        let shared_track_sum_exact = samples_equal(&shared.output, &[0.5; SAMPLE_COUNT])
            && approximately(shared.observation.track(shared_track).rms(), 0.5)
            && MixerTrackId::ALL
                .iter()
                .filter(|track_id| **track_id != shared_track)
                .all(|track_id| shared.observation.track(*track_id).rms() == 0.0);

        let half_level = MixerState::default().with_track(
            shared_track,
            MixerTrackParameters::default()
                .with_scalar_value(
                    crate::mixer::mixer_track_parameters::MixerTrackParameter::Level,
                    HALF_GAIN_DB,
                )
                .ok()?,
        );
        let shared_half = run_mix(&shared_outputs, half_level, &[first_stem, second_stem], &[])?;
        let track_level_controls_shared_sum =
            samples_equal(&shared_half.output, &[0.25; SAMPLE_COUNT])
                && approximately(
                    shared_half.observation.track(shared_track).rms(),
                    shared.observation.track(shared_track).rms() * 0.5,
                )
                && MixerTrackId::ALL
                    .iter()
                    .filter(|track_id| **track_id != shared_track)
                    .all(|track_id| shared_half.observation.track(*track_id).rms() == 0.0);

        let trimmed_outputs = [
            PatchOutput::new(shared_track, HALF_GAIN_DB).ok()?,
            PatchOutput::to_track(shared_track),
        ];
        let trimmed = run_mix(
            &trimmed_outputs,
            MixerState::default(),
            &[first_stem, second_stem],
            &[],
        )?;
        let patch_trim_isolated = samples_equal(&trimmed.output, &[0.4, 0.3, 0.4, 0.3])
            && approximately(
                trimmed.observation.track(shared_track).rms(),
                0.125_f32.sqrt(),
            )
            && MixerTrackId::ALL
                .iter()
                .filter(|track_id| **track_id != shared_track)
                .all(|track_id| trimmed.observation.track(*track_id).rms() == 0.0);

        let rerouted_outputs = [
            PatchOutput::to_track(shared_track),
            PatchOutput::to_track(reroute_track),
        ];
        let rerouted = run_mix(
            &rerouted_outputs,
            MixerState::default(),
            &[first_stem, second_stem],
            &[],
        )?;
        let patch_reroute_isolated = samples_equal(&rerouted.output, &shared.output)
            && approximately(
                rerouted.observation.track(shared_track).rms(),
                0.1_f32.sqrt(),
            )
            && approximately(
                rerouted.observation.track(reroute_track).rms(),
                0.05_f32.sqrt(),
            )
            && MixerTrackId::ALL
                .iter()
                .filter(|track_id| **track_id != shared_track && **track_id != reroute_track)
                .all(|track_id| rerouted.observation.track(*track_id).rms() == 0.0);

        // The retired per-name send checks, now indexed: sends[0] = 0.25 and
        // sends[1] = 0.5 with unity returns installed at buses 0 and 1. The
        // post-fader stem is [0.2, 0.1, ...], so the bus inputs are exactly
        // 0.25x and 0.5x of it, and the mixed output is dry + both wet sums.
        let send_stem = [0.4, 0.2, 0.4, 0.2];
        let send_buses = [BusId::ALL[0], BusId::ALL[1]];
        let send_parameters = MixerTrackParameters::from_values(
            HALF_GAIN_DB,
            0.0,
            false,
            false,
            [0.25, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        )
        .ok()?;
        let sends = run_mix(
            &[PatchOutput::to_track(shared_track)],
            MixerState::default().with_track(shared_track, send_parameters),
            &[send_stem],
            &send_buses,
        )?;
        let sounding_meter = sends.observation.track(shared_track);
        let post_gate_sends_exact = samples_equal(&sends.output, &[0.35, 0.175, 0.35, 0.175])
            && approximately(
                sends
                    .observation
                    .bus_input_rms(send_buses[0])
                    .expect("bus belongs to observed default bank"),
                rms(&[0.05, 0.025, 0.05, 0.025]),
            )
            && approximately(
                sends
                    .observation
                    .bus_input_rms(send_buses[1])
                    .expect("bus belongs to observed default bank"),
                rms(&[0.1, 0.05, 0.1, 0.05]),
            )
            && approximately(
                sends
                    .observation
                    .bus_output_rms(send_buses[0])
                    .expect("bus belongs to observed default bank"),
                rms(&[0.05, 0.025, 0.05, 0.025]),
            )
            && approximately(
                sends
                    .observation
                    .bus_output_rms(send_buses[1])
                    .expect("bus belongs to observed default bank"),
                rms(&[0.1, 0.05, 0.1, 0.05]),
            );
        let sounding_meter_exact = approximately(sounding_meter.left_peak(), 0.2)
            && approximately(sounding_meter.right_peak(), 0.1)
            && approximately(sounding_meter.rms(), 0.025_f32.sqrt());

        let muted_parameters = MixerTrackParameters::from_values(
            HALF_GAIN_DB,
            0.0,
            true,
            false,
            [0.25, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        )
        .ok()?;
        let muted = run_mix(
            &[PatchOutput::to_track(shared_track)],
            MixerState::default().with_track(shared_track, muted_parameters),
            &[send_stem],
            &send_buses,
        )?;
        let mute_wins = samples_equal(&muted.output, &[0.0; SAMPLE_COUNT])
            && BusId::ALL.iter().all(|bus| {
                muted
                    .observation
                    .bus_input_rms(*bus)
                    .expect("bus belongs to observed default bank")
                    == 0.0
            })
            && BusId::ALL.iter().all(|bus| {
                muted
                    .observation
                    .bus_output_rms(*bus)
                    .expect("bus belongs to observed default bank")
                    == 0.0
            });
        let pre_gate_meters_exact = sounding_meter_exact
            && meters_equal(muted.observation, sends.observation, shared_track);

        let solo_parameters =
            MixerTrackParameters::from_values(0.0, 0.0, false, true, [0.0; MAX_BUS_RETURNS])
                .ok()?;
        let soloed = run_mix(
            &rerouted_outputs,
            MixerState::default().with_track(shared_track, solo_parameters),
            &[first_stem, second_stem],
            &[],
        )?;
        let muted_solo_parameters =
            MixerTrackParameters::from_values(0.0, 0.0, true, true, [0.5; MAX_BUS_RETURNS]).ok()?;
        let muted_solo = run_mix(
            &rerouted_outputs,
            MixerState::default().with_track(shared_track, muted_solo_parameters),
            &[first_stem, second_stem],
            &[],
        )?;
        let any_solo_exact = samples_equal(&soloed.output, &first_stem)
            && samples_equal(&muted_solo.output, &[0.0; SAMPLE_COUNT])
            && muted_solo.observation.track(shared_track).rms() > 0.0
            && muted_solo.observation.track(reroute_track).rms() > 0.0;

        // SC-004 / NFR-007: raise one send toward each destination in turn
        // with every return occupied; the other seven destinations must stay
        // at exact silence, reported numerically in dBFS.
        let mut send_isolation_exact = true;
        let mut max_off_target_linear = 0.0_f32;
        for target in BusId::ALL {
            let isolated_parameters = MixerTrackParameters::default()
                .with_send(target, 0.8)
                .ok()?;
            let isolated = run_mix(
                &[PatchOutput::to_track(shared_track)],
                MixerState::default().with_track(shared_track, isolated_parameters),
                &[send_stem],
                &BusId::ALL,
            )?;
            let expected_bus = [0.32, 0.16, 0.32, 0.16];
            send_isolation_exact &= approximately(
                isolated
                    .observation
                    .bus_input_rms(target)
                    .expect("bus belongs to observed default bank"),
                rms(&expected_bus),
            ) && approximately(
                isolated
                    .observation
                    .bus_output_rms(target)
                    .expect("bus belongs to observed default bank"),
                rms(&expected_bus),
            ) && samples_equal(&isolated.output, &[0.72, 0.36, 0.72, 0.36]);
            for other in BusId::ALL {
                if other != target {
                    let off_target = isolated
                        .observation
                        .bus_input_rms(other)
                        .expect("bus belongs to observed default bank")
                        .max(
                            isolated
                                .observation
                                .bus_output_rms(other)
                                .expect("bus belongs to observed default bank"),
                        );
                    send_isolation_exact &= off_target == 0.0;
                    max_off_target_linear = max_off_target_linear.max(off_target);
                }
            }
        }
        let max_off_target_bus_dbfs = linear_to_dbfs(max_off_target_linear);

        // C-BR-4: two tracks into one bus sum exactly, each scaled by its own
        // send: 0.5 * [0.2, 0.4, ...] + 0.25 * [0.3, 0.1, ...].
        let accumulation_bus = BusId::ALL[5];
        let accumulation_state = MixerState::default()
            .with_track(
                shared_track,
                MixerTrackParameters::default()
                    .with_send(accumulation_bus, 0.5)
                    .ok()?,
            )
            .with_track(
                reroute_track,
                MixerTrackParameters::default()
                    .with_send(accumulation_bus, 0.25)
                    .ok()?,
            );
        let accumulated = run_mix(
            &rerouted_outputs,
            accumulation_state,
            &[first_stem, second_stem],
            &[accumulation_bus],
        )?;
        let accumulated_bus = [
            0.2 * 0.5 + 0.3 * 0.25,
            0.4 * 0.5 + 0.1 * 0.25,
            0.2 * 0.5 + 0.3 * 0.25,
            0.4 * 0.5 + 0.1 * 0.25,
        ];
        let accumulation_exact = approximately(
            accumulated
                .observation
                .bus_input_rms(accumulation_bus)
                .expect("bus belongs to observed default bank"),
            rms(&accumulated_bus),
        ) && samples_equal(
            &accumulated.output,
            &[
                0.5 + accumulated_bus[0],
                0.5 + accumulated_bus[1],
                0.5 + accumulated_bus[2],
                0.5 + accumulated_bus[3],
            ],
        );

        // SC-005: muted and solo-excluded tracks contribute zero wet signal
        // toward every occupied destination, measured numerically.
        let gate_bus = BusId::ALL[6];
        let muted_full_sends =
            MixerTrackParameters::from_values(0.0, 0.0, true, false, [1.0; MAX_BUS_RETURNS])
                .ok()?;
        let gated_muted = run_mix(
            &[PatchOutput::to_track(shared_track)],
            MixerState::default().with_track(shared_track, muted_full_sends),
            &[send_stem],
            &BusId::ALL,
        )?;
        let solo_excluded_state = MixerState::default()
            .with_track(
                shared_track,
                MixerTrackParameters::from_values(0.0, 0.0, false, true, [0.0; MAX_BUS_RETURNS])
                    .ok()?,
            )
            .with_track(
                reroute_track,
                MixerTrackParameters::default()
                    .with_send(gate_bus, 1.0)
                    .ok()?,
            );
        let gated_excluded = run_mix(
            &rerouted_outputs,
            solo_excluded_state,
            &[first_stem, second_stem],
            &BusId::ALL,
        )?;
        let gated_wet_contribution = BusId::ALL
            .iter()
            .map(|bus| {
                gated_muted
                    .observation
                    .bus_output_rms(*bus)
                    .expect("bus belongs to observed default bank")
                    .max(
                        gated_excluded
                            .observation
                            .bus_output_rms(*bus)
                            .expect("bus belongs to observed default bank"),
                    )
                    .max(
                        gated_muted
                            .observation
                            .bus_input_rms(*bus)
                            .expect("bus belongs to observed default bank"),
                    )
                    .max(
                        gated_excluded
                            .observation
                            .bus_input_rms(*bus)
                            .expect("bus belongs to observed default bank"),
                    )
            })
            .fold(0.0_f32, f32::max);

        // C-BR-6: a raised send toward an unoccupied return accumulates at
        // the bus, but the empty return adds nothing to the mix.
        let unoccupied_bus = BusId::ALL[7];
        let unoccupied = run_mix(
            &[PatchOutput::to_track(shared_track)],
            MixerState::default().with_track(
                shared_track,
                MixerTrackParameters::default()
                    .with_send(unoccupied_bus, 1.0)
                    .ok()?,
            ),
            &[send_stem],
            &[],
        )?;
        let unoccupied_return_silent = approximately(
            unoccupied
                .observation
                .bus_input_rms(unoccupied_bus)
                .expect("bus belongs to observed default bank"),
            rms(&send_stem),
        ) && unoccupied
            .observation
            .bus_output_rms(unoccupied_bus)
            .expect("bus belongs to observed default bank")
            == 0.0
            && samples_equal(&unoccupied.output, &send_stem);

        Some(Self {
            shared_track_sum_exact,
            track_level_controls_shared_sum,
            patch_trim_isolated,
            patch_reroute_isolated,
            mute_wins,
            any_solo_exact,
            post_gate_sends_exact,
            pre_gate_meters_exact,
            send_isolation_exact,
            max_off_target_bus_dbfs,
            accumulation_exact,
            gated_wet_contribution,
            unoccupied_return_silent,
        })
    }
}

/// A sample-exact unity return: leaves its input unchanged so the rack's wet
/// contribution equals the accumulated bus input times the return level.
struct UnityReturn;

impl PreparedPostEffect for UnityReturn {
    fn patch_id(&self) -> PatchId {
        PatchId::new(u32::MAX).expect("the static measurement Patch id is non-zero")
    }

    fn slot_id(&self) -> EffectSlotId {
        EffectSlotId::new(1).expect("the static measurement slot id is non-zero")
    }

    fn process(
        &mut self,
        _interleaved_stereo: &mut [f32],
        _frame_count: usize,
        _parameters: &RtPostEffectParameters,
    ) -> Result<(), PreparedEffectError> {
        Ok(())
    }
}

struct MixRun {
    output: [f32; SAMPLE_COUNT],
    observation: MixObservation,
}

fn unity_parameters() -> Option<RtPostEffectParameters> {
    RtPostEffectParameters::new(EffectSlotId::new(1).ok()?, &[]).ok()
}

fn run_mix(
    outputs: &[PatchOutput],
    mixer_state: MixerState,
    stems: &[[f32; SAMPLE_COUNT]],
    unity_return_buses: &[BusId],
) -> Option<MixRun> {
    if outputs.len() != stems.len() {
        return None;
    }
    let patches = outputs
        .iter()
        .copied()
        .enumerate()
        .map(|(index, output)| {
            PatchId::new(index as u32 + 1)
                .ok()
                .map(|patch_id| RtPatchParameters::new(patch_id, output))
        })
        .collect::<Option<Vec<_>>>()?;
    let mut returns = [const { RtBusReturnParameters::EMPTY }; MAX_BUS_RETURNS];
    for bus in unity_return_buses {
        returns[bus.index()] =
            RtBusReturnParameters::new(EffectSlotId::new(1).ok()?, &[], 1.0).ok()?;
    }
    let parameters =
        ParameterSnapshot::for_graph(1, GraphRevision::INITIAL, globals()?, mixer_state, &patches)
            .ok()?
            .with_returns(returns);
    let mut patch_audio = PatchAudioBlock::prepare(FRAME_COUNT).ok()?;
    patch_audio.begin_render(&parameters, FRAME_COUNT).ok()?;
    for (index, (patch, stem)) in parameters.patches().iter().zip(stems).enumerate() {
        patch_audio
            .stem_mut(index, patch.patch_id()?)?
            .copy_from_slice(stem);
    }

    let mut mixer = MixEngine::new();
    mixer.prepare(SAMPLE_RATE, FRAME_COUNT).ok()?;
    for bus in unity_return_buses {
        mixer
            .install_bus_return(*bus, Box::new(UnityReturn), unity_parameters()?, 1.0)
            .ok()?;
    }
    let mut output = [0.0; SAMPLE_COUNT];
    let observation = mixer.mix(&patch_audio, &parameters, &mut output);
    Some(MixRun {
        output,
        observation,
    })
}

fn globals() -> Option<GlobalParameters> {
    GlobalParameters::new(0.0).ok()
}

fn track(index: u8) -> Option<MixerTrackId> {
    MixerTrackId::new(index).ok()
}

fn approximately(actual: f32, expected: f32) -> bool {
    actual.is_finite() && expected.is_finite() && (actual - expected).abs() <= EPSILON
}

fn samples_equal(actual: &[f32], expected: &[f32]) -> bool {
    actual.len() == expected.len()
        && actual
            .iter()
            .zip(expected)
            .all(|(actual, expected)| approximately(*actual, *expected))
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let energy: f64 = samples
        .iter()
        .map(|sample| f64::from(*sample) * f64::from(*sample))
        .sum();
    (energy / samples.len() as f64).sqrt() as f32
}

pub(crate) fn linear_to_dbfs(linear: f32) -> f32 {
    if linear <= 0.0 {
        SILENCE_DBFS
    } else {
        (20.0 * linear.log10()).max(SILENCE_DBFS)
    }
}

fn meters_equal(actual: MixObservation, expected: MixObservation, track_id: MixerTrackId) -> bool {
    let actual = actual.track(track_id);
    let expected = expected.track(track_id);
    approximately(actual.left_peak(), expected.left_peak())
        && approximately(actual.right_peak(), expected.right_peak())
        && approximately(actual.rms(), expected.rms())
}
