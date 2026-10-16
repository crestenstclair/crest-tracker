use crate::kernel::midi_channel::MidiChannel;
use crate::kernel::patch_id::PatchId;
use crate::mixer::bus_id::{BusId, DEFAULT_BUS_RETURNS};
use crate::mixer::mixer_track_parameters::BUS_SEND_DESCRIPTOR;
use crate::mixer::patch_output::PatchOutput;
use crate::synth::effect_slot_id::{EffectSlotIndex, MAX_EFFECT_SLOTS};
use crate::synth::instrument_capability::{
    CapabilityDescriptor, CapabilityError, InstrumentConfig, ParameterUpdate, VoicePolicy,
};
use crate::synth::parameter_id::ParameterId;
use crate::synth::voice_envelope::{VoiceEnvelope, VoiceEnvelopeParameter};
use crate::synth::voice_limit::{VoiceLimit, VoiceLimitError};
use crate::synth::{EffectSlotId, PostEffectConfig};
use core::fmt;
use serde::{Deserialize, Serialize};

/// One entry in the canonical schema-derived editable Patch surface.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(tag = "kind", content = "parameter", rename_all = "camelCase")]
pub enum PatchEditableTarget {
    Envelope(VoiceEnvelopeParameter),
    Instrument(ParameterId),
}

impl PatchEditableTarget {
    /// Returns the stable semantic identifier used by projection and coverage.
    pub fn name(&self) -> &str {
        match self {
            Self::Envelope(parameter) => parameter.name(),
            Self::Instrument(parameter) => parameter.as_str(),
        }
    }
}

/// Resolves the only editable Patch ordering from immutable schema and config.
pub fn resolve_patch_editable_targets(
    descriptor: &CapabilityDescriptor,
    config: &InstrumentConfig,
) -> Result<Vec<PatchEditableTarget>, CapabilityError> {
    if descriptor.id() != config.capability_id() {
        return Err(CapabilityError::ProviderRegistryMismatch(
            config.capability_id().clone(),
        ));
    }
    let canonical = descriptor.create_config(config.values(), config.asset_references())?;
    if canonical != *config {
        return Err(CapabilityError::ConfigOrderMismatch(
            config.capability_id().clone(),
        ));
    }

    let mut targets = Vec::with_capacity(
        VoiceEnvelope::surface_descriptor().len() + descriptor.scalar_parameter_count(),
    );
    targets.extend(
        VoiceEnvelope::surface_descriptor()
            .iter()
            .map(|descriptor| PatchEditableTarget::Envelope(descriptor.parameter())),
    );
    targets.extend(
        descriptor
            .parameters()
            .filter(|parameter| parameter.update() == ParameterUpdate::Scalar)
            .map(|parameter| PatchEditableTarget::Instrument(parameter.id().clone())),
    );
    Ok(targets)
}

/// One installed, playable instrument capability configuration.
///
/// A patch's identity, display name, instrument, and assigned MIDI channel are
/// fixed at construction time. Output, sends, envelope, and descriptor-classified
/// values can change only through the canonical reducer.
#[derive(Clone, Debug, PartialEq)]
pub struct Patch {
    id: PatchId,
    name: String,
    instrument: InstrumentConfig,
    channel: MidiChannel,
    envelope: VoiceEnvelope,
    output: PatchOutput,
    /// Independent Patch contributions to the shared effect-return bank.
    sends: Vec<f32>,
    /// The canonical bounded effect chain: exactly `MAX_EFFECT_SLOTS` ordered
    /// positions, each independently empty or occupied. Slot order is render
    /// order, positions are stable addresses, and a fourth effect is
    /// unrepresentable in the type.
    ///
    /// This array is the aggregate's only chain representation. No compacted
    /// or otherwise position-erasing projection is stored beside it, so every
    /// consumer reads positions exactly as they were written and a gapped
    /// chain can never be silently renumbered by a round trip.
    effects: [Option<PostEffectConfig>; MAX_EFFECT_SLOTS],
    /// The canonical Patch-owned ceiling on simultaneously sounding notes.
    ///
    /// Patch-local and following the Patch, exactly like the envelope and the
    /// output route. It is seeded from the Patch's own capability at
    /// installation ([`Patch::seed_voice_limit`]) so no Patch starts
    /// unlimited-by-omission, and thereafter it changes only through the
    /// canonical reducer.
    voice_limit: VoiceLimit,
}

impl Patch {
    /// Creates a patch whose capability's voice policy is not resolved here.
    ///
    /// The limit seeds to the engine-managed ceiling, which is the widest any
    /// installed engine declares. Installation resolves the Patch's own
    /// capability against the registry and re-seeds through
    /// [`Self::seed_voice_limit`], which is the only site with registry access.
    pub fn new(
        id: PatchId,
        name: String,
        instrument: InstrumentConfig,
        channel: MidiChannel,
        output: PatchOutput,
    ) -> Self {
        Self {
            id,
            name,
            instrument,
            channel,
            envelope: VoiceEnvelope::default(),
            output,
            sends: vec![BUS_SEND_DESCRIPTOR.default(); DEFAULT_BUS_RETURNS],
            effects: std::array::from_fn(|_| None),
            voice_limit: VoiceLimit::seeded_from(VoicePolicy::EngineManaged),
        }
    }

