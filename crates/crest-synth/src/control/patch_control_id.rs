use crate::control::semantic_resolver::row_is_visible_and_enabled;
use crate::mixer::global_parameters::{GlobalParameter, GlobalParameters};
use crate::mixer::patch_output::PatchOutputParameter;
use crate::synth::effect_slot_id::EffectSlotIndex;
use crate::synth::{
    CapabilityDescriptor, EffectCapabilityRegistry, EffectSlotId, InstrumentConfig, ParameterId,
    PatchInteraction, PostEffectConfig, VoiceEnvelope, VoiceEnvelopeParameter,
};
use core::fmt;
use core::str::FromStr;
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::borrow::Cow;
use std::cmp::Ordering;

/// Stable semantic focus identity inside the PATCH context.
///
/// `EffectSlot` is one of the three ordered occupancy rows: its identity is
/// the position alone, so the row survives every occupancy change and an
/// empty slot stays reachable. `Effect` addresses one configured occupant's
/// scalar row through the stable instance and parameter identities.
///
/// `Global` reuses the canonical [`GlobalParameter`] surface descriptor —
/// the very one [`crate::control::MixerControlId::Global`] uses — so PATCH
/// Utility's master-volume row and the MIXER Inspector's address one value
/// through one descriptor with one set of bounds and steps. PATCH owns no
/// copy of it. `MidiInput` and `VoiceLimit`, by contrast, address the focused
/// Patch's *own* channel and limit, so both follow the Patch across a patch
/// selection rather than persisting as surface-local values.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum PatchControlId {
    Send(crate::mixer::bus_id::BusId),
    Engine,
    Output(PatchOutputParameter),
    Envelope(VoiceEnvelopeParameter),
    Capability(ParameterId),
    EffectSlot(EffectSlotIndex),
    Effect(EffectSlotId, ParameterId),
    Global(GlobalParameter),
    MidiInput,
    VoiceLimit,
}

impl PatchControlId {
    pub const ALL: [Self; 1 + VoiceEnvelope::surface_descriptor().len()] = [
        Self::Engine,
        Self::Envelope(VoiceEnvelopeParameter::AttackMilliseconds),
        Self::Envelope(VoiceEnvelopeParameter::DecayMilliseconds),
        Self::Envelope(VoiceEnvelopeParameter::Sustain),
        Self::Envelope(VoiceEnvelopeParameter::ReleaseMilliseconds),
    ];

