use crate::mixer::bus_id::{BusId, MAX_BUS_RETURNS};
use crate::mixer::bus_return::{EffectError, RETURN_LEVEL_DESCRIPTOR};
use crate::real_time::prepared_graph::PositionCapabilityIdentity;
use crate::real_time::{ParameterSnapshot, RtBusReturnParameters, RtPostEffectParameters};
use crate::synth::{EffectSlotId, PreparedPostEffect};
use core::fmt;

/// One prepared bus return: a registry-prepared effect instance, its
/// install-time parameter block (the structural attestation: instance
/// identity and scalar layout), the occupant's recorded effect capability
/// identity, the install-time return level, and preallocated input scratch.
/// Live scalar values and the live return level arrive per block from the
/// `ParameterSnapshot`.
struct PreparedReturnEffect {
    effect: Box<dyn PreparedPostEffect>,
    parameters: RtPostEffectParameters,
    /// The effect capability identity this occupant was prepared from,
    /// recorded at build time from the validated candidate configuration.
    /// Fixed-size and copyable; compared only at carry-over decision time.
    capability_identity: Option<PositionCapabilityIdentity>,
}

struct PreparedBusReturn {
    effects: Vec<PreparedReturnEffect>,
    return_level: f32,
    input_scratch: Vec<f32>,
}

/// Bounded bank of eight prepared bus returns, the peer of
/// `PreparedPostEffectRack` on the shared-return side of the mix.
///
/// This rack replaces the retired `GlobalEffectsProcessor` port, whose
/// signature hardcoded two named returns. It carries that port's two input
/// obligations forward verbatim:
///
/// - Wet excitation derives exclusively from the declared return input passed
///   to [`Self::process_return`]. Samples already accumulated in the output
///   are never treated as an implicit send: the input is copied into the
///   return's private scratch and the output buffer is never handed to the
///   prepared effect (C-BR-7).
/// - Zero effect input cannot create a wet return (C-BR-8).
///
/// Topology: a return sums into the caller's dry mix and cannot feed another
/// return. No API exposes a return's wet output as an input buffer, so no
/// return-to-send edge — and therefore no routing cycle — is expressible
/// (C-BR-9). An unoccupied return contributes silence; it never passes its
/// accumulated input through (C-BR-6).
///
/// `install` and `clear` allocate and destroy and are prepare-time operations;
/// `process_return` runs on the audio thread and is allocation-free,
/// lock-free, non-blocking, and free of I/O, logging, panic, and destruction.
pub struct PreparedBusReturnRack {
    returns: Vec<Option<PreparedBusReturn>>,
    max_frames: usize,
}