    /// Returns this patch's stable process-lifetime identity.
    pub const fn id(&self) -> PatchId {
        self.id
    }

    /// Returns the immutable display name assigned at installation.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the current generic instrument configuration.
    pub const fn instrument_config(&self) -> &InstrumentConfig {
        &self.instrument
    }

    /// Returns the MIDI channel deciding which incoming part drives this Patch.
    ///
    /// Seeded at installation and thereafter editable through the reducer's
    /// PATCH Utility MIDI-input row — see [`Self::set_channel`].
    pub const fn channel(&self) -> MidiChannel {
        self.channel
    }

    /// Returns the canonical Patch-owned ADSR value.
    pub const fn envelope(&self) -> &VoiceEnvelope {
        &self.envelope
    }

    /// Returns this Patch's validated pre-track trim and destination.
    pub const fn output(&self) -> PatchOutput {
        self.output
    }

    pub fn send(&self, bus: BusId) -> f32 {
        self.sends.get(bus.index()).copied().unwrap_or(0.0)
    }

    pub fn sends(&self) -> &[f32] {
        &self.sends
    }

    /// Restores a complete, validated Patch-owned send bank.
    pub fn with_sends(mut self, sends: Vec<f32>) -> Result<Self, PatchSendError> {
        validate_send_count(sends.len())?;
        for (index, value) in sends.iter().copied().enumerate() {
            let bus = BusId::new(index as u16).expect("validated send count fits BusId");
            validate_send(bus, value)?;
        }
        self.sends = sends;
        Ok(self)
    }

    /// Shapes control-side storage without discarding an audible send route.
    pub fn with_send_count(mut self, count: usize) -> Result<Self, PatchSendError> {
        validate_send_count(count)?;
        if let Some(index) = self
            .sends
            .iter()
            .enumerate()
            .skip(count)
            .find_map(|(index, amount)| (*amount != 0.0).then_some(index))
        {
            return Err(PatchSendError::WouldDiscardSend {
                bus: BusId::new(index as u16).expect("installed send count fits BusId"),
            });
        }
        self.sends.resize(count, BUS_SEND_DESCRIPTOR.default());
        Ok(self)
    }

    /// Applies a scalar edit only to an installed send, without changing shape.
    pub(crate) fn set_send(&mut self, bus: BusId, value: f32) -> Result<(), PatchSendError> {
        validate_send(bus, value)?;
        let target = self
            .sends
            .get_mut(bus.index())
            .ok_or(PatchSendError::UnknownSend { bus })?;
        *target = value;
        Ok(())
    }

    /// Returns the canonical Patch-owned ceiling on simultaneously sounding
    /// notes. This is the value carried to the callback on the latest-scalar
    /// snapshot and enforced there at note-on.
    pub const fn voice_limit(&self) -> VoiceLimit {
        self.voice_limit
    }

    /// Returns the canonical ordered effect chain, one entry per position.
    ///
    /// This is the only chain view the aggregate exposes. Slot order is render
    /// order and empty positions stay in place: clearing slot 1 leaves slot 2
    /// occupied at index 2, never compacted down.
    pub const fn effect_slots(&self) -> &[Option<PostEffectConfig>; MAX_EFFECT_SLOTS] {
        &self.effects
    }

    /// Returns the configuration occupying one validated position, if any.
    pub fn effect_slot(&self, index: EffectSlotIndex) -> Option<&PostEffectConfig> {
        self.effects[index.index()].as_ref()
    }

    /// Replaces the complete bounded Patch output value through the reducer.
    pub(crate) fn set_output(&mut self, output: PatchOutput) {
        self.output = output;
    }

    /// Supplies a non-default envelope while constructing a Patch fixture.
    pub fn with_envelope(mut self, envelope: VoiceEnvelope) -> Self {
        self.envelope = envelope;
        self
    }

    /// Supplies one already-validated saved voice limit while reconstructing
    /// persistent session data. Installation still validates it against the
    /// restored capability's declared ceiling before it becomes canonical.
    pub fn with_voice_limit(mut self, value: u16) -> Result<Self, VoiceLimitError> {
        self.voice_limit = VoiceLimit::new(value)?;
        Ok(self)
    }

    /// Seeds this Patch's limit from its own capability's declared per-Patch
    /// polyphony ceiling, returning the seeded value.
    ///
    /// This is the installation transition: it runs once, when the Patch's
    /// capability has been resolved against the immutable registry, so the Patch
    /// never enters canonical state unlimited-by-omission or carrying a limit
    /// its own engine could not honour.
    pub fn seed_voice_limit(&mut self, policy: VoicePolicy) -> VoiceLimit {
        self.voice_limit = VoiceLimit::seeded_from(policy);
        self.voice_limit
    }