    pub const fn surface_descriptor() -> &'static [Self] {
        &Self::ALL
    }

    /// The PATCH Utility panel: exactly five rows, in this declared order.
    ///
    /// The order is *written*, not derived from any descriptor's iteration
    /// order, so reordering [`PatchOutputParameter::ALL`] or the global
    /// descriptor cannot silently reshuffle the panel. The set is bounded by
    /// declaration rather than by what fits: five rows seat within the
    /// persistent side region at both authored viewports, so the panel needs
    /// no scroll affordance and no row is reachable only by a gesture the
    /// surface does not declare.
    ///
    /// None of these controls enters the PatchMain order — [`Self::ALL`] is
    /// that order's base, and [`Self::resolve`] appends only descriptor rows.
    pub const UTILITY: [Self; 5] = [
        Self::Global(GlobalParameter::MasterGainDb),
        Self::Output(PatchOutputParameter::TrimGain),
        Self::MidiInput,
        Self::Output(PatchOutputParameter::OutputTrack),
        Self::VoiceLimit,
    ];

    pub const fn utility_surface_descriptor() -> &'static [Self] {
        &Self::UTILITY
    }

    /// Reports whether this identity belongs to the PATCH Utility panel.
    ///
    /// The five Utility identities and the PatchMain order are disjoint; this
    /// predicate is the one place that split is decided, so `FocusPath`
    /// validation and the resolvers cannot disagree about where a row lives.
    pub fn is_utility(&self) -> bool {
        matches!(self, Self::Send(_)) || Self::UTILITY.contains(self)
    }

    pub fn as_str(&self) -> Cow<'_, str> {
        match self {
            Self::Send(bus) => Cow::Owned(format!("patch.send.{}", bus.value())),
            Self::Engine => Cow::Borrowed("patch.engine"),
            Self::Output(PatchOutputParameter::TrimGain) => {
                Cow::Borrowed("patch.output.trimGainDb")
            }
            Self::Output(PatchOutputParameter::OutputTrack) => {
                Cow::Borrowed("patch.output.outputTrack")
            }
            Self::Envelope(VoiceEnvelopeParameter::AttackMilliseconds) => {
                Cow::Borrowed("patch.envelope.attackMilliseconds")
            }
            Self::Envelope(VoiceEnvelopeParameter::DecayMilliseconds) => {
                Cow::Borrowed("patch.envelope.decayMilliseconds")
            }
            Self::Envelope(VoiceEnvelopeParameter::Sustain) => {
                Cow::Borrowed("patch.envelope.sustain")
            }
            Self::Envelope(VoiceEnvelopeParameter::ReleaseMilliseconds) => {
                Cow::Borrowed("patch.envelope.releaseMilliseconds")
            }
            Self::Capability(parameter_id) => {
                Cow::Owned(format!("patch.capability.{parameter_id}"))
            }
            Self::EffectSlot(index) => Cow::Owned(format!("patch.effectSlot.{index}")),
            Self::Effect(slot_id, parameter_id) => {
                Cow::Owned(format!("patch.effect.{slot_id}.{parameter_id}"))
            }
            // Derived from the canonical global descriptor's own field name,
            // not spelled again here, so the two surfaces addressing this one
            // value cannot drift apart in their serialized paths either.
            Self::Global(parameter) => Cow::Owned(format!("patch.global.{}", parameter.name())),
            Self::MidiInput => Cow::Borrowed("patch.midiInput"),
            Self::VoiceLimit => Cow::Borrowed("patch.voiceLimit"),
        }
    }

    /// Resolves the complete PATCH focus surface from the active descriptor and
    /// canonical config. This is the sole ordering/filtering authority used by
    /// reducer navigation and projections.
    ///
    /// After the instrument's visible `StructuralChoice` rows, each of the
    /// three effect slots contributes its occupancy row in ascending
    /// `EffectSlotIndex` order — whether occupied or empty, so an empty
    /// position stays reachable and fillable — followed by the occupant's
    /// visible enabled `ScalarEdit` rows when occupied.
    pub fn resolve(
        descriptor: &CapabilityDescriptor,
        config: &InstrumentConfig,
        effect_registry: &EffectCapabilityRegistry,
        effect_slots: &[Option<PostEffectConfig>],
    ) -> Vec<Self> {
        let mut controls = Self::surface_descriptor().to_vec();
        for spec in descriptor.parameters() {
            if spec.patch_interaction() == PatchInteraction::StructuralChoice
                && row_is_visible_and_enabled(spec, |id| config.value(id))
            {
                controls.push(Self::Capability(spec.id().clone()));
            }
        }
        for (position, occupant) in effect_slots.iter().enumerate() {
            let Ok(index) = EffectSlotIndex::new(position) else {
                continue;
            };
            controls.push(Self::EffectSlot(index));
            let Some(effect) = occupant else {
                continue;
            };
            let Some(effect_descriptor) = effect_registry.descriptor(effect.capability_id()) else {
                continue;
            };
            for spec in effect_descriptor.parameters() {
                if spec.patch_interaction() == PatchInteraction::ScalarEdit
                    && row_is_visible_and_enabled(spec, |id| effect.value(id))
                {
                    controls.push(Self::Effect(effect.slot_id(), spec.id().clone()));
                }
            }
        }
        controls
    }
}

impl Ord for PatchControlId {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_str().cmp(&other.as_str())
    }
}

impl PartialOrd for PatchControlId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for PatchControlId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str().as_ref())
    }
}

impl FromStr for PatchControlId {
    type Err = ParsePatchControlIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            value if value.starts_with("patch.send.") => value[11..]
                .parse::<u16>()
                .ok()
                .and_then(|value| crate::mixer::bus_id::BusId::new(value).ok())
                .map(Self::Send)
                .ok_or(ParsePatchControlIdError),
            "patch.engine" => Ok(Self::Engine),
            "patch.output.trimGainDb" => Ok(Self::Output(PatchOutputParameter::TrimGain)),
            "patch.output.outputTrack" => Ok(Self::Output(PatchOutputParameter::OutputTrack)),
            "patch.envelope.attackMilliseconds" => {
                Ok(Self::Envelope(VoiceEnvelopeParameter::AttackMilliseconds))
            }
            "patch.envelope.decayMilliseconds" => {
                Ok(Self::Envelope(VoiceEnvelopeParameter::DecayMilliseconds))
            }
            "patch.envelope.sustain" => Ok(Self::Envelope(VoiceEnvelopeParameter::Sustain)),
            "patch.envelope.releaseMilliseconds" => {
                Ok(Self::Envelope(VoiceEnvelopeParameter::ReleaseMilliseconds))
            }
            "patch.midiInput" => Ok(Self::MidiInput),
            "patch.voiceLimit" => Ok(Self::VoiceLimit),
            // Resolved through the canonical global surface descriptor rather
            // than a locally spelled name table, so PATCH cannot recognise a
            // global field the one descriptor does not declare.
            value if value.starts_with("patch.global.") => {
                let field = value
                    .strip_prefix("patch.global.")
                    .expect("the prefix was just matched");
                GlobalParameters::surface_descriptor()
                    .iter()
                    .find(|descriptor| descriptor.name() == field)
                    .map(|descriptor| Self::Global(descriptor.parameter()))
                    .ok_or(ParsePatchControlIdError)
            }
            value if value.starts_with("patch.capability.") => {
                let parameter = value
                    .strip_prefix("patch.capability.")
                    .expect("the prefix was just matched");
                ParameterId::new(parameter)
                    .map(Self::Capability)
                    .map_err(|_| ParsePatchControlIdError)
            }
            value if value.starts_with("patch.effectSlot.") => {
                let index = value
                    .strip_prefix("patch.effectSlot.")
                    .expect("the prefix was just matched");
                let index = index
                    .parse::<usize>()
                    .map_err(|_| ParsePatchControlIdError)?;
                EffectSlotIndex::new(index)
                    .map(Self::EffectSlot)
                    .map_err(|_| ParsePatchControlIdError)
            }
            value if value.starts_with("patch.effect.") => {
                let suffix = value
                    .strip_prefix("patch.effect.")
                    .expect("the prefix was just matched");
                let (slot, parameter) = suffix.split_once('.').ok_or(ParsePatchControlIdError)?;
                let slot = slot.parse::<u16>().map_err(|_| ParsePatchControlIdError)?;
                let slot = EffectSlotId::new(slot).map_err(|_| ParsePatchControlIdError)?;
                let parameter =
                    ParameterId::new(parameter).map_err(|_| ParsePatchControlIdError)?;
                Ok(Self::Effect(slot, parameter))
            }
            _ => Err(ParsePatchControlIdError),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParsePatchControlIdError;

impl fmt::Display for ParsePatchControlIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("unknown PATCH control identity")
    }
}

