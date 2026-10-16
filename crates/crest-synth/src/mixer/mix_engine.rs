use crate::mixer::bus_id::{BusId, MAX_BUS_RETURNS};
use crate::mixer::bus_return::EffectError;
use crate::mixer::mix_observation::MixObservation;
use crate::mixer::mixer_track_id::MixerTrackId;
use crate::mixer::track_meter::TrackMeter;
use crate::real_time::parameter_snapshot::ParameterSnapshot;
use crate::real_time::patch_audio_block::PatchAudioBlock;
use crate::real_time::prepared_bus_return_rack::PreparedBusReturnRack;
use crate::real_time::RtPostEffectParameters;
use crate::synth::PreparedPostEffect;

/// Combines identity-preserving Patch stems, eight indexed bus returns, and
/// the master stage.
///
/// Sends are taken post-fader and post-gate and are indexed by `BusId`; the
/// send stage did not move when the destination count generalized from two to
/// eight (FR-011, C-BR-1). Each occupied return sums into the mix before
/// master gain; an unoccupied return contributes silence (C-BR-6).
///
/// There is no named shared-effects port: every shared return renders through
/// the bus-return rack, with live scalar values and levels read from the
/// snapshot's indexed return entries, and the measurement harnesses probe the
/// per-bus sends through [`MixObservation`] and installed returns.
pub struct MixEngine {
    returns: PreparedBusReturnRack,
    track_scratch: [Vec<f32>; MixerTrackId::COUNT],
    bus_inputs: Vec<Vec<f32>>,
    wet_scratch: Vec<f32>,
    dry_output: Vec<f32>,
    max_frames: usize,
    prepared: bool,
}

impl Default for MixEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl MixEngine {
    /// Creates an unprepared mixer without allocating callback storage.
    #[must_use]
    pub fn new() -> Self {
        Self {
            returns: PreparedBusReturnRack::empty(),
            track_scratch: std::array::from_fn(|_| Vec::new()),
            bus_inputs: Vec::new(),
            wet_scratch: Vec::new(),
            dry_output: Vec::new(),
            max_frames: 0,
            prepared: false,
        }
    }

    /// Allocates every scratch buffer and prepares the effect stages.
    ///
    /// Re-preparation empties the bus-return rack; occupancy is installed
    /// afterward through [`Self::install_bus_return`].
    pub fn prepare(&mut self, _sample_rate: f32, max_frames: usize) -> Result<(), EffectError> {
        self.prepare_with_returns(_sample_rate, max_frames, MAX_BUS_RETURNS)
    }

    pub fn prepare_with_returns(
        &mut self,
        _sample_rate: f32,
        max_frames: usize,
        return_count: usize,
    ) -> Result<(), EffectError> {
        self.prepared = false;

        // The rack owns return-count validation. Check it before allocating
        // per-bus scratch so every later index fits the BusId representation.
        let returns = PreparedBusReturnRack::with_count(max_frames, return_count)?;
        let sample_capacity = max_frames
            .checked_mul(2)
            .ok_or(EffectError::StorageAllocationFailed)?;
        let mut bus_inputs = vec![Vec::new(); return_count];
        for bus_input in &mut bus_inputs {
            *bus_input = allocate_zeros(sample_capacity)?;
        }
        let wet_scratch = allocate_zeros(sample_capacity)?;
        let dry_output = allocate_zeros(sample_capacity)?;
        let mut track_scratch = std::array::from_fn(|_| Vec::new());
        for track in &mut track_scratch {
            *track = allocate_zeros(sample_capacity)?;
        }

        self.bus_inputs = bus_inputs;
        self.wet_scratch = wet_scratch;
        self.dry_output = dry_output;
        self.track_scratch = track_scratch;
        self.returns = returns;
        self.max_frames = max_frames;
        self.prepared = true;
        Ok(())
    }

    /// Prepare-time only: occupies one bus return with a prepared registry
    /// effect, its install-time structural attestation (instance identity and
    /// scalar layout), and the install-time return level. Live values arrive
    /// per block from the snapshot's indexed return entries.
    pub fn install_bus_return(
        &mut self,
        bus: BusId,
        effect: Box<dyn PreparedPostEffect>,
        parameters: RtPostEffectParameters,
        return_level: f32,
    ) -> Result<(), EffectError> {
        self.returns.install(bus, effect, parameters, return_level)
    }

    /// Prepare-time only: empties one bus return.
    pub fn clear_bus_return(&mut self, bus: BusId) {
        self.returns.clear(bus);
    }

    pub const fn bus_returns(&self) -> &PreparedBusReturnRack {
        &self.returns
    }

    /// Mutable access for the graph-owned voice carry-over exchange:
    /// `PreparedGraph::carry_live_state_from` swaps still-live return
    /// instances between two complete graphs at block-boundary activation.
    /// The rack's own carry operation is callback-safe; this accessor adds no
    /// behavior of its own.
    pub(crate) fn bus_returns_mut(&mut self) -> &mut PreparedBusReturnRack {
        &mut self.returns
    }