    /// Applies the reducer's voice-limit adjustment, validating against the
    /// declared bounds.
    ///
    /// # Errors
    ///
    /// Returns [`VoiceLimitError::OutOfRange`] and leaves the Patch untouched
    /// when the requested value falls outside the bounds — refused, never
    /// clamped or wrapped.
    pub(crate) fn set_voice_limit(&mut self, value: u16) -> Result<VoiceLimit, VoiceLimitError> {
        let limit = VoiceLimit::new(value)?;
        self.voice_limit = limit;
        Ok(limit)
    }

    /// Replaces the complete instrument configuration for an engine change and
    /// reconciles the Patch's limit with the new engine's declared ceiling.
    ///
    /// The limit survives the swap: an engine replacement replaces the
    /// `InstrumentConfig`, not the player's chosen limit. If the incoming
    /// engine's ceiling is lower than the current limit, the limit is narrowed
    /// to it *here* — at the one point the new ceiling becomes known — and the
    /// narrowing is reported in the returned outcome rather than left for a
    /// caller to discover, so the Patch never carries a limit the new engine
    /// could not honour.
    pub fn replace_instrument_config(
        &mut self,
        config: InstrumentConfig,
        policy: VoicePolicy,
    ) -> VoiceLimitCarryOver {
        self.instrument = config;
        let carry_over = VoiceLimitCarryOver::resolve(self.voice_limit, policy);
        self.voice_limit = carry_over.limit();
        carry_over
    }

    /// Occupies one validated position while constructing a Patch, leaving
    /// every other position exactly as it stands.
    ///
    /// Construction is position-explicit: the caller names the address, so a
    /// list order is never reinterpreted as a chain layout and a fixture can
    /// state a gapped chain directly. Installation validates registry identity
    /// and config; the type itself bounds the chain, so a fourth position is
    /// unrepresentable rather than refused.
    ///
    /// # Panics
    ///
    /// Panics when the occupant's instance identity already occupies another
    /// position. A duplicate identity is a construction-time programmer error;
    /// the reducer path reports it as `EffectSlotOccupancyError` instead.
    pub fn with_effect_slot(mut self, index: EffectSlotIndex, occupant: PostEffectConfig) -> Self {
        self.set_slot_occupancy(index, Some(occupant))
            .expect("a constructed occupant carries an identity unique to its Patch");
        self
    }

    /// Applies the structural `SetSlotOccupancy` domain transition: occupies,
    /// replaces, or clears exactly one validated position, leaving every other
    /// position untouched. Occupancy changes what exists, so it travels the
    /// prepared-structural-change path, never the scalar snapshot.
    ///
    /// The occupant carries its own stable `EffectSlotId` instance identity;
    /// an identity already occupying another position is rejected, never
    /// silently replaced.
    pub(crate) fn set_slot_occupancy(
        &mut self,
        index: EffectSlotIndex,
        occupant: Option<PostEffectConfig>,
    ) -> Result<(), EffectSlotOccupancyError> {
        if let Some(config) = &occupant {
            let duplicate = self
                .effects
                .iter()
                .enumerate()
                .filter(|(position, _)| *position != index.index())
                .filter_map(|(_, slot)| slot.as_ref())
                .any(|other| other.slot_id() == config.slot_id());
            if duplicate {
                return Err(EffectSlotOccupancyError::DuplicateSlotId(config.slot_id()));
            }
        }
        self.effects[index.index()] = occupant;
        Ok(())
    }

    /// Resolves this Patch's common ADSR and live instrument targets.
    pub fn editable_targets(
        &self,
        descriptor: &CapabilityDescriptor,
    ) -> Result<Vec<PatchEditableTarget>, CapabilityError> {
        resolve_patch_editable_targets(descriptor, &self.instrument)
    }

    pub(crate) fn set_envelope(&mut self, envelope: VoiceEnvelope) {
        self.envelope = envelope;
    }

    /// Changes the incoming MIDI channel this Patch subscribes to.
    ///
    /// The channel is already validated into `0..=15` by its own type, so
    /// there is nothing left to refuse here. MIDI subscriptions are
    /// intentionally many-to-many: other installed Patches may use the same
    /// channel and will receive the same incoming messages.
    pub(crate) fn set_channel(&mut self, channel: MidiChannel) {
        self.channel = channel;
    }

    pub(crate) fn set_instrument_config(&mut self, config: InstrumentConfig) {
        self.instrument = config;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, thiserror::Error)]