impl std::error::Error for ParsePatchControlIdError {}

impl Serialize for PatchControlId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str().as_ref())
    }
}

impl<'de> Deserialize<'de> for PatchControlId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct PatchControlIdVisitor;

        impl Visitor<'_> for PatchControlIdVisitor {
            type Value = PatchControlId;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a stable PATCH control identity")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                value.parse().map_err(E::custom)
            }
        }

        deserializer.deserialize_str(PatchControlIdVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::PatchControlId;
    use crate::synth::{VoiceEnvelope, VoiceEnvelopeParameter};

    #[test]
    fn patch_control_ids_follow_the_canonical_envelope_descriptor() {
        let expected_envelope = VoiceEnvelope::surface_descriptor()
            .iter()
            .map(|descriptor| PatchControlId::Envelope(descriptor.parameter()))
            .collect::<Vec<_>>();

        assert_eq!(PatchControlId::surface_descriptor().len(), 5);
        assert_eq!(
            PatchControlId::surface_descriptor()[0],
            PatchControlId::Engine
        );
        assert_eq!(
            &PatchControlId::surface_descriptor()[1..],
            expected_envelope
        );
        assert_eq!(
            expected_envelope,
            vec![
                PatchControlId::Envelope(VoiceEnvelopeParameter::AttackMilliseconds),
                PatchControlId::Envelope(VoiceEnvelopeParameter::DecayMilliseconds),
                PatchControlId::Envelope(VoiceEnvelopeParameter::Sustain),
                PatchControlId::Envelope(VoiceEnvelopeParameter::ReleaseMilliseconds),
            ]
        );
    }

    #[test]
    fn every_patch_control_id_has_a_stable_string_round_trip() {
        let names = [
            "patch.engine",
            "patch.envelope.attackMilliseconds",
            "patch.envelope.decayMilliseconds",
            "patch.envelope.sustain",
            "patch.envelope.releaseMilliseconds",
        ];

        for (control, name) in PatchControlId::surface_descriptor().iter().zip(names) {
            assert_eq!(control.as_str(), name);
            assert_eq!(control.to_string(), name);
            assert_eq!(name.parse::<PatchControlId>().unwrap(), *control);
            let json = serde_json::to_string(control).unwrap();
            assert_eq!(json, format!("\"{name}\""));
            assert_eq!(
                serde_json::from_str::<PatchControlId>(&json).unwrap(),
                *control
            );
        }
    }

    #[test]
    fn unknown_patch_control_identity_is_rejected() {
        assert!("patch.envelope.filter".parse::<PatchControlId>().is_err());
        assert!(serde_json::from_str::<PatchControlId>("\"patch.envelope.filter\"").is_err());
    }

    #[test]
    fn effect_slot_occupancy_rows_round_trip_positional_identity() {
        use crate::synth::effect_slot_id::EffectSlotIndex;
        for index in EffectSlotIndex::ALL {
            let control = PatchControlId::EffectSlot(index);
            let name = format!("patch.effectSlot.{}", index.index());
            assert_eq!(control.as_str(), name);
            assert_eq!(name.parse::<PatchControlId>().unwrap(), control);
            let json = serde_json::to_string(&control).unwrap();
            assert_eq!(
                serde_json::from_str::<PatchControlId>(&json).unwrap(),
                control
            );
        }
        assert!("patch.effectSlot.3".parse::<PatchControlId>().is_err());
        assert!("patch.effectSlot.x".parse::<PatchControlId>().is_err());
    }
}