impl PreparedBusReturnRack {
    /// Creates the unprepared empty rack; every install is rejected until the
    /// rack is recreated with a validated frame capacity.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            returns: (0..MAX_BUS_RETURNS).map(|_| None).collect(),
            max_frames: 0,
        }
    }

    /// Creates an empty rack bounded to `max_frames` per block.
    pub fn new(max_frames: usize) -> Result<Self, EffectError> {
        Self::with_count(max_frames, MAX_BUS_RETURNS)
    }

    pub fn with_count(max_frames: usize, count: usize) -> Result<Self, EffectError> {
        if max_frames == 0 {
            return Err(EffectError::InvalidMaxFrames);
        }
        // Every prepared position must have a distinct representable BusId.
        // Validate before allocation so later positional casts cannot wrap.
        if count > usize::from(BusId::MAX) + 1 {
            return Err(EffectError::StorageAllocationFailed);
        }
        let mut returns = Vec::new();
        returns
            .try_reserve_exact(count)
            .map_err(|_| EffectError::StorageAllocationFailed)?;
        returns.resize_with(count, || None);
        Ok(Self {
            returns,
            max_frames,
        })
    }

    pub fn len(&self) -> usize {
        self.returns.len()
    }

    pub fn is_empty(&self) -> bool {
        self.returns.is_empty()
    }

    pub fn chain_layout(
        &self,
        bus: BusId,
    ) -> Vec<(
        Option<EffectSlotId>,
        usize,
        Option<PositionCapabilityIdentity>,
    )> {
        self.returns
            .get(bus.index())
            .and_then(Option::as_ref)
            .map(|entry| {
                entry
                    .effects
                    .iter()
                    .map(|effect| {
                        (
                            effect.parameters.slot_id(),
                            effect.parameters.scalar_count(),
                            effect.capability_identity,
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub const fn max_frames(&self) -> usize {
        self.max_frames
    }

    /// Occupies one return with a prepared effect, replacing any prior
    /// occupant. Prepare-time only: allocates scratch and may drop the
    /// replaced effect.
    pub fn install(
        &mut self,
        bus: BusId,
        effect: Box<dyn PreparedPostEffect>,
        parameters: RtPostEffectParameters,
        return_level: f32,
    ) -> Result<(), EffectError> {
        if self.max_frames == 0 {
            return Err(EffectError::InvalidMaxFrames);
        }
        if !RETURN_LEVEL_DESCRIPTOR.contains(return_level) {
            return Err(EffectError::InvalidReturnLevel);
        }
        let sample_capacity = self
            .max_frames
            .checked_mul(2)
            .ok_or(EffectError::StorageAllocationFailed)?;
        let mut input_scratch = Vec::new();
        input_scratch
            .try_reserve_exact(sample_capacity)
            .map_err(|_| EffectError::StorageAllocationFailed)?;
        input_scratch.resize(sample_capacity, 0.0);
        let target = self
            .returns
            .get_mut(bus.index())
            .ok_or(EffectError::StorageAllocationFailed)?;
        *target = Some(PreparedBusReturn {
            effects: vec![PreparedReturnEffect {
                effect,
                parameters,
                capability_identity: None,
            }],
            return_level,
            input_scratch,
        });
        Ok(())
    }

    /// Appends a prepared processor in signal order. Preparation thread only.
    pub fn append(
        &mut self,
        bus: BusId,
        effect: Box<dyn PreparedPostEffect>,
        parameters: RtPostEffectParameters,
        return_level: f32,
    ) -> Result<(), EffectError> {
        if let Some(Some(prepared)) = self.returns.get_mut(bus.index()) {
            prepared.effects.push(PreparedReturnEffect {
                effect,
                parameters,
                capability_identity: None,
            });
            Ok(())
        } else {
            self.install(bus, effect, parameters, return_level)
        }
    }

    /// Records the validated effect capability identity prepared at one
    /// occupied return.
    ///
    /// Prepare-time only: the graph builder stamps every occupant from the
    /// validated candidate configuration right after installation. Returns
    /// whether the return is occupied and carries the exact instance
    /// identity; a disagreeing return is left untouched.
    pub(crate) fn record_occupant_identity(
        &mut self,
        bus: BusId,
        slot_id: EffectSlotId,
        identity: PositionCapabilityIdentity,
    ) -> bool {
        let Some(prepared) = self.returns[bus.index()].as_mut() else {
            return false;
        };
        let Some(effect) = prepared
            .effects
            .iter_mut()
            .find(|effect| effect.parameters.slot_id() == Some(slot_id))
        else {
            return false;
        };
        effect.capability_identity = Some(identity);
        true
    }

    /// Returns the recorded occupant capability identity at one return.
    pub fn capability_identity(&self, bus: BusId) -> Option<PositionCapabilityIdentity> {
        self.returns[bus.index()]
            .as_ref()
            .and_then(|prepared| prepared.effects.first())
            .and_then(|effect| effect.capability_identity)
    }

    /// Empties one return. Prepare-time only: drops the prepared effect.
    pub fn clear(&mut self, bus: BusId) {
        self.returns[bus.index()] = None;
    }

    pub fn occupied(&self, bus: BusId) -> bool {
        self.returns[bus.index()].is_some()
    }

    pub fn occupied_count(&self) -> usize {
        self.returns.iter().filter(|slot| slot.is_some()).count()
    }

    pub fn return_level(&self, bus: BusId) -> Option<f32> {
        self.returns[bus.index()]
            .as_ref()
            .map(|prepared| prepared.return_level)
    }

    /// Returns the instance identity prepared at one return.
    pub fn slot_id(&self, bus: BusId) -> Option<EffectSlotId> {
        self.returns[bus.index()]
            .as_ref()
            .and_then(|prepared| prepared.effects.first())
            .and_then(|effect| effect.parameters.slot_id())
    }

    /// Returns the scalar layout prepared at one return.
    pub fn scalar_count(&self, bus: BusId) -> Option<usize> {
        self.returns[bus.index()]
            .as_ref()
            .and_then(|prepared| prepared.effects.first())
            .map(|effect| effect.parameters.scalar_count())
    }

    /// Proves the snapshot's return bank and this prepared rack agree exactly.
    ///
    /// Per bus: an unoccupied return requires an inactive entry at that exact
    /// index; an occupied return requires an active entry whose `slot_id` and
    /// `scalar_count` both match the prepared instance — nothing weaker, and
    /// never repositioned.
    pub fn matches_parameters(&self, parameters: &ParameterSnapshot) -> bool {
        self.returns.len() == parameters.returns().len()
            && parameters.patches().iter().all(|patch| {
                patch
                    .sends()
                    .iter()
                    .skip(self.returns.len())
                    .all(|send| *send == 0.0)
            })
            && self
                .returns
                .iter()
                .zip(parameters.returns())
                .all(|(prepared, entry)| match prepared {
                    None => !entry.is_active(),
                    Some(prepared) => {
                        prepared.effects.len() == entry.effect_count()
                            && prepared
                                .effects
                                .iter()
                                .zip(entry.effects())
                                .all(|(effect, live)| {
                                    effect.parameters.slot_id() == live.slot_id()
                                        && effect.parameters.scalar_count() == live.scalar_count()
                                })
                    }
                })
    }

    /// Exchanges the still-live prepared effect instance from the superseded
    /// bank into this replacement at every return the structural delta leaves
    /// unchanged, so unchanged shared returns keep their tails across
    /// block-boundary activation.
    ///
    /// Callback-safe: bounded by `MAX_BUS_RETURNS` pointer-sized `mem::swap`s
    /// with no allocation, deallocation, locking, blocking, or destruction.
    /// Only the effect box is exchanged; each return keeps its own
    /// preallocated input scratch, which is overwritten before every use.
    ///
    /// `exclude` names the one return the delta changed; it keeps its fresh
    /// instance (the permitted local restart). Every exchange requires exact
    /// instance-identity, scalar-layout, and recorded occupant
    /// capability-identity agreement at the same return — a non-matching
    /// return keeps its fresh instance. The identity check is a fixed-size
    /// comparison made once at this carry-over decision, never per render
    /// block.
    pub(crate) fn carry_live_returns_from(
        &mut self,
        superseded: &mut Self,
        exclude: Option<BusId>,
    ) {
        for (index, (target, source)) in self
            .returns
            .iter_mut()
            .zip(&mut superseded.returns)
            .enumerate()
        {
            if exclude.is_some_and(|bus| bus.index() == index) {
                continue;
            }
            let (Some(target), Some(source)) = (target.as_mut(), source.as_mut()) else {
                continue;
            };
            if target.effects.len() != source.effects.len()
                || !target
                    .effects
                    .iter()
                    .zip(&source.effects)
                    .all(|(target, source)| {
                        target.parameters.slot_id() == source.parameters.slot_id()
                            && target.parameters.scalar_count() == source.parameters.scalar_count()
                            && target.capability_identity == source.capability_identity
                    })
            {
                continue;
            }
            for (target, source) in target.effects.iter_mut().zip(&mut source.effects) {
                core::mem::swap(&mut target.effect, &mut source.effect);
            }
        }
    }

    /// Adds one return's wet contribution to `output` and reports its RMS.
    ///
    /// Callback-safe. An unoccupied return contributes exact silence and
    /// touches neither buffer. A return whose effect rejects processing also
    /// contributes silence — it never falls back to passing its input
    /// through. Wet excitation derives exclusively from `input`; the
    /// accumulated `output` is never handed to the effect.
    ///
    /// Live scalar values and the return level come from `live`, the latest
    /// snapshot entry for this bus. A `live` entry that does not attest this
    /// exact prepared instance (inactive, wrong `slot_id`, or wrong
    /// `scalar_count`) contributes silence — wrong values are never
    /// substituted. The frozen install-time parameters remain the structural
    /// attestation checked by [`Self::matches_parameters`].
    pub fn process_return(
        &mut self,
        bus: BusId,
        input: &[f32],
        output: &mut [f32],
        live: &RtBusReturnParameters,
    ) -> f32 {
        let Some(prepared) = self.returns[bus.index()].as_mut() else {
            return 0.0;
        };
        if prepared.effects.len() != live.effect_count()
            || !prepared
                .effects
                .iter()
                .zip(live.effects())
                .all(|(effect, live)| {
                    effect.parameters.slot_id() == live.slot_id()
                        && effect.parameters.scalar_count() == live.scalar_count()
                })
        {
            return 0.0;
        }
        let frame_count = (input.len() / 2).min(output.len() / 2).min(self.max_frames);
        let sample_count = frame_count * 2;
        if sample_count == 0 || sample_count > prepared.input_scratch.len() {
            return 0.0;
        }
        prepared.input_scratch[..sample_count].copy_from_slice(&input[..sample_count]);
        for (effect, values) in prepared.effects.iter_mut().zip(live.effects()) {
            if effect
                .effect
                .process(
                    &mut prepared.input_scratch[..sample_count],
                    frame_count,
                    values,
                )
                .is_err()
            {
                return 0.0;
            }
        }
        let return_level = live.return_level();
        let mut energy = 0.0_f64;
        for (output_sample, wet_sample) in output[..sample_count]
            .iter_mut()
            .zip(&prepared.input_scratch[..sample_count])
        {
            let wet = wet_sample * return_level;
            *output_sample += wet;
            energy += f64::from(wet) * f64::from(wet);
        }
        (energy / sample_count as f64).sqrt() as f32
    }
}

impl Default for PreparedBusReturnRack {
    fn default() -> Self {
        Self::empty()
    }
}

impl fmt::Debug for PreparedBusReturnRack {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedBusReturnRack")
            .field("max_frames", &self.max_frames)
            .field("occupied_count", &self.occupied_count())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::PreparedBusReturnRack;
    use crate::kernel::PatchId;
    use crate::mixer::bus_id::{BusId, MAX_BUS_RETURNS};
    use crate::mixer::bus_return::EffectError;
    use crate::mixer::global_parameters::GlobalParameters;
    use crate::mixer::mixer_state::MixerState;
    use crate::real_time::prepared_graph::PositionCapabilityIdentity;
    use crate::real_time::{ParameterSnapshot, RtBusReturnParameters, RtPostEffectParameters};
    use crate::synth::{EffectCapabilityId, EffectSlotId, PreparedEffectError, PreparedPostEffect};

    struct UnityEffect;

    impl PreparedPostEffect for UnityEffect {
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

    struct FailingEffect;

    impl PreparedPostEffect for FailingEffect {
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
            Err(PreparedEffectError::ProcessRejected)
        }
    }

    fn parameters() -> RtPostEffectParameters {
        RtPostEffectParameters::new(EffectSlotId::new(1).unwrap(), &[]).unwrap()
    }

    /// A live snapshot entry attesting the fixture instance at `level`.
    fn live(level: f32) -> RtBusReturnParameters {
        RtBusReturnParameters::new(EffectSlotId::new(1).unwrap(), &[], level).unwrap()
    }

    #[test]
    fn preparation_rejects_invalid_configuration() {
        assert!(matches!(
            PreparedBusReturnRack::new(0),
            Err(EffectError::InvalidMaxFrames)
        ));
        let mut unprepared = PreparedBusReturnRack::empty();
        assert_eq!(
            unprepared
                .install(BusId::default(), Box::new(UnityEffect), parameters(), 0.5)
                .unwrap_err(),
            EffectError::InvalidMaxFrames
        );
        let mut rack = PreparedBusReturnRack::new(4).unwrap();
        assert_eq!(
            rack.install(BusId::default(), Box::new(UnityEffect), parameters(), 1.5)
                .unwrap_err(),
            EffectError::InvalidReturnLevel
        );
        assert_eq!(
            rack.install(
                BusId::default(),
                Box::new(UnityEffect),
                parameters(),
                f32::NAN
            )
            .unwrap_err(),
            EffectError::InvalidReturnLevel
        );
    }

    #[test]
    fn rack_prepares_default_returns_with_preallocated_scratch() {
        let mut rack = PreparedBusReturnRack::new(4).unwrap();
        for bus in BusId::ALL {
            rack.install(bus, Box::new(UnityEffect), parameters(), 0.5)
                .unwrap();
        }
        assert_eq!(rack.occupied_count(), MAX_BUS_RETURNS);
        for bus in BusId::ALL {
            assert!(rack.occupied(bus));
            assert_eq!(rack.return_level(bus), Some(0.5));
        }
    }

    #[test]
    fn return_count_accepts_full_identity_range_and_rejects_wrapping_counts() {
        let maximum_count = usize::from(BusId::MAX) + 1;
        let mut rack = PreparedBusReturnRack::with_count(1, maximum_count).unwrap();
        assert_eq!(rack.len(), maximum_count);
        let final_bus = BusId::new(BusId::MAX).unwrap();
        rack.install(final_bus, Box::new(UnityEffect), parameters(), 1.0)
            .unwrap();
        assert!(rack.occupied(final_bus));
        assert!(!rack.occupied(BusId::default()));
        let mut output = [0.0; 2];
        assert_eq!(
            rack.process_return(final_bus, &[0.5; 2], &mut output, &live(1.0)),
            0.5
        );
        assert_eq!(output, [0.5; 2]);

        for count in [maximum_count + 1, usize::MAX] {
            assert_eq!(
                PreparedBusReturnRack::with_count(1, count).unwrap_err(),
                EffectError::StorageAllocationFailed
            );
        }
    }

    #[test]
    fn unoccupied_return_contributes_exact_silence_not_its_input() {
        let mut rack = PreparedBusReturnRack::new(2).unwrap();
        let input = [1.0; 4];
        let mut output = [0.25; 4];

        let rms = rack.process_return(BusId::new(3).unwrap(), &input, &mut output, &live(1.0));

        assert_eq!(rms, 0.0);
        assert_eq!(output, [0.25; 4]);
    }

    #[test]
    fn occupied_return_scales_wet_by_the_return_owned_level() {
        let mut rack = PreparedBusReturnRack::new(2).unwrap();
        let bus = BusId::new(5).unwrap();
        rack.install(bus, Box::new(UnityEffect), parameters(), 0.5)
            .unwrap();
        let input = [1.0, -1.0, 0.5, -0.5];
        let mut output = [0.1; 4];

        let rms = rack.process_return(bus, &input, &mut output, &live(0.5));

        assert_eq!(output, [0.6, -0.4, 0.35, -0.15]);
        assert!(rms > 0.0);
    }

    #[test]
    fn zero_input_cannot_produce_a_wet_return() {
        let mut rack = PreparedBusReturnRack::new(2).unwrap();
        let bus = BusId::new(0).unwrap();
        rack.install(bus, Box::new(UnityEffect), parameters(), 1.0)
            .unwrap();
        // The output already carries accumulated dry signal; it must never act
        // as an implicit send into the return.
        let mut output = [0.75; 4];

        let rms = rack.process_return(bus, &[0.0; 4], &mut output, &live(1.0));

        assert_eq!(rms, 0.0);
        assert_eq!(output, [0.75; 4]);
    }

    #[test]
    fn one_return_cannot_feed_another_return() {
        let mut rack = PreparedBusReturnRack::new(2).unwrap();
        let first = BusId::new(0).unwrap();
        let second = BusId::new(1).unwrap();
        rack.install(first, Box::new(UnityEffect), parameters(), 1.0)
            .unwrap();
        rack.install(second, Box::new(UnityEffect), parameters(), 1.0)
            .unwrap();
        let mut output = [0.0; 4];

        assert!(rack.process_return(first, &[1.0; 4], &mut output, &live(1.0)) > 0.0);
        // The second return's declared input is silence; the wet signal the
        // first return just placed in the output must not excite it.
        let rms = rack.process_return(second, &[0.0; 4], &mut output, &live(1.0));

        assert_eq!(rms, 0.0);
        assert_eq!(output, [1.0; 4]);
    }

    #[test]
    fn failed_processing_contributes_silence_not_passthrough() {
        let mut rack = PreparedBusReturnRack::new(2).unwrap();
        let bus = BusId::new(7).unwrap();
        rack.install(bus, Box::new(FailingEffect), parameters(), 1.0)
            .unwrap();
        let mut output = [0.0; 4];

        let rms = rack.process_return(bus, &[1.0; 4], &mut output, &live(1.0));

        assert_eq!(rms, 0.0);
        assert_eq!(output, [0.0; 4]);
    }

    #[test]
    fn occupancy_can_be_replaced_and_cleared_per_return() {
        let mut rack = PreparedBusReturnRack::new(2).unwrap();
        let bus = BusId::new(2).unwrap();
        rack.install(bus, Box::new(UnityEffect), parameters(), 0.25)
            .unwrap();
        rack.install(bus, Box::new(UnityEffect), parameters(), 0.75)
            .unwrap();
        assert_eq!(rack.return_level(bus), Some(0.75));
        rack.clear(bus);
        assert!(!rack.occupied(bus));
        assert_eq!(rack.occupied_count(), 0);
    }

    #[test]
    fn processing_is_bounded_by_the_prepared_frame_capacity() {
        let mut rack = PreparedBusReturnRack::new(1).unwrap();
        let bus = BusId::new(4).unwrap();
        rack.install(bus, Box::new(UnityEffect), parameters(), 1.0)
            .unwrap();
        let input = [1.0; 8];
        let mut output = [0.0; 8];

        rack.process_return(bus, &input, &mut output, &live(1.0));

        assert_eq!(output[..2], [1.0, 1.0]);
        assert_eq!(output[2..], [0.0; 6]);
    }

    struct MarkerReturnEffect {
        value: f32,
    }

    impl PreparedPostEffect for MarkerReturnEffect {
        fn patch_id(&self) -> PatchId {
            PatchId::new(u32::MAX).unwrap()
        }

        fn slot_id(&self) -> EffectSlotId {
            EffectSlotId::new(1).unwrap()
        }

        fn process(
            &mut self,
            interleaved_stereo: &mut [f32],
            _frame_count: usize,
            _parameters: &RtPostEffectParameters,
        ) -> Result<(), PreparedEffectError> {
            interleaved_stereo.fill(self.value);
            Ok(())
        }
    }

    /// One occupied return with a marker occupant, its recorded effect
    /// capability identity stamped as the graph builder does at build time.
    fn marker_rack(bus: BusId, value: f32, capability: &str) -> PreparedBusReturnRack {
        let mut rack = PreparedBusReturnRack::new(2).unwrap();
        rack.install(
            bus,
            Box::new(MarkerReturnEffect { value }),
            parameters(),
            1.0,
        )
        .unwrap();
        let identity = PositionCapabilityIdentity::from_effect_capability_id(
            &EffectCapabilityId::new(capability).unwrap(),
        )
        .unwrap();
        assert!(rack.record_occupant_identity(bus, EffectSlotId::new(1).unwrap(), identity));
        rack
    }

    fn wet_marker(rack: &mut PreparedBusReturnRack, bus: BusId) -> f32 {
        let mut output = [0.0_f32; 4];
        rack.process_return(bus, &[1.0; 4], &mut output, &live(1.0));
        output[0]
    }

    /// A candidate agreeing on instance identity and scalar layout but
    /// carrying a different recorded occupant capability identity at the
    /// same return is refused: the freshly prepared instance stays and
    /// nothing panics.
    #[test]
    fn carry_over_capability_identity_mismatch_keeps_the_fresh_return_instance() {
        let bus = BusId::new(3).unwrap();
        let mut fresh = marker_rack(bus, 0.25, "effect.alpha");
        let mut superseded = marker_rack(bus, 0.75, "effect.beta");

        fresh.carry_live_returns_from(&mut superseded, None);

        assert_eq!(wet_marker(&mut fresh, bus), 0.25);
        assert_eq!(wet_marker(&mut superseded, bus), 0.75);
    }

    /// Exact per-return agreement — instance identity, scalar layout, and
    /// recorded capability identity — still carries the live occupant and
    /// its tail state over.
    #[test]
    fn carry_over_capability_identity_agreement_still_carries_the_live_return() {
        let bus = BusId::new(3).unwrap();
        let mut fresh = marker_rack(bus, 0.25, "effect.alpha");
        let mut superseded = marker_rack(bus, 0.75, "effect.alpha");

        fresh.carry_live_returns_from(&mut superseded, None);

        assert_eq!(wet_marker(&mut fresh, bus), 0.75);
        assert_eq!(wet_marker(&mut superseded, bus), 0.25);
    }

    /// C-RT-5: a live entry that does not attest the prepared instance
    /// contributes silence — wrong values are never substituted.
    #[test]
    fn mismatched_live_entry_contributes_silence_never_wrong_values() {
        let mut rack = PreparedBusReturnRack::new(2).unwrap();
        let bus = BusId::new(2).unwrap();
        rack.install(bus, Box::new(UnityEffect), parameters(), 1.0)
            .unwrap();
        let input = [1.0; 4];

        let mut output = [0.0; 4];
        let inactive = RtBusReturnParameters::EMPTY;
        assert_eq!(
            rack.process_return(bus, &input, &mut output, &inactive),
            0.0
        );
        assert_eq!(output, [0.0; 4]);

        let wrong_slot =
            RtBusReturnParameters::new(EffectSlotId::new(9).unwrap(), &[], 1.0).unwrap();
        assert_eq!(
            rack.process_return(bus, &input, &mut output, &wrong_slot),
            0.0
        );
        assert_eq!(output, [0.0; 4]);

        let wrong_scalar_count =
            RtBusReturnParameters::new(EffectSlotId::new(1).unwrap(), &[0.5], 1.0).unwrap();
        assert_eq!(
            rack.process_return(bus, &input, &mut output, &wrong_scalar_count),
            0.0
        );
        assert_eq!(output, [0.0; 4]);

        assert!(rack.process_return(bus, &input, &mut output, &live(1.0)) > 0.0);
    }

    /// C-RT-5: rack/snapshot matching is exact per bus — occupancy,
    /// instance identity, and scalar layout must all agree at every index.
    #[test]
    fn matches_parameters_stays_exact_at_every_return() {
        fn snapshot_with(returns: [RtBusReturnParameters; MAX_BUS_RETURNS]) -> ParameterSnapshot {
            ParameterSnapshot::for_graph_with_returns(
                1,
                crate::real_time::GraphRevision::INITIAL,
                GlobalParameters::new(0.0).unwrap(),
                MixerState::default(),
                &[],
                returns,
            )
            .unwrap()
        }

        let bus = BusId::new(3).unwrap();
        let mut rack = PreparedBusReturnRack::new(2).unwrap();
        rack.install(bus, Box::new(UnityEffect), parameters(), 0.5)
            .unwrap();

        let mut exact = [const { RtBusReturnParameters::EMPTY }; MAX_BUS_RETURNS];
        exact[bus.index()] = live(0.25);
        assert!(rack.matches_parameters(&snapshot_with(exact.clone())));
        assert_eq!(rack.slot_id(bus), Some(EffectSlotId::new(1).unwrap()));
        assert_eq!(rack.scalar_count(bus), Some(0));

        // Occupied return facing an inactive entry is rejected.
        assert!(!rack.matches_parameters(&snapshot_with(
            [const { RtBusReturnParameters::EMPTY }; MAX_BUS_RETURNS]
        )));

        // The matching entry at the wrong bus is rejected, not repositioned.
        let mut repositioned = [const { RtBusReturnParameters::EMPTY }; MAX_BUS_RETURNS];
        repositioned[0] = live(0.25);
        assert!(!rack.matches_parameters(&snapshot_with(repositioned)));

        // Wrong instance identity and wrong scalar layout are rejected.
        let mut wrong_slot = exact.clone();
        wrong_slot[bus.index()] =
            RtBusReturnParameters::new(EffectSlotId::new(9).unwrap(), &[], 0.25).unwrap();
        assert!(!rack.matches_parameters(&snapshot_with(wrong_slot)));
        let mut wrong_count = exact.clone();
        wrong_count[bus.index()] =
            RtBusReturnParameters::new(EffectSlotId::new(1).unwrap(), &[0.5], 0.25).unwrap();
        assert!(!rack.matches_parameters(&snapshot_with(wrong_count)));

        // An active entry facing an unoccupied return is rejected.
        let mut extra = exact.clone();
        extra[7] = live(1.0);
        assert!(!rack.matches_parameters(&snapshot_with(extra)));

        // The empty rack attests only the all-inactive bank.
        let empty = PreparedBusReturnRack::new(2).unwrap();
        assert!(empty.matches_parameters(&snapshot_with(
            [const { RtBusReturnParameters::EMPTY }; MAX_BUS_RETURNS]
        )));
        assert!(!empty.matches_parameters(&snapshot_with(exact)));
    }
}