pub enum PatchSendError {
    #[error("Patch send count {count} exceeds the BusId representation")]
    SendCountOutOfRange { count: usize },
    #[error("Patch send {bus} must be finite")]
    NonFiniteSend { bus: BusId },
    #[error("Patch send {bus} must be in 0.0..=1.0, got {value}")]
    OutOfRangeSend { bus: BusId, value: f32 },
    #[error("Patch send {bus} is not installed")]
    UnknownSend { bus: BusId },
    #[error("shrinking the Patch send bank would discard nonzero send {bus}")]
    WouldDiscardSend { bus: BusId },
}

fn validate_send_count(count: usize) -> Result<(), PatchSendError> {
    if count > usize::from(BusId::MAX) + 1 {
        return Err(PatchSendError::SendCountOutOfRange { count });
    }
    Ok(())
}

fn validate_send(bus: BusId, value: f32) -> Result<(), PatchSendError> {
    if !value.is_finite() {
        return Err(PatchSendError::NonFiniteSend { bus });
    }
    if !BUS_SEND_DESCRIPTOR.contains(value) {
        return Err(PatchSendError::OutOfRangeSend { bus, value });
    }
    Ok(())
}

/// What an engine replacement did to the Patch's voice limit.
///
/// The type exists so the narrowing is a reported outcome rather than a silent
/// mutation a caller has to go looking for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VoiceLimitCarryOver {
    /// The incoming engine honours the existing limit, which crossed unchanged.
    Preserved(VoiceLimit),
    /// The incoming engine's ceiling is lower, so the limit was narrowed to it.
    Clamped {
        /// The limit the Patch carried before the swap.
        ///
        /// **No production consumer, deliberately** (F-02 item 4, N1). The
        /// discriminant alone is what the *transient* report needs: the
        /// projection resolves this outcome before the commit, so the row it
        /// annotates still carries the previous limit as its own projected
        /// value and `requested_value` carries the clamped one. Reading
        /// `previous` there would project the value the row already shows.
        ///
        /// It earns a consumer only in the durable engine-selection report,
        /// where the outcome is retained on `EngineSelectionStatus` after the
        /// commit has already overwritten canonical state — at which point the
        /// previous limit is no longer recoverable from the row and this field
        /// is the only thing that still knows it. That needs a canonical field
        /// in application state. It is kept rather than deleted for exactly
        /// that reason, and the reducer test
        /// `an_engine_swap_to_a_narrower_engine_clamps_the_limit_in_canonical_state`
        /// asserts the whole outcome, so the field cannot silently go wrong in
        /// the meantime.
        previous: VoiceLimit,
        limit: VoiceLimit,
    },
}

impl VoiceLimitCarryOver {
    /// Decides what an engine replacement does to one existing limit, without
    /// performing it.
    ///
    /// This is the one place the declared asymmetry lives — narrowing clamps
    /// because a limit the engine cannot honour is not a limit; widening
    /// preserves the player's value because an engine change is not a request
    /// to change it. [`Patch::replace_instrument_config`] applies the answer
    /// and a projection showing the limit while a swap narrows it reads the
    /// same answer to say so, so the two can never disagree about whether a
    /// swap costs the player anything.
    pub const fn resolve(current: VoiceLimit, policy: VoicePolicy) -> Self {
        let ceiling = VoiceLimit::seeded_from_ceiling(policy.polyphony_ceiling());
        if current.value() <= ceiling.value() {
            Self::Preserved(current)
        } else {
            Self::Clamped {
                previous: current,
                limit: ceiling,
            }
        }
    }

    /// Returns the limit the Patch carries after the replacement.
    pub const fn limit(self) -> VoiceLimit {
        match self {
            Self::Preserved(limit) | Self::Clamped { limit, .. } => limit,
        }
    }

    /// Returns whether the incoming engine's ceiling narrowed the limit.
    pub const fn was_clamped(self) -> bool {
        matches!(self, Self::Clamped { .. })
    }
}

/// The reason a `SetSlotOccupancy` transition was rejected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectSlotOccupancyError {
    /// The occupant's instance identity already occupies another position.
    DuplicateSlotId(EffectSlotId),
}

impl fmt::Display for EffectSlotOccupancyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateSlotId(slot_id) => {
                write!(
                    formatter,
                    "effect instance {slot_id} already occupies another slot"
                )
            }
        }
    }
}