    /// Mixes one prepared stereo stem per active Patch into the stereo output.
    ///
    /// Each stem must match the Patch identity and index in `parameters`.
    /// A missing or mismatched stem silences the complete output rather than
    /// substituting another Patch's audio or parameters.
    pub fn mix(
        &mut self,
        patch_audio: &PatchAudioBlock,
        parameters: &ParameterSnapshot,
        output: &mut [f32],
    ) -> MixObservation {
        self.mix_with_send_leases(patch_audio, parameters, output, None)
    }

    pub(crate) fn mix_with_send_leases(
        &mut self,
        patch_audio: &PatchAudioBlock,
        parameters: &ParameterSnapshot,
        output: &mut [f32],
        leases: Option<&crate::real_time::SendLeases>,
    ) -> MixObservation {
        output.fill(0.0);
        if !self.prepared {
            return MixObservation::default();
        }

        let frame_count = (output.len() / 2).min(self.max_frames);
        let sample_count = frame_count * 2;
        if patch_audio.frame_count() != frame_count
            || patch_audio.patch_count() != parameters.patch_count()
        {
            return MixObservation::default();
        }

        for (index, patch) in parameters.patches().iter().enumerate() {
            let Some(patch_id) = patch.patch_id() else {
                return MixObservation::default();
            };
            let Some(stem) = patch_audio.stem(index, patch_id) else {
                return MixObservation::default();
            };
            if stem.frame_count() != frame_count {
                return MixObservation::default();
            }
        }

        for bus_input in &mut self.bus_inputs {
            bus_input[..sample_count].fill(0.0);
        }
        for track in &mut self.track_scratch {
            track[..sample_count].fill(0.0);
        }

        let any_solo = parameters.mixer_tracks().iter().any(|track| track.solo());
        // Patch trim and many-to-one dry destination accumulation. Each Patch
        // independently feeds shared returns before its identity is lost in
        // the track sum, using that track's fader, pan, and mute/solo gate.
        let mut occupied_tracks = [false; MixerTrackId::COUNT];
        for (index, patch) in parameters.patches().iter().enumerate() {
            let Some(patch_id) = patch.patch_id() else {
                output.fill(0.0);
                return MixObservation::default();
            };
            let Some(stem) = patch_audio.stem(index, patch_id) else {
                output.fill(0.0);
                return MixObservation::default();
            };
            let audio = stem.samples();
            let patch_output = patch.output();
            occupied_tracks[patch_output.track_id().index()] = true;
            let trim = db_to_linear(patch_output.trim_gain_db());
            let track = &mut self.track_scratch[patch_output.track_id().index()];
            for sample_index in 0..sample_count {
                track[sample_index] += audio[sample_index] * trim;
            }
            let track_parameters = parameters.mixer_track(patch_output.track_id());
            if !track_parameters.mute() && (!any_solo || track_parameters.solo()) {
                let gain = db_to_linear(track_parameters.level_db());
                let (left_pan, right_pan) = pan_gains(track_parameters.pan());
                let left_gain = gain * left_pan;
                let right_gain = gain * right_pan;
                for (index, (bus_input, base)) in
                    self.bus_inputs.iter_mut().zip(patch.sends()).enumerate()
                {
                    let send = crate::mixer::bus_id::BusId::new(index as u16)
                        .ok()
                        .and_then(|bus| leases.and_then(|leases| leases.amount(patch_id, bus)))
                        .unwrap_or(*base);
                    if send == 0.0 {
                        continue;
                    }
                    for frame in 0..frame_count {
                        let left = frame * 2;
                        let right = left + 1;
                        bus_input[left] += finite_or_zero(audio[left] * trim * left_gain) * send;
                        bus_input[right] += finite_or_zero(audio[right] * trim * right_gain) * send;
                    }
                }
            }
        }

        let mut track_meters = [TrackMeter::default(); MixerTrackId::COUNT];
        let mut non_finite_samples = 0_u64;

        // Track level/pan, pre-gate meters, gates, dry sum, and post-gate sends.
        // The stage order is FR-011 and C-BR-1/C-BR-2 and must not move:
        // fader and pan first, the meter next (pre-gate, so muted tracks stay
        // diagnosable), then the mute/solo gate, and only for audible tracks
        // the dry sum and the eight indexed sends.
        for track_id in MixerTrackId::ALL {
            // No Patch stem can feed this track. Its scratch and meter are
            // already zero; return tails are still processed below.
            if !occupied_tracks[track_id.index()] {
                continue;
            }
            let track_parameters = parameters.mixer_track(track_id);
            let gain = db_to_linear(track_parameters.level_db());
            let (left_pan, right_pan) = pan_gains(track_parameters.pan());
            let left_gain = gain * left_pan;
            let right_gain = gain * right_pan;
            let audible = !track_parameters.mute() && (!any_solo || track_parameters.solo());
            let sends = track_parameters.sends();
            let track = &mut self.track_scratch[track_id.index()];
            let mut left_peak = 0.0_f32;
            let mut right_peak = 0.0_f32;
            let mut energy = 0.0_f64;

            for frame in 0..frame_count {
                let left = frame * 2;
                let right = left + 1;
                let raw_left = track[left] * left_gain;
                let raw_right = track[right] * right_gain;
                non_finite_samples = non_finite_samples
                    .saturating_add(u64::from(!raw_left.is_finite()))
                    .saturating_add(u64::from(!raw_right.is_finite()));
                let mixed_left = finite_or_zero(raw_left);
                let mixed_right = finite_or_zero(raw_right);
                track[left] = mixed_left;
                track[right] = mixed_right;
                left_peak = left_peak.max(mixed_left.abs());
                right_peak = right_peak.max(mixed_right.abs());
                energy += f64::from(mixed_left) * f64::from(mixed_left)
                    + f64::from(mixed_right) * f64::from(mixed_right);

                if audible {
                    output[left] += mixed_left;
                    output[right] += mixed_right;
                }
            }
            track_meters[track_id.index()] =
                TrackMeter::new(left_peak, right_peak, rms(energy, sample_count))
                    .unwrap_or_default();
            if audible {
                // Iterate each nonzero send once per block. Sample and track
                // accumulation order stay identical to the interleaved loop.
                for (bus_input, send) in self.bus_inputs.iter_mut().zip(sends) {
                    if *send != 0.0 {
                        for (destination, sample) in bus_input[..sample_count]
                            .iter_mut()
                            .zip(&track[..sample_count])
                        {
                            *destination += sample * send;
                        }
                    }
                }
            }
        }

        self.dry_output[..sample_count].copy_from_slice(&output[..sample_count]);

        // Bus returns: each occupied return derives its wet signal only from
        // its own accumulated send input. Returns accumulate into one zeroed
        // wet buffer that is summed into the mix exactly once before master
        // gain — the same `output + (wet_a + wet_b)` association the retired
        // port used, so occupying returns 0 and 1 with the registry reverb
        // and delay reproduces today's arithmetic bit for bit. Unoccupied
        // returns contribute silence, never their input. Live scalar values
        // and the live return level come from the snapshot's indexed return
        // entries — the latest-value transport, exactly as every other scalar.
        let mut bus_output_rms = [0.0_f32; MAX_BUS_RETURNS];
        self.wet_scratch[..sample_count].fill(0.0);
        for (index, live) in parameters
            .returns()
            .iter()
            .enumerate()
            .take(self.bus_inputs.len())
        {
            let Ok(bus) = BusId::new(index as u16) else {
                continue;
            };
            let measured = self.returns.process_return(
                bus,
                &self.bus_inputs[bus.index()][..sample_count],
                &mut self.wet_scratch[..sample_count],
                live,
            );
            if let Some(meter) = bus_output_rms.get_mut(index) {
                *meter = measured;
            }
        }
        if self.returns.occupied_count() > 0 {
            for (output_sample, wet_sample) in output[..sample_count]
                .iter_mut()
                .zip(&self.wet_scratch[..sample_count])
            {
                *output_sample += wet_sample;
            }
        }

        let master_gain = db_to_linear(parameters.global().master_gain_db());
        for sample in &mut output[..sample_count] {
            *sample *= master_gain;
        }

        observe_mix(
            track_meters,
            non_finite_samples,
            &self.bus_inputs,
            sample_count,
            bus_output_rms,
            &self.dry_output[..sample_count],
            &output[..sample_count],
            master_gain,
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn observe_mix(
    tracks: [TrackMeter; MixerTrackId::COUNT],
    prior_non_finite_samples: u64,
    bus_inputs: &[Vec<f32>],
    sample_count: usize,
    bus_output_rms: [f32; MAX_BUS_RETURNS],
    dry_output: &[f32],
    output: &[f32],
    master_gain: f32,
) -> MixObservation {
    let mut left_peak = 0.0_f32;
    let mut right_peak = 0.0_f32;
    let mut output_energy = 0.0_f64;
    let mut wet_energy = 0.0_f64;
    let mut non_finite_samples = prior_non_finite_samples;
    let mut clipped_samples = 0_u64;

    for (index, (output_sample, dry_sample)) in output.iter().zip(dry_output).enumerate() {
        let finite_output = if output_sample.is_finite() {
            *output_sample
        } else {
            non_finite_samples = non_finite_samples.saturating_add(1);
            0.0
        };
        if finite_output.abs() > 1.0 {
            clipped_samples = clipped_samples.saturating_add(1);
        }
        if index % 2 == 0 {
            left_peak = left_peak.max(finite_output.abs());
        } else {
            right_peak = right_peak.max(finite_output.abs());
        }

        let wet_before_master = if master_gain.is_finite() && master_gain > 0.0 {
            finite_output / master_gain - finite_or_zero(*dry_sample)
        } else {
            0.0
        };

        output_energy += f64::from(finite_output) * f64::from(finite_output);
        wet_energy += f64::from(wet_before_master) * f64::from(wet_before_master);
    }

    let mut bus_input_rms = [0.0_f32; MAX_BUS_RETURNS];
    for (bus_rms, bus_input) in bus_input_rms.iter_mut().zip(bus_inputs) {
        let mut bus_energy = 0.0_f64;
        for sample in &bus_input[..sample_count] {
            let finite_sample = finite_or_zero(*sample);
            bus_energy += f64::from(finite_sample) * f64::from(finite_sample);
        }
        *bus_rms = rms(bus_energy, sample_count);
    }

    MixObservation::measured(
        tracks,
        left_peak,
        right_peak,
        rms(output_energy, output.len()),
        bus_input_rms,
        bus_output_rms,
        rms(wet_energy, output.len()),
        non_finite_samples,
        clipped_samples,
    )
}

fn finite_or_zero(value: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        0.0
    }
}

fn rms(energy: f64, sample_count: usize) -> f32 {
    if sample_count == 0 {
        0.0
    } else {
        (energy / sample_count as f64).sqrt() as f32
    }
}

fn db_to_linear(decibels: f32) -> f32 {
    10.0_f32.powf(decibels / 20.0)
}

fn pan_gains(pan: f32) -> (f32, f32) {
    if pan < 0.0 {
        (1.0, 1.0 + pan)
    } else {
        (1.0 - pan, 1.0)
    }
}

fn allocate_zeros(length: usize) -> Result<Vec<f32>, EffectError> {
    let mut storage = Vec::new();
    storage
        .try_reserve_exact(length)
        .map_err(|_| EffectError::StorageAllocationFailed)?;
    storage.resize(length, 0.0);
    Ok(storage)
}

#[cfg(test)]
mod tests {
    use super::MixEngine;
    use crate::kernel::patch_id::PatchId;
    use crate::mixer::bus_id::BusId;
    use crate::mixer::bus_id::MAX_BUS_RETURNS;
    use crate::mixer::global_parameters::GlobalParameters;
    use crate::mixer::mixer_state::MixerState;
    use crate::mixer::mixer_track_id::MixerTrackId;
    use crate::mixer::mixer_track_parameters::MixerTrackParameters;
    use crate::mixer::patch_output::PatchOutput;
    use crate::real_time::parameter_snapshot::{
        ParameterSnapshot, RtBusReturnParameters, RtPatchParameters, RtPostEffectParameters,
    };
    use crate::real_time::patch_audio_block::PatchAudioBlock;
    use crate::synth::{EffectSlotId, PreparedEffectError, PreparedPostEffect};

    /// A prepared "effect" that returns its input unchanged (100% wet copy),
    /// making return arithmetic sample-exact in proofs.
    struct UnityReturn;

    impl PreparedPostEffect for UnityReturn {
        fn patch_id(&self) -> PatchId {
            PatchId::new(u32::MAX).unwrap()
        }

        fn slot_id(&self) -> EffectSlotId {
            EffectSlotId::new(1).unwrap()
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

    fn unity_parameters() -> RtPostEffectParameters {
        RtPostEffectParameters::new(EffectSlotId::new(1).unwrap(), &[]).unwrap()
    }

    /// Overlays live snapshot entries attesting installed unity returns at
    /// the given buses with per-bus live return levels.
    fn with_leveled_returns(
        snapshot: ParameterSnapshot,
        buses_and_levels: &[(BusId, f32)],
    ) -> ParameterSnapshot {
        let mut returns = [const { RtBusReturnParameters::EMPTY }; MAX_BUS_RETURNS];
        for (bus, level) in buses_and_levels {
            returns[bus.index()] =
                RtBusReturnParameters::new(EffectSlotId::new(1).unwrap(), &[], *level).unwrap();
        }
        snapshot.with_returns(returns)
    }

    /// Overlays live snapshot entries attesting installed unity returns at
    /// the given buses (level 1.0).
    fn with_unity_returns(snapshot: ParameterSnapshot, buses: &[BusId]) -> ParameterSnapshot {
        let pairs = buses.iter().map(|bus| (*bus, 1.0)).collect::<Vec<_>>();
        with_leveled_returns(snapshot, &pairs)
    }

    /// Installs sample-exact unity returns at the given buses at prepare time.
    fn install_unity_returns(mixer: &mut MixEngine, buses: &[BusId]) {
        for bus in buses {
            mixer
                .install_bus_return(*bus, Box::new(UnityReturn), unity_parameters(), 1.0)
                .unwrap();
        }
    }

    fn track(level_db: f32, pan: f32, bus0_send: f32, bus1_send: f32) -> MixerTrackParameters {
        let mut sends = [0.0; MAX_BUS_RETURNS];
        sends[0] = bus0_send;
        sends[1] = bus1_send;
        MixerTrackParameters::from_values(level_db, pan, false, false, sends)
            .expect("test track values satisfy their bounds")
    }

    fn globals(master_gain_db: f32) -> GlobalParameters {
        GlobalParameters::new(master_gain_db).expect("test global values satisfy their bounds")
    }

    fn snapshot(
        ids_and_tracks: &[(u32, MixerTrackParameters)],
        global: GlobalParameters,
    ) -> ParameterSnapshot {
        let mut patches = std::array::from_fn::<_, 16, _>(|_| {
            RtPatchParameters::new(PatchId::new(1).unwrap(), PatchOutput::default())
        });
        let mut mixer = MixerState::default();
        for (index, (slot, (id, track))) in patches.iter_mut().zip(ids_and_tracks).enumerate() {
            let track_id = MixerTrackId::new(index as u8).unwrap();
            *slot =
                RtPatchParameters::new(PatchId::new(*id).unwrap(), PatchOutput::to_track(track_id));
            mixer.set_track(track_id, track.clone());
        }
        ParameterSnapshot::new(1, global, mixer, &patches[..ids_and_tracks.len()]).unwrap()
    }

    fn audio_block(parameters: &ParameterSnapshot, stems: &[&[f32]]) -> PatchAudioBlock {
        let frame_count = stems.first().map_or(0, |stem| stem.len() / 2);
        let mut block = PatchAudioBlock::prepare(frame_count.max(1)).unwrap();
        block.begin_render(parameters, frame_count).unwrap();

        for (index, (patch, samples)) in parameters.patches().iter().zip(stems).enumerate() {
            block
                .stem_mut(index, patch.patch_id().unwrap())
                .unwrap()
                .copy_from_slice(samples);
        }

        block
    }

    #[test]
    fn preparation_rejects_unrepresentable_return_counts_before_scratch_allocation() {
        let maximum_count = usize::from(BusId::MAX) + 1;
        let mut mixer = MixEngine::new();
        for count in [maximum_count + 1, usize::MAX] {
            assert_eq!(
                mixer.prepare_with_returns(48_000.0, 1, count),
                Err(crate::mixer::bus_return::EffectError::StorageAllocationFailed)
            );
            assert!(mixer.bus_inputs.is_empty());
            assert!(!mixer.prepared);
        }
    }

    #[test]
    fn highest_representable_return_renders_without_wrapping_to_bus_zero() {
        let return_count = usize::from(BusId::MAX) + 1;
        let bus = BusId::new(BusId::MAX).unwrap();
        let mut mixer = MixEngine::new();
        mixer
            .prepare_with_returns(48_000.0, 1, return_count)
            .unwrap();
        install_unity_returns(&mut mixer, &[bus]);
        let sends_track = track(0.0, 0.0, 0.0, 0.0).with_send(bus, 1.0).unwrap();
        let mut returns = vec![RtBusReturnParameters::EMPTY; return_count];
        returns[bus.index()] =
            RtBusReturnParameters::new(EffectSlotId::new(1).unwrap(), &[], 1.0).unwrap();
        let parameters = snapshot(&[(7, sends_track)], globals(0.0)).with_returns(returns);
        let patch = [1.0, 1.0];
        let block = audio_block(&parameters, &[&patch]);
        let mut output = [0.0; 2];

        mixer.mix(&block, &parameters, &mut output);

        assert_eq!(mixer.bus_inputs.len(), return_count);
        assert_eq!(mixer.bus_inputs[bus.index()], [1.0, 1.0]);
        assert_eq!(mixer.bus_inputs[0], [0.0, 0.0]);
        assert_eq!(output, [2.0, 2.0]);
    }

    #[test]
    fn global_mix_applies_gain_and_pan_independently() {
        let mut mixer = MixEngine::new();
        mixer.prepare(48_000.0, 1).expect("mixer prepares");
        let parameters = snapshot(
            &[
                (11, track(-6.020_600_3, -1.0, 0.0, 0.0)),
                (22, track(-6.020_600_3, 1.0, 0.0, 0.0)),
            ],
            globals(0.0),
        );
        let left_patch = [1.0, 1.0];
        let right_patch = [1.0, 1.0];
        let block = audio_block(&parameters, &[&left_patch, &right_patch]);
        let mut output = [0.0; 2];

        mixer.mix(&block, &parameters, &mut output);

        assert!((output[0] - 0.5).abs() < 0.000_001);
        assert!((output[1] - 0.5).abs() < 0.000_001);
    }

    #[test]
    fn global_mix_routes_both_sends_through_both_returns() {
        let buses = [BusId::new(0).unwrap(), BusId::new(1).unwrap()];
        let mut mixer = MixEngine::new();
        mixer.prepare(48_000.0, 1).expect("mixer prepares");
        install_unity_returns(&mut mixer, &buses);
        let parameters = with_leveled_returns(
            snapshot(&[(7, track(0.0, 0.0, 0.25, 0.5))], globals(0.0)),
            &[(buses[0], 0.4), (buses[1], 0.6)],
        );
        let patch = [1.0, 1.0];
        let block = audio_block(&parameters, &[&patch]);
        let mut output = [0.0; 2];

        mixer.mix(&block, &parameters, &mut output);

        assert!((output[0] - 1.4).abs() < 0.000_001);
        assert!((output[1] - 1.4).abs() < 0.000_001);
    }

    #[test]
    fn global_mix_applies_master_gain_after_effect_returns() {
        let bus = BusId::new(0).unwrap();
        let mut mixer = MixEngine::new();
        mixer.prepare(48_000.0, 1).expect("mixer prepares");
        install_unity_returns(&mut mixer, &[bus]);
        let parameters = with_unity_returns(
            snapshot(&[(3, track(0.0, 0.0, 1.0, 0.0))], globals(-6.020_600_3)),
            &[bus],
        );
        let patch = [1.0, 1.0];
        let block = audio_block(&parameters, &[&patch]);
        let mut output = [0.0; 2];

        mixer.mix(&block, &parameters, &mut output);

        assert!((output[0] - 1.0).abs() < 0.000_001);
        assert!((output[1] - 1.0).abs() < 0.000_001);
    }

    #[test]
    fn global_mix_silences_mismatched_patch_identity() {
        let mut mixer = MixEngine::new();
        mixer.prepare(48_000.0, 1).expect("mixer prepares");
        let rendered_parameters = snapshot(
            &[
                (1, track(0.0, 0.0, 0.0, 0.0)),
                (2, track(0.0, 0.0, 0.0, 0.0)),
            ],
            globals(0.0),
        );
        let current_parameters = snapshot(
            &[
                (1, track(0.0, 0.0, 0.0, 0.0)),
                (3, track(0.0, 0.0, 0.0, 0.0)),
            ],
            globals(0.0),
        );
        let first = [1.0, 0.0];
        let second = [0.0, 1.0];
        let block = audio_block(&rendered_parameters, &[&first, &second]);
        let mut output = [1.0; 2];

        mixer.mix(&block, &current_parameters, &mut output);

        assert_eq!(output, [0.0; 2]);
    }

    #[test]
    fn global_mix_uses_only_preallocated_bounded_storage() {
        let buses = [BusId::new(0).unwrap(), BusId::new(1).unwrap()];
        let mut mixer = MixEngine::new();
        mixer.prepare(48_000.0, 2).expect("mixer prepares");
        install_unity_returns(&mut mixer, &buses);
        let bus_capacities: Vec<usize> = mixer
            .bus_inputs
            .iter()
            .map(|bus_input| bus_input.capacity())
            .collect();
        let dry_capacity = mixer.dry_output.capacity();
        let parameters = with_unity_returns(
            snapshot(&[(9, track(0.0, 0.0, 0.5, 0.5))], globals(0.0)),
            &buses,
        );
        let patch = [1.0; 4];
        let block = audio_block(&parameters, &[&patch]);
        let mut output = [1.0; 8];

        mixer.mix(&block, &parameters, &mut output);

        for (bus_input, capacity) in mixer.bus_inputs.iter().zip(bus_capacities) {
            assert_eq!(bus_input.capacity(), capacity);
        }
        assert_eq!(mixer.dry_output.capacity(), dry_capacity);
        assert_eq!(output[4..], [0.0; 4]);
    }

    #[test]
    fn mix_observation_measures_owned_stages_without_changing_output() {
        let buses = [BusId::new(0).unwrap(), BusId::new(1).unwrap()];
        let mut mixer = MixEngine::new();
        mixer.prepare(48_000.0, 1).expect("mixer prepares");
        install_unity_returns(&mut mixer, &buses);
        let parameters = with_leveled_returns(
            snapshot(&[(7, track(0.0, 0.0, 0.5, 0.25))], globals(0.0)),
            &[(buses[0], 0.4), (buses[1], 0.8)],
        );
        let patch = [1.0, 1.0];
        let block = audio_block(&parameters, &[&patch]);
        let mut output = [0.0; 2];

        let observation = mixer.mix(&block, &parameters, &mut output);

        assert!(output
            .iter()
            .all(|sample| (*sample - 1.4).abs() < 0.000_001));
        assert!((observation.left_peak() - 1.4).abs() < 0.000_001);
        assert!((observation.right_peak() - 1.4).abs() < 0.000_001);
        assert!((observation.output_rms() - 1.4).abs() < 0.000_001);
        assert!((observation.reverb_input_rms() - 0.5).abs() < 0.000_001);
        assert!((observation.delay_input_rms() - 0.25).abs() < 0.000_001);
        assert!((observation.wet_output_rms() - 0.4).abs() < 0.000_001);
        assert_eq!(observation.non_finite_samples(), 0);
        assert_eq!(observation.clipped_samples(), 2);
    }

    #[test]
    fn mix_observation_counts_non_finite_output_without_non_finite_metrics() {
        let mut mixer = MixEngine::new();
        mixer.prepare(48_000.0, 1).expect("mixer prepares");
        let parameters = snapshot(&[(7, track(0.0, 0.0, 0.0, 0.0))], globals(0.0));
        let patch = [f32::NAN, f32::INFINITY];
        let block = audio_block(&parameters, &[&patch]);
        let mut output = [0.0; 2];

        let observation = mixer.mix(&block, &parameters, &mut output);

        assert_eq!(observation.non_finite_samples(), 2);
        assert_eq!(observation.left_peak(), 0.0);
        assert_eq!(observation.right_peak(), 0.0);
        assert_eq!(observation.output_rms(), 0.0);
        assert!(observation.wet_output_rms().is_finite());
    }

    // ---- T025: the seven routing proofs -----------------------------------

    /// C-BR-1: the send is taken after the fader, sample-exactly.
    ///
    /// A 0.5x fader with a 0.8 send toward bus 3 must place exactly
    /// 0.5 * 0.8 = 0.4 on the bus, and a unity return at level 1.0 must make
    /// the final output exactly dry + wet = 0.5 + 0.4.
    #[test]
    fn send_position_is_post_fader_sample_exact() {
        let bus = BusId::new(3).unwrap();
        let mut mixer = MixEngine::new();
        mixer.prepare(48_000.0, 1).expect("mixer prepares");
        mixer
            .install_bus_return(bus, Box::new(UnityReturn), unity_parameters(), 1.0)
            .unwrap();
        let sends_track = track(-6.020_600_3, 0.0, 0.0, 0.0)
            .with_send(bus, 0.8)
            .unwrap();
        let parameters = with_unity_returns(snapshot(&[(7, sends_track)], globals(0.0)), &[bus]);
        let patch = [1.0, 1.0];
        let block = audio_block(&parameters, &[&patch]);
        let mut output = [0.0; 2];

        let observation = mixer.mix(&block, &parameters, &mut output);

        assert!(
            (observation
                .bus_input_rms(bus)
                .expect("bus belongs to observed default bank")
                - 0.4)
                .abs()
                < 0.000_001
        );
        assert!((output[0] - 0.9).abs() < 0.000_001);
        assert!((output[1] - 0.9).abs() < 0.000_001);
    }

    /// C-BR-2: mute always wins — a muted track contributes no dry signal and
    /// no send, while its meter stays pre-gate and diagnosable.
    #[test]
    fn muted_track_contributes_nothing_to_any_send() {
        let bus = BusId::new(6).unwrap();
        let mut mixer = MixEngine::new();
        mixer.prepare(48_000.0, 1).expect("mixer prepares");
        mixer
            .install_bus_return(bus, Box::new(UnityReturn), unity_parameters(), 1.0)
            .unwrap();
        let muted = MixerTrackParameters::from_values(
            0.0,
            0.0,
            true,
            false,
            [1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        )
        .unwrap()
        .with_send(bus, 1.0)
        .unwrap();
        let parameters = with_unity_returns(snapshot(&[(7, muted)], globals(0.0)), &[bus]);
        let patch = [1.0, 1.0];
        let block = audio_block(&parameters, &[&patch]);
        let mut output = [0.0; 2];

        let observation = mixer.mix(&block, &parameters, &mut output);

        assert_eq!(output, [0.0; 2]);
        for bus in BusId::ALL {
            assert_eq!(
                observation
                    .bus_input_rms(bus)
                    .expect("bus belongs to observed default bank"),
                0.0
            );
            assert_eq!(
                observation
                    .bus_output_rms(bus)
                    .expect("bus belongs to observed default bank"),
                0.0
            );
        }
        // Meters stay pre-gate: the muted track remains diagnosable.
        assert!(observation.track(MixerTrackId::default()).left_peak() > 0.0);
    }

    /// C-BR-2: when any track is soloed, non-soloed tracks contribute neither
    /// dry signal nor sends.
    #[test]
    fn solo_excluded_track_contributes_nothing_to_any_send() {
        let bus = BusId::new(2).unwrap();
        let mut mixer = MixEngine::new();
        mixer.prepare(48_000.0, 1).expect("mixer prepares");
        mixer
            .install_bus_return(bus, Box::new(UnityReturn), unity_parameters(), 1.0)
            .unwrap();
        let soloed =
            MixerTrackParameters::from_values(0.0, 0.0, false, true, [0.0; MAX_BUS_RETURNS])
                .unwrap();
        let excluded = track(0.0, 0.0, 0.0, 0.0).with_send(bus, 1.0).unwrap();
        let parameters = with_unity_returns(
            snapshot(&[(11, soloed), (22, excluded)], globals(0.0)),
            &[bus],
        );
        let soloed_patch = [0.5, 0.5];
        let excluded_patch = [1.0, 1.0];
        let block = audio_block(&parameters, &[&soloed_patch, &excluded_patch]);
        let mut output = [0.0; 2];

        let observation = mixer.mix(&block, &parameters, &mut output);

        // Only the soloed track's dry signal remains; the excluded track's
        // send never reached the bus.
        assert!((output[0] - 0.5).abs() < 0.000_001);
        assert!((output[1] - 0.5).abs() < 0.000_001);
        assert_eq!(
            observation
                .bus_input_rms(bus)
                .expect("bus belongs to observed default bank"),
            0.0
        );
        assert_eq!(
            observation
                .bus_output_rms(bus)
                .expect("bus belongs to observed default bank"),
            0.0
        );
    }

    /// C-BR-3 / C-BR-5 / NFR-007: raising one send toward one bus leaves the
    /// other seven destinations below -60 dBFS from that source.
    #[test]
    fn raised_send_is_isolated_to_its_own_destination() {
        let bus = BusId::new(2).unwrap();
        let mut mixer = MixEngine::new();
        mixer.prepare(48_000.0, 1).expect("mixer prepares");
        for destination in BusId::ALL {
            mixer
                .install_bus_return(destination, Box::new(UnityReturn), unity_parameters(), 1.0)
                .unwrap();
        }
        let sends_track = track(0.0, 0.0, 0.0, 0.0).with_send(bus, 0.8).unwrap();
        let parameters =
            with_unity_returns(snapshot(&[(7, sends_track)], globals(0.0)), &BusId::ALL);
        let patch = [1.0, 1.0];
        let block = audio_block(&parameters, &[&patch]);
        let mut output = [0.0; 2];

        let observation = mixer.mix(&block, &parameters, &mut output);

        let minus_sixty_dbfs = 0.8 * 0.001;
        assert!(
            (observation
                .bus_input_rms(bus)
                .expect("bus belongs to observed default bank")
                - 0.8)
                .abs()
                < 0.000_001
        );
        for other in BusId::ALL {
            if other != bus {
                assert!(
                    observation
                        .bus_input_rms(other)
                        .expect("bus belongs to observed default bank")
                        < minus_sixty_dbfs
                );
                assert!(
                    observation
                        .bus_output_rms(other)
                        .expect("bus belongs to observed default bank")
                        < minus_sixty_dbfs
                );
                assert_eq!(
                    observation
                        .bus_input_rms(other)
                        .expect("bus belongs to observed default bank"),
                    0.0
                );
            }
        }
    }

    /// C-BR-4: two tracks sending to one bus sum there, and each send scales
    /// only its own contribution.
    #[test]
    fn sends_from_two_tracks_accumulate_at_one_bus() {
        let bus = BusId::new(1).unwrap();
        let mut mixer = MixEngine::new();
        mixer.prepare(48_000.0, 1).expect("mixer prepares");
        mixer
            .install_bus_return(bus, Box::new(UnityReturn), unity_parameters(), 1.0)
            .unwrap();
        let first = track(0.0, 0.0, 0.0, 0.0).with_send(bus, 0.5).unwrap();
        let second = track(0.0, 0.0, 0.0, 0.0).with_send(bus, 0.25).unwrap();
        let parameters =
            with_unity_returns(snapshot(&[(11, first), (22, second)], globals(0.0)), &[bus]);
        let first_patch = [1.0, 1.0];
        let second_patch = [1.0, 1.0];
        let block = audio_block(&parameters, &[&first_patch, &second_patch]);
        let mut output = [0.0; 2];

        let observation = mixer.mix(&block, &parameters, &mut output);

        // Dry 2.0 plus the summed sends 0.5 + 0.25 through a unity return.
        assert!(
            (observation
                .bus_input_rms(bus)
                .expect("bus belongs to observed default bank")
                - 0.75)
                .abs()
                < 0.000_001
        );
        assert!((output[0] - 2.75).abs() < 0.000_001);
        assert!((output[1] - 2.75).abs() < 0.000_001);
    }

    /// C-BR-6: an unoccupied return contributes silence — not its input.
    #[test]
    fn unoccupied_return_contributes_silence_not_its_accumulated_input() {
        let bus = BusId::new(4).unwrap();
        let mut mixer = MixEngine::new();
        mixer.prepare(48_000.0, 1).expect("mixer prepares");
        let sends_track = track(0.0, 0.0, 0.0, 0.0).with_send(bus, 1.0).unwrap();
        let parameters = snapshot(&[(7, sends_track)], globals(0.0));
        let patch = [1.0, 1.0];
        let block = audio_block(&parameters, &[&patch]);
        let mut output = [0.0; 2];

        let observation = mixer.mix(&block, &parameters, &mut output);

        // The send accumulated at the bus, but the empty return added nothing:
        // the output is exactly the dry signal.
        assert!(
            (observation
                .bus_input_rms(bus)
                .expect("bus belongs to observed default bank")
                - 1.0)
                .abs()
                < 0.000_001
        );
        assert_eq!(
            observation
                .bus_output_rms(bus)
                .expect("bus belongs to observed default bank"),
            0.0
        );
        assert!((output[0] - 1.0).abs() < 0.000_001);
        assert!((output[1] - 1.0).abs() < 0.000_001);
    }

    /// C-BR-9: a return sums into the dry mix and cannot feed another return;
    /// no return-to-send edge exists.
    #[test]
    fn return_output_cannot_feed_another_return() {
        let fed = BusId::new(0).unwrap();
        let idle = BusId::new(1).unwrap();
        let mut mixer = MixEngine::new();
        mixer.prepare(48_000.0, 1).expect("mixer prepares");
        for destination in [fed, idle] {
            mixer
                .install_bus_return(destination, Box::new(UnityReturn), unity_parameters(), 1.0)
                .unwrap();
        }
        let sends_track = track(0.0, 0.0, 0.0, 0.0).with_send(fed, 1.0).unwrap();
        let parameters =
            with_unity_returns(snapshot(&[(7, sends_track)], globals(0.0)), &[fed, idle]);
        let patch = [1.0, 1.0];
        let block = audio_block(&parameters, &[&patch]);
        let mut output = [0.0; 2];

        let observation = mixer.mix(&block, &parameters, &mut output);

        // The fed return produced wet signal, and that wet signal excited no
        // other return: output is exactly dry + one return's wet.
        assert!(
            observation
                .bus_output_rms(fed)
                .expect("bus belongs to observed default bank")
                > 0.0
        );
        assert_eq!(
            observation
                .bus_input_rms(idle)
                .expect("bus belongs to observed default bank"),
            0.0
        );
        assert_eq!(
            observation
                .bus_output_rms(idle)
                .expect("bus belongs to observed default bank"),
            0.0
        );
        assert!((output[0] - 2.0).abs() < 0.000_001);
        assert!((output[1] - 2.0).abs() < 0.000_001);
    }
}