impl std::error::Error for EffectSlotOccupancyError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mixer::mixer_track_id::MixerTrackId;
    use crate::synth::capability_id::CapabilityId;
    use crate::synth::EffectCapabilityId;

    fn test_patch() -> Patch {
        Patch::new(
            PatchId::new(1).unwrap(),
            "Test".to_owned(),
            InstrumentConfig::from_parts(
                CapabilityId::new("instrument.test").unwrap(),
                Vec::new(),
                Vec::new(),
            ),
            MidiChannel::new(0).unwrap(),
            PatchOutput::default(),
        )
    }

    fn config(slot_id: u16) -> PostEffectConfig {
        PostEffectConfig::from_parts(
            EffectSlotId::new(slot_id).unwrap(),
            EffectCapabilityId::new("effect.test").unwrap(),
            Vec::new(),
            Vec::new(),
        )
    }

    fn slot(index: usize) -> EffectSlotIndex {
        EffectSlotIndex::new(index).unwrap()
    }

    #[test]
    fn sends_are_independent_for_patches_routed_to_the_same_track() {
        let mut first = test_patch();
        let mut second = first.clone();
        let bus = BusId::new(5).unwrap();
        assert_eq!(first.sends(), &[0.0; DEFAULT_BUS_RETURNS]);
        first.set_send(bus, 0.25).unwrap();
        second.set_send(bus, 0.75).unwrap();
        assert_eq!(first.output().track_id(), second.output().track_id());
        assert_eq!(first.send(bus), 0.25);
        assert_eq!(second.send(bus), 0.75);
        let expanded = first.with_send_count(40).unwrap();
        assert_eq!(expanded.send(bus), 0.25);
        assert!(expanded.sends()[DEFAULT_BUS_RETURNS..]
            .iter()
            .all(|value| *value == 0.0));
        assert_eq!(
            test_patch()
                .with_send_count(usize::from(BusId::MAX) + 1)
                .unwrap()
                .sends()
                .len(),
            65_536
        );
    }

    #[test]
    fn invalid_send_edits_are_rejected_without_mutation_or_clamping() {
        let mut patch = test_patch();
        let before = patch.clone();
        let bus = BusId::default();
        for value in [-0.1, 1.1, f32::INFINITY, f32::NAN] {
            assert!(patch.set_send(bus, value).is_err());
            assert_eq!(patch, before);
            assert!(test_patch().with_sends(vec![value]).is_err());
        }
        let missing = BusId::new(DEFAULT_BUS_RETURNS as u16).unwrap();
        assert_eq!(
            patch.set_send(missing, 0.5),
            Err(PatchSendError::UnknownSend { bus: missing })
        );
        assert_eq!(patch, before);
        let count = usize::from(BusId::MAX) + 2;
        assert_eq!(
            test_patch().with_send_count(count),
            Err(PatchSendError::SendCountOutOfRange { count })
        );
        assert_eq!(
            test_patch().with_sends(vec![0.0; count]),
            Err(PatchSendError::SendCountOutOfRange { count })
        );
    }

    #[test]
    fn shrinking_send_storage_refuses_nonzero_routes_but_allows_zero_tail() {
        let mut patch = test_patch().with_send_count(19).unwrap();
        let kept = BusId::new(2).unwrap();
        let outside = BusId::new(18).unwrap();
        patch.set_send(kept, 0.25).unwrap();
        patch.set_send(outside, 0.75).unwrap();
        assert_eq!(
            patch.clone().with_send_count(DEFAULT_BUS_RETURNS),
            Err(PatchSendError::WouldDiscardSend { bus: outside })
        );
        patch.set_send(outside, 0.0).unwrap();
        let resized = patch.with_send_count(DEFAULT_BUS_RETURNS).unwrap();
        assert_eq!(resized.sends().len(), DEFAULT_BUS_RETURNS);
        assert_eq!(resized.send(kept), 0.25);
    }

    #[test]
    fn patch_holds_zero_through_three_ordered_effects() {
        for count in 0..=MAX_EFFECT_SLOTS {
            let configs: Vec<_> = (0..count).map(|index| config(index as u16 + 1)).collect();
            let patch =
                configs
                    .iter()
                    .enumerate()
                    .fold(test_patch(), |patch, (position, occupant)| {
                        patch.with_effect_slot(slot(position), occupant.clone())
                    });
            assert_eq!(patch.effect_slots().len(), MAX_EFFECT_SLOTS);
            for position in 0..MAX_EFFECT_SLOTS {
                assert_eq!(
                    patch.effect_slot(slot(position)),
                    configs.get(position),
                    "position {position} with {count} installed"
                );
            }
        }
    }

    #[test]
    fn a_fourth_position_is_unrepresentable() {
        assert_eq!(test_patch().effect_slots().len(), MAX_EFFECT_SLOTS);
        assert_eq!(
            EffectSlotIndex::new(MAX_EFFECT_SLOTS),
            Err(
                crate::synth::effect_slot_id::EffectSlotIndexError::OutOfRange {
                    value: MAX_EFFECT_SLOTS
                }
            )
        );
    }

    #[test]
    fn each_position_sets_replaces_and_clears_independently() {
        for position in 0..MAX_EFFECT_SLOTS {
            let mut patch = test_patch();
            for occupied in 0..MAX_EFFECT_SLOTS {
                patch
                    .set_slot_occupancy(slot(occupied), Some(config(occupied as u16 + 1)))
                    .unwrap();
            }

            let replacement = PostEffectConfig::from_parts(
                EffectSlotId::new(position as u16 + 1).unwrap(),
                EffectCapabilityId::new("effect.other").unwrap(),
                Vec::new(),
                Vec::new(),
            );
            patch
                .set_slot_occupancy(slot(position), Some(replacement.clone()))
                .unwrap();
            assert_eq!(patch.effect_slot(slot(position)), Some(&replacement));

            patch.set_slot_occupancy(slot(position), None).unwrap();
            assert_eq!(patch.effect_slot(slot(position)), None);
            for other in (0..MAX_EFFECT_SLOTS).filter(|other| *other != position) {
                assert_eq!(
                    patch.effect_slot(slot(other)),
                    Some(&config(other as u16 + 1)),
                    "clearing position {position} must preserve position {other}"
                );
            }
        }
    }

    #[test]
    fn clearing_a_slot_never_compacts_the_others() {
        let mut patch = test_patch()
            .with_effect_slot(slot(0), config(1))
            .with_effect_slot(slot(1), config(2))
            .with_effect_slot(slot(2), config(3));
        patch.set_slot_occupancy(slot(1), None).unwrap();

        assert_eq!(patch.effect_slot(slot(0)), Some(&config(1)));
        assert_eq!(patch.effect_slot(slot(1)), None);
        assert_eq!(patch.effect_slot(slot(2)), Some(&config(3)));
        // The survivors keep their addresses and their instance identities:
        // nothing slid down into the hole the cleared position left.
        assert_eq!(
            patch.effect_slots()[2]
                .as_ref()
                .map(PostEffectConfig::slot_id),
            Some(EffectSlotId::new(3).unwrap())
        );
    }

    #[test]
    fn an_occupied_later_position_leaves_the_earlier_one_empty() {
        let patch = test_patch().with_effect_slot(slot(1), config(2));

        assert_eq!(patch.effect_slot(slot(0)), None);
        assert_eq!(patch.effect_slot(slot(1)), Some(&config(2)));
        assert_eq!(patch.effect_slot(slot(2)), None);
        assert_eq!(
            patch.effect_slots(),
            &[None, Some(config(2)), None],
            "an occupied second position is reported at index 1, never at index 0"
        );
    }

    #[test]
    fn instance_identities_stay_stable_across_occupancy_changes_elsewhere() {
        let mut patch = test_patch()
            .with_effect_slot(slot(0), config(1))
            .with_effect_slot(slot(1), config(2))
            .with_effect_slot(slot(2), config(3));
        let identities_before: Vec<_> = patch
            .effect_slots()
            .iter()
            .map(|occupant| occupant.as_ref().map(PostEffectConfig::slot_id))
            .collect();

        patch.set_slot_occupancy(slot(1), None).unwrap();
        patch.set_slot_occupancy(slot(1), Some(config(9))).unwrap();

        let identities_after: Vec<_> = patch
            .effect_slots()
            .iter()
            .map(|occupant| occupant.as_ref().map(PostEffectConfig::slot_id))
            .collect();
        assert_eq!(identities_before[0], identities_after[0]);
        assert_eq!(identities_after[1], Some(EffectSlotId::new(9).unwrap()));
        assert_eq!(identities_before[2], identities_after[2]);
        let occupied: Vec<_> = identities_after.iter().flatten().collect();
        let mut unique = occupied.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            occupied.len(),
            unique.len(),
            "every occupied position holds a distinct instance identity"
        );
    }

    #[test]
    fn duplicate_instance_identity_is_rejected_not_replaced() {
        let mut patch = test_patch();
        patch.set_slot_occupancy(slot(0), Some(config(7))).unwrap();

        assert_eq!(
            patch.set_slot_occupancy(slot(2), Some(config(7))),
            Err(EffectSlotOccupancyError::DuplicateSlotId(
                EffectSlotId::new(7).unwrap()
            ))
        );
        assert_eq!(patch.effect_slot(slot(2)), None);
        assert_eq!(patch.effect_slot(slot(0)), Some(&config(7)));

        // Replacing an instance in place with its own identity stays valid.
        patch.set_slot_occupancy(slot(0), Some(config(7))).unwrap();
        assert_eq!(patch.effect_slot(slot(0)), Some(&config(7)));
    }

    /// The fluent constructor must route through the same duplicate-identity
    /// guard as the reducer path, not write the slot array directly.
    ///
    /// Nothing else pins that routing: a constructor that assigned
    /// `self.effects[index] = Some(occupant)` would let one instance identity
    /// occupy two positions and leave every other test green, which is exactly
    /// the bypass the retired compact chain constructor allowed. The panic
    /// message is asserted so the pin fails on a silent bypass rather than on
    /// any incidental panic.
    #[test]
    #[should_panic(expected = "a constructed occupant carries an identity unique to its Patch")]
    fn with_effect_slot_refuses_one_instance_identity_at_two_positions() {
        let _duplicate = test_patch()
            .with_effect_slot(slot(0), config(7))
            .with_effect_slot(slot(2), config(7));
    }

    #[test]
    fn rerouting_the_output_leaves_the_effect_chain_untouched() {
        let mut patch = test_patch()
            .with_effect_slot(slot(0), config(1))
            .with_effect_slot(slot(1), config(2));
        patch.set_slot_occupancy(slot(1), None).unwrap();
        let chain_before = patch.effect_slots().clone();

        patch.set_output(PatchOutput::new(MixerTrackId::new(9).unwrap(), -4.5).unwrap());

        assert_eq!(patch.effect_slots(), &chain_before);
        assert_eq!(
            patch.output(),
            PatchOutput::new(MixerTrackId::new(9).unwrap(), -4.5).unwrap()
        );
    }

    #[test]
    fn replacing_a_gapped_occupant_addresses_its_own_position() {
        let mut patch = test_patch()
            .with_effect_slot(slot(0), config(1))
            .with_effect_slot(slot(2), config(3));

        let replacement = PostEffectConfig::from_parts(
            EffectSlotId::new(3).unwrap(),
            EffectCapabilityId::new("effect.other").unwrap(),
            Vec::new(),
            Vec::new(),
        );
        patch
            .set_slot_occupancy(slot(2), Some(replacement.clone()))
            .unwrap();

        assert_eq!(patch.effect_slot(slot(2)), Some(&replacement));
        assert_eq!(patch.effect_slot(slot(1)), None);
        assert_eq!(patch.effect_slot(slot(0)), Some(&config(1)));
    }

    // ---- Canonical voice limit (FR-009) -----------------------------------

    /// Seeding is per capability. The two installed engines declare different
    /// ceilings, and each Patch takes its own — a shared seed would make these
    /// two assertions the same number.
    #[test]
    fn a_soundfont_patch_and_a_braids_patch_seed_from_their_own_ceilings() {
        use crate::adapter::braids_capability::{BraidsCapability, BRAIDS_FIXED_VOICES};
        use crate::adapter::hidef_soundfont_capability::HIDEF_POLYPHONY_CEILING;
        use crate::adapter::production_instruments::production_soundfont_capability;
        use crate::synth::instrument_capability_provider::InstrumentCapabilityProvider;

        let soundfont = production_soundfont_capability().unwrap().descriptor();
        let braids = BraidsCapability::new().unwrap().descriptor();

        let soundfont_patch = installed_patch(1, soundfont.voice_policy());
        let braids_patch = installed_patch(2, braids.voice_policy());

        assert_eq!(
            soundfont_patch.voice_limit().value(),
            HIDEF_POLYPHONY_CEILING
        );
        assert_eq!(braids_patch.voice_limit().value(), BRAIDS_FIXED_VOICES);
        assert_ne!(
            soundfont_patch.voice_limit(),
            braids_patch.voice_limit(),
            "a shared seed would collapse two different engine ceilings into one"
        );
    }

    /// No installed Patch is left unlimited-by-omission, and none is left at the
    /// type's maximum as a stand-in for "unset": a Patch whose engine declares
    /// sixteen voices carries sixteen.
    #[test]
    fn every_installed_patch_carries_its_own_engine_ceiling_not_the_type_maximum() {
        use crate::adapter::production_instruments::production_capability_registry;

        let registry = production_capability_registry().unwrap();
        let mut below_maximum = 0;
        for (index, descriptor) in registry.descriptors().iter().enumerate() {
            let policy = descriptor.voice_policy();
            let patch = installed_patch(index as u32 + 1, policy);
            assert_eq!(
                patch.voice_limit().value(),
                policy.initial_voices(),
                "{} must seed from its own declared ceiling",
                descriptor.id().as_str()
            );
            assert!(patch.voice_limit().value() >= VoiceLimit::MINIMUM);
            if patch.voice_limit().value() < VoiceLimit::MAXIMUM {
                below_maximum += 1;
            }
        }
        assert!(
            below_maximum > 0,
            "at least one installed engine seeds below the type maximum, so a \
             blanket maximum default would be visible here"
        );
    }

    #[test]
    fn the_reducer_facing_setter_refuses_values_outside_the_bound() {
        let mut patch = test_patch();
        let seeded = patch.voice_limit();

        assert_eq!(patch.set_voice_limit(24).unwrap().value(), 24);
        assert_eq!(patch.voice_limit().value(), 24);

        assert_eq!(
            patch.set_voice_limit(0),
            Err(VoiceLimitError::OutOfRange { value: 0 })
        );

        assert_eq!(
            patch.voice_limit().value(),
            24,
            "a refused adjustment leaves the canonical value untouched"
        );
        assert_ne!(seeded.value(), 24, "the fixture actually moved the value");
    }

    /// An engine replacement replaces the config, not the player's limit — and
    /// when the incoming engine cannot honour it, the narrowing is reported.
    #[test]
    fn a_configurable_engine_swap_preserves_the_voice_budget() {
        use crate::adapter::braids_capability::{BraidsCapability, BRAIDS_CAPABILITY_ID};
        use crate::synth::instrument_capability_provider::InstrumentCapabilityProvider;

        let braids_policy = BraidsCapability::new().unwrap().descriptor().voice_policy();
        let braids_config = InstrumentConfig::from_parts(
            CapabilityId::new(BRAIDS_CAPABILITY_ID).unwrap(),
            Vec::new(),
            Vec::new(),
        );

        // A limit the incoming engine can honour crosses unchanged.
        let mut patch = test_patch();
        patch.set_voice_limit(8).unwrap();
        let carry_over = patch.replace_instrument_config(braids_config.clone(), braids_policy);
        assert_eq!(
            carry_over,
            VoiceLimitCarryOver::Preserved(VoiceLimit::new(8).unwrap())
        );
        assert!(!carry_over.was_clamped());
        assert_eq!(patch.voice_limit().value(), 8);
        assert_eq!(patch.instrument_config(), &braids_config);

        // Configurable engines preserve a budget above their initial default.
        let mut patch = test_patch();
        patch.set_voice_limit(129).unwrap();
        let carry_over = patch.replace_instrument_config(braids_config, braids_policy);
        assert_eq!(
            carry_over,
            VoiceLimitCarryOver::Preserved(VoiceLimit::new(129).unwrap())
        );
        assert_eq!(patch.voice_limit().value(), 129);
    }

    #[test]
    fn a_limit_change_touches_nothing_else_the_patch_owns() {
        let mut patch = test_patch()
            .with_effect_slot(slot(0), config(1))
            .with_effect_slot(slot(2), config(3))
            .with_envelope(VoiceEnvelope::new(5.0, 25.0, 0.5, 125.0).unwrap());
        patch.set_output(PatchOutput::new(MixerTrackId::new(4).unwrap(), -2.5).unwrap());
        let before = patch.clone();

        patch.set_voice_limit(11).unwrap();

        assert_eq!(patch.voice_limit().value(), 11);
        assert_eq!(patch.id(), before.id());
        assert_eq!(patch.name(), before.name());
        assert_eq!(patch.channel(), before.channel());
        assert_eq!(patch.instrument_config(), before.instrument_config());
        assert_eq!(patch.envelope(), before.envelope());
        assert_eq!(patch.effect_slots(), before.effect_slots());
        assert_eq!(patch.output(), before.output());
    }

    /// Composes an installed Patch exactly the way the reducer does:
    /// `Patch::new`, then `seed_voice_limit` once the capability has been
    /// resolved against the registry. There is deliberately no constructor
    /// that does both — the reducer needs the registry lookup *between* them,
    /// so a second way to build an installed Patch would be one the production
    /// installation site could never use.
    fn installed_patch(id: u32, policy: VoicePolicy) -> Patch {
        let mut patch = Patch::new(
            PatchId::new(id).unwrap(),
            format!("Patch {id}"),
            InstrumentConfig::from_parts(
                CapabilityId::new("instrument.test").unwrap(),
                Vec::new(),
                Vec::new(),
            ),
            MidiChannel::new(((id - 1) % 16) as u8).unwrap(),
            PatchOutput::default(),
        );
        patch.seed_voice_limit(policy);
        patch
    }

    #[test]
    fn public_api_keeps_configuration_read_only() {
        let _: fn(PatchId, String, InstrumentConfig, MidiChannel, PatchOutput) -> Patch =
            Patch::new;
        let _: fn(&Patch) -> PatchId = Patch::id;
        let _: for<'a> fn(&'a Patch) -> &'a str = Patch::name;
        let _: for<'a> fn(&'a Patch) -> &'a InstrumentConfig = Patch::instrument_config;
        let _: fn(&Patch) -> MidiChannel = Patch::channel;
        let _: for<'a> fn(&'a Patch) -> &'a VoiceEnvelope = Patch::envelope;
        let _: fn(&Patch) -> PatchOutput = Patch::output;
        let _: fn(&Patch) -> VoiceLimit = Patch::voice_limit;
        let _: for<'a> fn(&'a Patch) -> &'a [Option<PostEffectConfig>; MAX_EFFECT_SLOTS] =
            Patch::effect_slots;
        let _: fn(Patch, EffectSlotIndex, PostEffectConfig) -> Patch = Patch::with_effect_slot;

        let config = InstrumentConfig::from_parts(
            CapabilityId::new("instrument.test").unwrap(),
            Vec::new(),
            Vec::new(),
        );
        let patch = Patch::new(
            PatchId::new(1).unwrap(),
            "Test".to_owned(),
            config.clone(),
            MidiChannel::new(0).unwrap(),
            PatchOutput::default(),
        );
        assert_eq!(patch.instrument_config(), &config);
        assert_eq!(patch.envelope(), &VoiceEnvelope::default());
    }
}
