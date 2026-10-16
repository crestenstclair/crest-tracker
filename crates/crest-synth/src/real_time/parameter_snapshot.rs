use crate::kernel::patch_id::PatchId;
pub use crate::kernel::MAX_ACTIVE_PATCHES;
use crate::mixer::bus_id::{BusId, MAX_BUS_RETURNS};
use crate::mixer::global_parameters::GlobalParameters;
use crate::mixer::mixer_state::MixerState;
use crate::mixer::mixer_track_id::MixerTrackId;
use crate::mixer::mixer_track_parameters::MixerTrackParameters;
use crate::mixer::patch_output::PatchOutput;
use crate::real_time::graph_revision::GraphRevision;
use crate::synth::effect_slot_id::MAX_EFFECT_SLOTS;
use crate::synth::instrument_capability::CapabilityRegistry;
use crate::synth::patch::Patch;
use crate::synth::voice_envelope::VoiceEnvelope;
use crate::synth::voice_limit::VoiceLimit;
use crate::synth::{EffectCapabilityRegistry, EffectSlotId};
use core::fmt;
use serde::{Serialize, Serializer};

/// Descriptor-ordered live instrument values allocated on the control thread.
///
/// Choice values are encoded as descriptor indices, toggles as 0/1, and
/// numeric values directly. Storage contains exactly the declared values.
#[derive(Clone, Debug, PartialEq)]
pub struct RtInstrumentParameters {
    values: Vec<f32>,
}

impl RtInstrumentParameters {
    pub const EMPTY: Self = Self { values: Vec::new() };

    /// Prepares the complete descriptor-ordered scalar storage off callback.
    pub fn new(values: &[f32]) -> Result<Self, ParameterSnapshotError> {
        if let Some(index) = values.iter().position(|value| !value.is_finite()) {
            return Err(ParameterSnapshotError::NonFiniteInstrumentScalar { index });
        }
        Ok(Self {
            values: values.to_vec(),
        })
    }

    pub const fn count(&self) -> usize {
        self.values.len()
    }

    pub fn values(&self) -> &[f32] {
        &self.values[..self.count()]
    }

    pub fn value(&self, index: usize) -> Option<f32> {
        self.values().get(index).copied()
    }

    pub fn storage(&self) -> &[f32] {
        &self.values
    }
}

impl Serialize for RtInstrumentParameters {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct SerializableInstrumentParameters<'a> {
            count: usize,
            values: &'a [f32],
        }

        SerializableInstrumentParameters {
            count: self.count(),
            values: self.values(),
        }
        .serialize(serializer)
    }
}

impl Default for RtInstrumentParameters {
    fn default() -> Self {
        Self::EMPTY
    }
}

/// Descriptor-ordered effect scalars prepared off callback and exchanged by ownership.
#[derive(Clone, Debug, PartialEq)]
pub struct RtPostEffectParameters {
    slot_id: Option<EffectSlotId>,
    scalars: Vec<f32>,
}

impl RtPostEffectParameters {
    pub const EMPTY: Self = Self {
        slot_id: None,
        scalars: Vec::new(),
    };

    pub fn new(slot_id: EffectSlotId, scalars: &[f32]) -> Result<Self, ParameterSnapshotError> {
        if let Some(index) = scalars.iter().position(|value| !value.is_finite()) {
            return Err(ParameterSnapshotError::NonFiniteEffectScalar { index });
        }

        Ok(Self {
            slot_id: Some(slot_id),
            scalars: scalars.to_vec(),
        })
    }

    pub const fn is_active(&self) -> bool {
        self.slot_id.is_some()
    }

    pub const fn slot_id(&self) -> Option<EffectSlotId> {
        self.slot_id
    }

    pub const fn scalar_count(&self) -> usize {
        self.scalars.len()
    }

    pub fn scalars(&self) -> &[f32] {
        &self.scalars[..self.scalar_count()]
    }

    pub fn scalar(&self, index: usize) -> Option<f32> {
        self.scalars().get(index).copied()
    }

    pub fn storage(&self) -> &[f32] {
        &self.scalars
    }
}

impl Default for RtPostEffectParameters {
    fn default() -> Self {
        Self::EMPTY
    }
}

impl Serialize for RtPostEffectParameters {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct SerializableEffectParameters<'a> {
            active: bool,
            slot_id: Option<EffectSlotId>,
            scalar_count: usize,
            scalars: &'a [f32],
        }
        SerializableEffectParameters {
            active: self.is_active(),
            slot_id: self.slot_id(),
            scalar_count: self.scalar_count(),
            scalars: self.scalars(),
        }
        .serialize(serializer)
    }
}

/// Fixed live values for one bus return: the occupying instance's
/// descriptor-ordered scalars plus the return-owned output level.
///
/// Modelled directly on [`RtPostEffectParameters`] because a return plays the
/// same role as a Patch effect slot: zero or one prepared registry instance
/// whose live scalars cross the real-time boundary each block. The one
/// addition is the return-owned level, which scales the wet contribution and
/// survives occupancy changes on the domain side.
#[derive(Clone, Debug, PartialEq)]
pub struct RtBusReturnParameters {
    effect: RtPostEffectParameters,
    tail: Vec<RtPostEffectParameters>,
    return_level: f32,
}

impl RtBusReturnParameters {
    /// The canonical unoccupied return entry.
    pub const EMPTY: Self = Self {
        effect: RtPostEffectParameters::EMPTY,
        tail: Vec::new(),
        return_level: 0.0,
    };

    /// Copies one occupied return's descriptor-ordered scalars and level.
    ///
    /// Scalar validation reuses the effect-projection error paths. The return
    /// level is validated as the entry's final scalar-position value: a
    /// non-finite level reports `NonFiniteEffectScalar` with the index one
    /// past the last declared scalar.
    pub fn new(
        slot_id: EffectSlotId,
        scalars: &[f32],
        return_level: f32,
    ) -> Result<Self, ParameterSnapshotError> {
        let effect = RtPostEffectParameters::new(slot_id, scalars)?;
        if !return_level.is_finite() {
            return Err(ParameterSnapshotError::NonFiniteEffectScalar {
                index: scalars.len(),
            });
        }
        Ok(Self {
            effect,
            tail: Vec::new(),
            return_level,
        })
    }

    /// Builds a complete ordered chain off the audio thread.
    pub fn chain(
        mut effects: Vec<RtPostEffectParameters>,
        return_level: f32,
    ) -> Result<Self, ParameterSnapshotError> {
        if !return_level.is_finite() {
            return Err(ParameterSnapshotError::NonFiniteEffectScalar { index: 0 });
        }
        if effects.is_empty() {
            return Ok(Self::EMPTY);
        }
        let effect = effects.remove(0);
        Ok(Self {
            effect,
            tail: effects,
            return_level,
        })
    }

    pub fn effects(&self) -> impl Iterator<Item = &RtPostEffectParameters> {
        std::iter::once(&self.effect)
            .filter(|effect| effect.is_active())
            .chain(self.tail.iter())
    }

    pub fn effect_count(&self) -> usize {
        usize::from(self.is_active()) + self.tail.len()
    }

    pub const fn is_active(&self) -> bool {
        self.effect.is_active()
    }

    pub const fn slot_id(&self) -> Option<EffectSlotId> {
        self.effect.slot_id()
    }

    pub const fn scalar_count(&self) -> usize {
        self.effect.scalar_count()
    }

    pub fn scalars(&self) -> &[f32] {
        self.effect.scalars()
    }

    pub fn scalar(&self, index: usize) -> Option<f32> {
        self.effect.scalar(index)
    }

    /// Returns the inner effect-shaped view handed to the prepared instance.
    pub const fn effect_parameters(&self) -> &RtPostEffectParameters {
        &self.effect
    }

    pub const fn return_level(&self) -> f32 {
        self.return_level
    }
}

impl Default for RtBusReturnParameters {
    fn default() -> Self {
        Self::EMPTY
    }
}

impl Serialize for RtBusReturnParameters {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct SerializableBusReturnParameters<'a> {
            active: bool,
            slot_id: Option<EffectSlotId>,
            scalar_count: usize,
            scalars: &'a [f32],
            return_level: f32,
            tail: &'a [RtPostEffectParameters],
        }
        SerializableBusReturnParameters {
            active: self.is_active(),
            slot_id: self.slot_id(),
            scalar_count: self.scalar_count(),
            scalars: self.scalars(),
            return_level: self.return_level(),
            tail: &self.tail,
        }
        .serialize(serializer)
    }
}

/// The prepared audio parameters for one active Patch.
///
/// Scalar storage is allocated off callback and exchanged by ownership. An
/// absent identity marks unused ParameterSnapshot entries.
#[derive(Clone, Debug, PartialEq)]
pub struct RtPatchParameters {
    patch_id: Option<PatchId>,
    output: PatchOutput,
    sends: Vec<f32>,
    envelope: VoiceEnvelope,
    /// The Patch's canonical ceiling on simultaneously sounding notes, riding
    /// the latest-scalar transport as a plain bounded integer beside the
    /// envelope: it allocates nothing, borrows nothing, and carries no
    /// destructor; scalar storage is exchanged by ownership.
    ///
    /// It is enumerated in [`ParameterSnapshot::SERIALIZED_LEAF_DESCRIPTOR`]
    /// and mirrored in `StateTree` as `parameters.patches[].voiceLimit`. A
    /// canonical value that crosses the real-time boundary and changes what is
    /// audible has to be visible in the trace, or no measured proof can
    /// correlate a refused note with the limit that refused it.
    voice_limit: VoiceLimit,
    instrument: RtInstrumentParameters,
    effects: [RtPostEffectParameters; MAX_EFFECT_SLOTS],
}

impl RtPatchParameters {
    /// Copy a prepared sub-instrument's controls without changing storage ownership.
    pub(crate) fn copy_instrument_controls(
        &mut self,
        source: &Self,
        values: &[f32],
    ) -> Result<(), crate::synth::PreparedInstrumentError> {
        if self.patch_id != source.patch_id || self.instrument.values.len() != values.len() {
            return Err(crate::synth::PreparedInstrumentError::ScalarLayoutMismatch);
        }
        self.instrument.values.copy_from_slice(values);
        self.envelope = source.envelope;
        self.voice_limit = source.voice_limit;
        Ok(())
    }

    /// The limit an entry carries before a canonical Patch projection supplies
    /// one: the widest any installed engine declares, so an unprojected fixture
    /// entry never refuses a note the production path would have started. Every
    /// production entry comes from [`ParameterSnapshot::project_patches`] or
    /// [`ParameterSnapshot::project_patches_with_effects`], which copy the
    /// Patch's own canonical value.
    const UNPROJECTED_VOICE_LIMIT: VoiceLimit =
        VoiceLimit::seeded_from_ceiling(VoiceLimit::MAXIMUM);

    /// Copies one active Patch's identity and validated mixer parameters into a
    /// real-time-safe value.
    pub fn new(patch_id: PatchId, output: PatchOutput) -> Self {
        Self {
            patch_id: Some(patch_id),
            output,
            sends: vec![0.0; MAX_BUS_RETURNS],
            envelope: VoiceEnvelope::DEFAULT,
            voice_limit: Self::UNPROJECTED_VOICE_LIMIT,
            instrument: RtInstrumentParameters::EMPTY,
            effects: [const { RtPostEffectParameters::EMPTY }; MAX_EFFECT_SLOTS],
        }
    }

    /// Copies the full live projection for one active Patch.
    pub fn projected(
        patch_id: PatchId,
        output: PatchOutput,
        envelope: VoiceEnvelope,
        instrument: RtInstrumentParameters,
    ) -> Self {
        Self {
            patch_id: Some(patch_id),
            output,
            sends: vec![0.0; MAX_BUS_RETURNS],
            envelope,
            voice_limit: Self::UNPROJECTED_VOICE_LIMIT,
            instrument,
            effects: [const { RtPostEffectParameters::EMPTY }; MAX_EFFECT_SLOTS],
        }
    }

    /// Copies the full live projection including every ordered effect position.
    ///
    /// One entry per position: empty positions carry the canonical inactive
    /// value at their exact index, never compacted down.
    pub fn projected_with_effects(
        patch_id: PatchId,
        output: PatchOutput,
        envelope: VoiceEnvelope,
        instrument: RtInstrumentParameters,
        effects: [RtPostEffectParameters; MAX_EFFECT_SLOTS],
    ) -> Self {
        Self {
            patch_id: Some(patch_id),
            output,
            sends: vec![0.0; MAX_BUS_RETURNS],
            envelope,
            voice_limit: Self::UNPROJECTED_VOICE_LIMIT,
            instrument,
            effects,
        }
    }

    /// Carries one Patch's canonical voice limit on this entry.
    #[must_use]
    pub const fn with_voice_limit(mut self, voice_limit: VoiceLimit) -> Self {
        self.voice_limit = voice_limit;
        self
    }

    /// Returns whether this entry contains one active Patch.
    pub const fn is_active(&self) -> bool {
        self.patch_id.is_some()
    }

    /// Returns the active Patch identity, or None for unused storage.
    pub const fn patch_id(&self) -> Option<PatchId> {
        self.patch_id
    }

    /// Returns the Patch's copied, validated output route and trim.
    pub const fn output(&self) -> PatchOutput {
        self.output
    }

    /// Copies validated Patch send amounts on the control/preparation thread.
    pub fn with_sends(mut self, sends: &[f32]) -> Result<Self, ParameterSnapshotError> {
        if sends.len() > usize::from(BusId::MAX) + 1 {
            return Err(ParameterSnapshotError::InvalidSendCount);
        }
        if let Some(index) = sends.iter().position(|value| {
            !crate::mixer::mixer_track_parameters::BUS_SEND_DESCRIPTOR.contains(*value)
        }) {
            return Err(ParameterSnapshotError::InvalidSend { index });
        }
        self.sends = sends.to_vec();
        Ok(self)
    }

    pub fn sends(&self) -> &[f32] {
        &self.sends
    }

    pub const fn envelope(&self) -> &VoiceEnvelope {
        &self.envelope
    }

    /// Returns the Patch's ceiling on simultaneously sounding notes, as the
    /// callback reads it at note-on.
    pub const fn voice_limit(&self) -> VoiceLimit {
        self.voice_limit
    }

    pub const fn instrument(&self) -> &RtInstrumentParameters {
        &self.instrument
    }

    /// Returns every ordered effect position, one entry per slot index.
    pub const fn effects(&self) -> &[RtPostEffectParameters; MAX_EFFECT_SLOTS] {
        &self.effects
    }

    /// Returns the live entry at one exact slot position.
    pub fn effect(&self, slot: usize) -> Option<&RtPostEffectParameters> {
        self.effects.get(slot)
    }

    fn inactive() -> Self {
        Self {
            patch_id: None,
            output: PatchOutput::default(),
            sends: Vec::new(),
            envelope: VoiceEnvelope::DEFAULT,
            voice_limit: Self::UNPROJECTED_VOICE_LIMIT,
            instrument: RtInstrumentParameters::EMPTY,
            effects: [const { RtPostEffectParameters::EMPTY }; MAX_EFFECT_SLOTS],
        }
    }
}

impl Serialize for RtPatchParameters {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct SerializablePatchParameters<'a> {
            patch_id: Option<PatchId>,
            envelope: &'a VoiceEnvelope,
            voice_limit: VoiceLimit,
            instrument: &'a RtInstrumentParameters,
            effects: &'a [RtPostEffectParameters; MAX_EFFECT_SLOTS],
            output: PatchOutput,
            sends: &'a [f32],
        }

        SerializablePatchParameters {
            patch_id: self.patch_id(),
            envelope: self.envelope(),
            voice_limit: self.voice_limit(),
            instrument: self.instrument(),
            effects: self.effects(),
            output: self.output(),
            sends: self.sends(),
        }
        .serialize(serializer)
    }
}

/// The reason a complete real-time parameter snapshot could not be built.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParameterSnapshotError {
    /// The control state contains more Patch values than the fixed capacity.
    TooManyPatches {
        count: usize,
        capacity: usize,
    },
    /// An inactive value was supplied inside the active Patch prefix.
    InactivePatch {
        index: usize,
    },
    /// A scalar could not be represented as a finite real-time value.
    NonFiniteInstrumentScalar {
        index: usize,
    },
    /// An effect scalar could not be represented as a finite real-time value.
    NonFiniteEffectScalar {
        index: usize,
    },
    /// A Patch config did not resolve through the immutable registry.
    InvalidInstrumentConfig {
        index: usize,
    },
    /// A Patch effect config did not resolve through the immutable effect registry.
    InvalidEffectConfig {
        index: usize,
    },
    InvalidSend {
        index: usize,
    },
    InvalidSendCount,
}

impl fmt::Display for ParameterSnapshotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::TooManyPatches { count, capacity } => write!(
                formatter,
                "parameter snapshot has {count} patches; maximum is {capacity}"
            ),
            Self::InactivePatch { index } => {
                write!(formatter, "parameter snapshot patch {index} is inactive")
            }
            Self::NonFiniteInstrumentScalar { index } => {
                write!(formatter, "instrument Scalar {index} is not finite")
            }
            Self::NonFiniteEffectScalar { index } => {
                write!(formatter, "effect Scalar {index} is not finite")
            }
            Self::InvalidInstrumentConfig { index } => {
                write!(formatter, "Patch {index} has an invalid instrument config")
            }
            Self::InvalidEffectConfig { index } => {
                write!(formatter, "Patch {index} has an invalid effect config")
            }
            Self::InvalidSend { index } => {
                write!(formatter, "send {index} must be finite and in 0..=1")
            }
            Self::InvalidSendCount => formatter.write_str("send count exceeds bus identity range"),
        }
    }
}

impl std::error::Error for ParameterSnapshotError {}

/// The newest complete control state required for rendering.
///
/// Storage is allocated on the control thread. Audio-thread readers exchange
/// complete ownership through the triple buffer; superseded storage returns to
/// the producer or graph retirement without allocation or destruction on audio.
#[derive(Clone, Debug, PartialEq)]
pub struct ParameterSnapshot {
    generation: u64,
    graph_revision: GraphRevision,
    global: GlobalParameters,
    mixer_tracks: [MixerTrackParameters; MixerTrackId::COUNT],
    returns: Vec<RtBusReturnParameters>,
    patch_count: usize,
    patches: [RtPatchParameters; MAX_ACTIVE_PATCHES],
}

impl ParameterSnapshot {
    pub const SERIALIZED_LEAF_DESCRIPTOR: &'static [&'static str] = &[
        "generation",
        "graphRevision",
        "patchCount",
        "patches[].patchId",
        "patches[].envelope.attackMilliseconds",
        "patches[].envelope.decayMilliseconds",
        "patches[].envelope.sustain",
        "patches[].envelope.releaseMilliseconds",
        "patches[].voiceLimit",
        "patches[].instrument.count",
        "patches[].instrument.values[]",
        "patches[].effects[].active",
        "patches[].effects[].slotId",
        "patches[].effects[].scalarCount",
        "patches[].effects[].scalars[]",
        "patches[].output.trackId",
        "patches[].output.trimGainDb",
        "patches[].sends[]",
        "mixerTracks[].levelDb",
        "mixerTracks[].pan",
        "mixerTracks[].mute",
        "mixerTracks[].solo",
        "mixerTracks[].sends[]",
        "returns[].active",
        "returns[].slotId",
        "returns[].scalarCount",
        "returns[].scalars[]",
        "returns[].returnLevel",
        "returns[].tail[].active",
        "returns[].tail[].slotId",
        "returns[].tail[].scalarCount",
        "returns[].tail[].scalars[]",
        "global.masterGainDb",
    ];

    /// Returns the canonical control-side serialization surface mirrored in StateTree.
    pub const fn serialized_leaf_descriptor() -> &'static [&'static str] {
        Self::SERIALIZED_LEAF_DESCRIPTOR
    }

    /// Copies a complete accepted control projection into bounded storage.
    ///
    /// Patch values in the input slice must all be active. Remaining array
    /// entries are initialized to the canonical inactive value.
    pub fn new(
        generation: u64,
        global: GlobalParameters,
        mixer: MixerState,
        patches: &[RtPatchParameters],
    ) -> Result<Self, ParameterSnapshotError> {
        Self::for_graph(generation, GraphRevision::INITIAL, global, mixer, patches)
    }

    /// Copies one complete projection for a specific prepared graph revision.
    ///
    /// The eight bus-return entries are all inactive: live return values come
    /// only from a projection of the canonical `BusReturnBank` supplied
    /// through [`Self::for_graph_with_returns`] or
    /// [`Self::project_returns`], never derived from global state.
    pub fn for_graph(
        generation: u64,
        graph_revision: GraphRevision,
        global: GlobalParameters,
        mixer: MixerState,
        patches: &[RtPatchParameters],
    ) -> Result<Self, ParameterSnapshotError> {
        Self::for_graph_with_returns(
            generation,
            graph_revision,
            global,
            mixer,
            patches,
            [const { RtBusReturnParameters::EMPTY }; MAX_BUS_RETURNS],
        )
    }

    /// Copies one complete projection carrying an explicit return bank.
    pub fn for_graph_with_returns(
        generation: u64,
        graph_revision: GraphRevision,
        global: GlobalParameters,
        mixer: MixerState,
        patches: &[RtPatchParameters],
        returns: impl Into<Vec<RtBusReturnParameters>>,
    ) -> Result<Self, ParameterSnapshotError> {
        if patches.len() > MAX_ACTIVE_PATCHES {
            return Err(ParameterSnapshotError::TooManyPatches {
                count: patches.len(),
                capacity: MAX_ACTIVE_PATCHES,
            });
        }
        if let Some(index) = patches.iter().position(|patch| !patch.is_active()) {
            return Err(ParameterSnapshotError::InactivePatch { index });
        }

        let mut storage = std::array::from_fn(|_| RtPatchParameters::inactive());
        storage[..patches.len()].clone_from_slice(patches);

        Ok(Self {
            generation,
            graph_revision,
            global,
            mixer_tracks: mixer.tracks().clone(),
            returns: returns.into(),
            patch_count: patches.len(),
            patches: storage,
        })
    }

    /// Replaces the eight live return entries on an already-complete snapshot.
    #[must_use]
    pub fn with_returns(mut self, returns: impl Into<Vec<RtBusReturnParameters>>) -> Self {
        self.returns = returns.into();
        self
    }

    /// Projects the canonical bus-return bank into the eight fixed live
    /// entries: each occupied return contributes its instance identity, its
    /// descriptor-ordered scalar values, and the return-owned level; each
    /// unoccupied return stays the canonical inactive value at its exact
    /// index.
    ///
    /// This is the one derivation of live return values. The prepared graph
    /// builder installs occupancy from the same bank, so the snapshot and the
    /// prepared return rack cannot drift.
    pub fn project_returns(
        effect_registry: &EffectCapabilityRegistry,
        bank: &crate::mixer::bus_return::BusReturnBank,
    ) -> Result<Vec<RtBusReturnParameters>, ParameterSnapshotError> {
        bank.returns()
            .iter()
            .map(|bus_return| {
                let index = bus_return.id().index();
                let effects = bus_return
                    .effects()
                    .iter()
                    .map(|config| {
                        let descriptor = effect_registry
                            .descriptor(config.capability_id())
                            .ok_or(ParameterSnapshotError::InvalidEffectConfig { index })?;
                        let scalars = descriptor
                            .scalar_parameters()
                            .map(|spec| {
                                let value = config
                                    .value(spec.id())
                                    .ok_or(ParameterSnapshotError::InvalidEffectConfig { index })?;
                                spec.scalar_value(value).map_err(|_| {
                                    ParameterSnapshotError::InvalidEffectConfig { index }
                                })
                            })
                            .collect::<Result<Vec<_>, _>>()?;
                        RtPostEffectParameters::new(config.slot_id(), &scalars)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                RtBusReturnParameters::chain(effects, bus_return.return_level())
            })
            .collect()
    }

    /// Projects the canonical Patch/config values into the one fixed real-time shape.
    ///
    /// Control state projection and off-callback graph preparation share this
    /// implementation so descriptor ordering and Scalar encoding cannot drift.
    pub fn project_patches(
        generation: u64,
        graph_revision: GraphRevision,
        global: GlobalParameters,
        mixer: MixerState,
        patches: &[Patch],
        registry: &CapabilityRegistry,
    ) -> Result<Self, ParameterSnapshotError> {
        if patches.len() > MAX_ACTIVE_PATCHES {
            return Err(ParameterSnapshotError::TooManyPatches {
                count: patches.len(),
                capacity: MAX_ACTIVE_PATCHES,
            });
        }

        let projected = patches
            .iter()
            .enumerate()
            .map(|(index, patch)| {
                registry
                    .validate_config(patch.instrument_config())
                    .map_err(|_| ParameterSnapshotError::InvalidInstrumentConfig { index })?;
                let descriptor = registry
                    .descriptor_for_config(patch.instrument_config())
                    .ok_or(ParameterSnapshotError::InvalidInstrumentConfig { index })?;
                let values = descriptor
                    .scalar_parameters()
                    .map(|spec| {
                        let value = patch
                            .instrument_config()
                            .value(spec.id())
                            .ok_or(ParameterSnapshotError::InvalidInstrumentConfig { index })?;
                        spec.scalar_value(value)
                            .map_err(|_| ParameterSnapshotError::InvalidInstrumentConfig { index })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let instrument = RtInstrumentParameters::new(&values)?;
                RtPatchParameters::projected(
                    patch.id(),
                    patch.output(),
                    *patch.envelope(),
                    instrument,
                )
                .with_voice_limit(patch.voice_limit())
                .with_sends(patch.sends())
            })
            .collect::<Result<Vec<_>, ParameterSnapshotError>>()?;

        Self::for_graph(generation, graph_revision, global, mixer, &projected)
    }

    /// Projects instrument and post-effect values into their separate fixed layouts.
    pub fn project_patches_with_effects(
        generation: u64,
        graph_revision: GraphRevision,
        global: GlobalParameters,
        mixer: MixerState,
        patches: &[Patch],
        registry: &CapabilityRegistry,
        effect_registry: &EffectCapabilityRegistry,
    ) -> Result<Self, ParameterSnapshotError> {
        if patches.len() > MAX_ACTIVE_PATCHES {
            return Err(ParameterSnapshotError::TooManyPatches {
                count: patches.len(),
                capacity: MAX_ACTIVE_PATCHES,
            });
        }
        let projected = patches
            .iter()
            .enumerate()
            .map(|(index, patch)| {
                registry
                    .validate_config(patch.instrument_config())
                    .map_err(|_| ParameterSnapshotError::InvalidInstrumentConfig { index })?;
                let descriptor = registry
                    .descriptor_for_config(patch.instrument_config())
                    .ok_or(ParameterSnapshotError::InvalidInstrumentConfig { index })?;
                let instrument_values = descriptor
                    .scalar_parameters()
                    .map(|spec| {
                        let value = patch
                            .instrument_config()
                            .value(spec.id())
                            .ok_or(ParameterSnapshotError::InvalidInstrumentConfig { index })?;
                        spec.scalar_value(value)
                            .map_err(|_| ParameterSnapshotError::InvalidInstrumentConfig { index })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let instrument = RtInstrumentParameters::new(&instrument_values)?;
                let mut effects = [const { RtPostEffectParameters::EMPTY }; MAX_EFFECT_SLOTS];
                let slots = patch.effect_slots();
                for (position, occupancy) in slots.iter().enumerate() {
                    let Some(config) = occupancy else {
                        continue;
                    };
                    // Per-position feed validation: every occupied position
                    // must hold a canonical registry config, and an instance
                    // identity may occupy only one position. The bounded slot
                    // array makes a capacity violation unrepresentable.
                    if slots[..position]
                        .iter()
                        .flatten()
                        .any(|prior| prior.slot_id() == config.slot_id())
                    {
                        return Err(ParameterSnapshotError::InvalidEffectConfig { index });
                    }
                    effect_registry
                        .validate_config(config)
                        .map_err(|_| ParameterSnapshotError::InvalidEffectConfig { index })?;
                    let effect_descriptor = effect_registry
                        .descriptor(config.capability_id())
                        .ok_or(ParameterSnapshotError::InvalidEffectConfig { index })?;
                    let effect_values = effect_descriptor
                        .scalar_parameters()
                        .map(|spec| {
                            let value = config
                                .value(spec.id())
                                .ok_or(ParameterSnapshotError::InvalidEffectConfig { index })?;
                            spec.scalar_value(value)
                                .map_err(|_| ParameterSnapshotError::InvalidEffectConfig { index })
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    effects[position] =
                        RtPostEffectParameters::new(config.slot_id(), &effect_values)?;
                }
                RtPatchParameters::projected_with_effects(
                    patch.id(),
                    patch.output(),
                    *patch.envelope(),
                    instrument,
                    effects,
                )
                .with_voice_limit(patch.voice_limit())
                .with_sends(patch.sends())
            })
            .collect::<Result<Vec<_>, ParameterSnapshotError>>()?;
        Self::for_graph(generation, graph_revision, global, mixer, &projected)
    }

    /// Projects Patch, effect, and bus-return values in one complete pass.
    ///
    /// This is the canonical production projection: control state projection
    /// and off-callback graph preparation both derive the eight live return
    /// entries from the same `BusReturnBank` that drives prepared occupancy.
    #[allow(clippy::too_many_arguments)]
    pub fn project_patches_with_effects_and_returns(
        generation: u64,
        graph_revision: GraphRevision,
        global: GlobalParameters,
        mixer: MixerState,
        patches: &[Patch],
        registry: &CapabilityRegistry,
        effect_registry: &EffectCapabilityRegistry,
        bank: &crate::mixer::bus_return::BusReturnBank,
    ) -> Result<Self, ParameterSnapshotError> {
        let returns = Self::project_returns(effect_registry, bank)?;
        Ok(Self::project_patches_with_effects(
            generation,
            graph_revision,
            global,
            mixer,
            patches,
            registry,
            effect_registry,
        )?
        .with_returns(returns))
    }

    /// Returns the AppState generation from which this snapshot was projected.
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Returns the prepared graph revision targeted by this snapshot.
    pub const fn graph_revision(&self) -> GraphRevision {
        self.graph_revision
    }

    /// Returns the copied parameters for the one shared global mix.
    pub const fn global(&self) -> &GlobalParameters {
        &self.global
    }

    /// Returns all fixed track values in `MixerTrackId` order.
    pub const fn mixer_tracks(&self) -> &[MixerTrackParameters; MixerTrackId::COUNT] {
        &self.mixer_tracks
    }

    pub const fn mixer_track(&self, id: MixerTrackId) -> &MixerTrackParameters {
        &self.mixer_tracks[id.index()]
    }

    /// Returns all eight live bus-return entries in ascending `BusId` order.
    pub fn returns(&self) -> &[RtBusReturnParameters] {
        &self.returns
    }

    /// Returns the live entry for one bus return.
    pub fn bus_return(&self, bus: BusId) -> &RtBusReturnParameters {
        &self.returns[bus.index()]
    }

    /// Returns the number of active entries in the fixed Patch array.
    pub const fn patch_count(&self) -> usize {
        self.patch_count
    }

    /// Returns exactly the active Patch parameter prefix.
    pub fn patches(&self) -> &[RtPatchParameters] {
        &self.patches[..self.patch_count]
    }

    /// Returns the complete fixed storage, including inactive entries.
    pub const fn storage(&self) -> &[RtPatchParameters; MAX_ACTIVE_PATCHES] {
        &self.patches
    }

    /// Compares only data observed by the audio thread, excluding the
    /// control-generation correlation carried by this immutable projection.
    pub fn audio_values_equal(&self, other: &Self) -> bool {
        self.graph_revision == other.graph_revision
            && self.global == other.global
            && self.mixer_tracks == other.mixer_tracks
            && self.returns == other.returns
            && self.patch_count == other.patch_count
            && self.patches == other.patches
    }

    /// Finds one active Patch without allocation.
    pub fn patch(&self, patch_id: PatchId) -> Option<&RtPatchParameters> {
        self.patches()
            .iter()
            .find(|patch| patch.patch_id() == Some(patch_id))
    }

    /// Returns whether this complete snapshot targets an exact graph revision
    /// and ordered Patch layout.
    pub fn is_compatible(
        &self,
        graph_revision: GraphRevision,
        ordered_patch_ids: &[PatchId],
    ) -> bool {
        self.graph_revision == graph_revision
            && self.patch_count == ordered_patch_ids.len()
            && self
                .patches()
                .iter()
                .zip(ordered_patch_ids)
                .all(|(patch, patch_id)| patch.patch_id() == Some(*patch_id))
    }

    /// Reuses identical bounded parameter values for a MIDI-only generation.
    pub(crate) const fn with_generation(mut self, generation: u64) -> Self {
        self.update_generation(generation);
        self
    }

    /// Advances control-owned metadata without copying unchanged scalar storage.
    pub(crate) const fn update_generation(&mut self, generation: u64) {
        self.generation = generation;
    }
}

impl Serialize for ParameterSnapshot {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        /// The retired reverb/delay fields are no longer serialized: their
        /// values cross the boundary as the generic indexed `returns` entries.
        /// Master gain is the one genuinely global leaf.
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct SerializableGlobalParameters {
            master_gain_db: f32,
        }

        impl From<&GlobalParameters> for SerializableGlobalParameters {
            fn from(parameters: &GlobalParameters) -> Self {
                Self {
                    master_gain_db: parameters.master_gain_db(),
                }
            }
        }

        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct SerializableParameterSnapshot<'a> {
            generation: u64,
            graph_revision: GraphRevision,
            patch_count: usize,
            patches: &'a [RtPatchParameters],
            mixer_tracks: &'a [MixerTrackParameters; MixerTrackId::COUNT],
            returns: &'a [RtBusReturnParameters],
            global: SerializableGlobalParameters,
        }

        SerializableParameterSnapshot {
            generation: self.generation(),
            graph_revision: self.graph_revision(),
            patch_count: self.patch_count(),
            patches: self.patches(),
            mixer_tracks: self.mixer_tracks(),
            returns: self.returns(),
            global: SerializableGlobalParameters::from(self.global()),
        }
        .serialize(serializer)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ParameterSnapshot, ParameterSnapshotError, RtBusReturnParameters, RtInstrumentParameters,
        RtPatchParameters, RtPostEffectParameters, MAX_ACTIVE_PATCHES,
    };
    use crate::kernel::patch_id::PatchId;
    use crate::mixer::bus_id::{BusId, MAX_BUS_RETURNS};
    use crate::mixer::global_parameters::GlobalParameters;
    use crate::mixer::mixer_state::MixerState;
    use crate::mixer::mixer_track_id::MixerTrackId;
    use crate::mixer::mixer_track_parameters::MixerTrackParameters;
    use crate::mixer::patch_output::PatchOutput;
    use crate::real_time::graph_revision::GraphRevision;
    use crate::synth::effect_slot_id::MAX_EFFECT_SLOTS;
    use crate::synth::instrument_capability_provider::InstrumentCapabilityProvider;
    use crate::synth::voice_envelope::VoiceEnvelope;
    use crate::synth::EffectSlotId;
    const EFFECT_SCALAR_FIXTURE_COUNT: usize = 31;
    use serde_json::Value;
    use std::collections::BTreeSet;

    fn global() -> GlobalParameters {
        GlobalParameters::new(-3.0).unwrap()
    }

    fn patch(id: u32, gain_db: f32) -> RtPatchParameters {
        RtPatchParameters::new(
            PatchId::new(id).unwrap(),
            PatchOutput::new(MixerTrackId::new(((id - 1) % 16) as u8).unwrap(), gain_db).unwrap(),
        )
    }

    #[test]
    fn copies_one_complete_accepted_control_projection() {
        let patches = [patch(1, -6.0), patch(2, -12.0)];
        let revision = GraphRevision::new(7).unwrap();
        let snapshot =
            ParameterSnapshot::for_graph(42, revision, global(), MixerState::default(), &patches)
                .unwrap();

        assert_eq!(snapshot.generation(), 42);
        assert_eq!(snapshot.graph_revision(), revision);
        assert_eq!(snapshot.global(), &global());
        assert_eq!(snapshot.patch_count(), 2);
        assert_eq!(snapshot.patches(), &patches);
        assert_eq!(snapshot.patch(PatchId::new(2).unwrap()), Some(&patches[1]));
    }

    #[test]
    fn patch_projection_carries_bounded_envelope_and_descriptor_ordered_scalars() {
        let envelope = VoiceEnvelope::new(5.0, 120.0, 0.6, 450.0).unwrap();
        let instrument = RtInstrumentParameters::new(&[3.0, 0.25, 0.75]).unwrap();
        let projected = RtPatchParameters::projected(
            PatchId::new(7).unwrap(),
            PatchOutput::default(),
            envelope,
            instrument,
        );

        assert_eq!(projected.envelope(), &envelope);
        assert_eq!(projected.instrument().count(), 3);
        assert_eq!(projected.instrument().values(), &[3.0, 0.25, 0.75]);
        assert!(projected.instrument().storage()[3..]
            .iter()
            .all(|value| *value == 0.0));
        assert_eq!(
            RtInstrumentParameters::new(&[0.0; 257]).unwrap().count(),
            257
        );
        assert_eq!(
            RtInstrumentParameters::new(&[f32::NAN]),
            Err(ParameterSnapshotError::NonFiniteInstrumentScalar { index: 0 })
        );
    }

    #[test]
    fn compatibility_requires_revision_count_and_exact_patch_order() {
        let revision = GraphRevision::new(8).unwrap();
        let snapshot = ParameterSnapshot::for_graph(
            42,
            revision,
            global(),
            MixerState::default(),
            &[patch(1, -6.0), patch(2, -12.0)],
        )
        .unwrap();

        assert!(snapshot.is_compatible(
            revision,
            &[PatchId::new(1).unwrap(), PatchId::new(2).unwrap()]
        ));
        assert!(!snapshot.is_compatible(
            GraphRevision::new(9).unwrap(),
            &[PatchId::new(1).unwrap(), PatchId::new(2).unwrap()]
        ));
        assert!(!snapshot.is_compatible(revision, &[PatchId::new(1).unwrap()]));
        assert!(!snapshot.is_compatible(
            revision,
            &[PatchId::new(2).unwrap(), PatchId::new(1).unwrap()]
        ));
    }

    #[test]
    fn serialized_leaf_descriptor_exactly_matches_the_active_projection() {
        fn leaves(value: &Value, prefix: &str, output: &mut BTreeSet<String>) {
            match value {
                Value::Object(object) => {
                    for (name, child) in object {
                        let path = if prefix.is_empty() {
                            name.to_owned()
                        } else {
                            format!("{prefix}.{name}")
                        };
                        leaves(child, &path, output);
                    }
                }
                Value::Array(array) => {
                    for child in array {
                        leaves(child, &format!("{prefix}[]"), output);
                    }
                }
                _ => {
                    output.insert(prefix.to_owned());
                }
            }
        }

        let mut effects = [const { RtPostEffectParameters::EMPTY }; MAX_EFFECT_SLOTS];
        effects[1] =
            RtPostEffectParameters::new(EffectSlotId::new(1).unwrap(), &[0.5, 0.75]).unwrap();
        let projected = RtPatchParameters::projected_with_effects(
            PatchId::new(7).unwrap(),
            PatchOutput::new(MixerTrackId::new(7).unwrap(), -3.0).unwrap(),
            VoiceEnvelope::new(12.0, 34.0, 0.56, 78.0).unwrap(),
            RtInstrumentParameters::new(&[2.0, 0.35, 0.65]).unwrap(),
            effects,
        );
        let mut returns = [const { RtBusReturnParameters::EMPTY }; MAX_BUS_RETURNS];
        returns[2] = RtBusReturnParameters::chain(
            vec![
                RtPostEffectParameters::new(EffectSlotId::new(3).unwrap(), &[0.25, 0.5]).unwrap(),
                RtPostEffectParameters::new(EffectSlotId::new(4).unwrap(), &[0.125]).unwrap(),
            ],
            0.75,
        )
        .unwrap();
        let snapshot = ParameterSnapshot::for_graph(
            9,
            GraphRevision::new(4).unwrap(),
            global(),
            MixerState::default(),
            &[projected],
        )
        .unwrap()
        .with_returns(returns);
        let mut discovered = BTreeSet::new();
        leaves(
            &serde_json::to_value(snapshot).unwrap(),
            "",
            &mut discovered,
        );
        let descriptor = ParameterSnapshot::serialized_leaf_descriptor();
        let described = descriptor
            .iter()
            .map(|path| (*path).to_owned())
            .collect::<BTreeSet<_>>();

        assert_eq!(descriptor.len(), described.len());
        assert_eq!(described, discovered);
    }

    #[test]
    fn unused_fixed_entries_are_inactive() {
        let snapshot =
            ParameterSnapshot::new(1, global(), MixerState::default(), &[patch(1, 0.0)]).unwrap();

        assert!(snapshot.storage()[0].is_active());
        assert!(snapshot.storage()[1..]
            .iter()
            .all(|entry| !entry.is_active()));
    }

    #[test]
    fn rejects_state_larger_than_the_compile_time_bound() {
        let patches = std::array::from_fn::<_, { MAX_ACTIVE_PATCHES + 1 }, _>(|_| patch(1, 0.0));
        let error =
            ParameterSnapshot::new(1, global(), MixerState::default(), &patches).unwrap_err();

        assert_eq!(
            error,
            ParameterSnapshotError::TooManyPatches {
                count: MAX_ACTIVE_PATCHES + 1,
                capacity: MAX_ACTIVE_PATCHES
            }
        );
    }

    #[test]
    fn scalar_storage_accepts_complete_large_schemas() {
        let values = (0..257).map(|i| i as f32).collect::<Vec<_>>();
        let instrument = RtInstrumentParameters::new(&values).unwrap();
        let effect = RtPostEffectParameters::new(EffectSlotId::new(1).unwrap(), &values).unwrap();
        assert_eq!(instrument.values(), values);
        assert_eq!(effect.scalars(), values);
        assert!(RtInstrumentParameters::new(&[f32::NAN]).is_err());
        assert!(
            RtPostEffectParameters::new(EffectSlotId::new(1).unwrap(), &[f32::INFINITY]).is_err()
        );
    }

    /// The limit is a Patch-owned canonical value that crosses the boundary
    /// intact, per Patch — not one number applied to the whole bank.
    #[test]
    fn the_snapshot_carries_each_patch_its_own_canonical_limit() {
        use crate::adapter::braids_capability::{BraidsCapability, BRAIDS_CAPABILITY_ID};
        use crate::synth::instrument_capability::CapabilityRegistry;
        use crate::synth::patch::Patch;
        use crate::synth::voice_limit::VoiceLimit;

        let provider = BraidsCapability::new().unwrap();
        let registry = CapabilityRegistry::new(vec![provider.descriptor()]).unwrap();
        let patch = |id: u32, limit: u16| {
            let mut patch = Patch::new(
                PatchId::new(id).unwrap(),
                format!("Patch {id}"),
                provider.default_config().unwrap(),
                crate::kernel::midi_channel::MidiChannel::new((id - 1) as u8).unwrap(),
                PatchOutput::to_track(MixerTrackId::new((id - 1) as u8).unwrap()),
            );
            patch.set_voice_limit(limit).unwrap();
            patch
        };
        let patches = [patch(1, 5), patch(2, 41)];

        let snapshot = ParameterSnapshot::project_patches(
            7,
            GraphRevision::new(2).unwrap(),
            global(),
            MixerState::default(),
            &patches,
            &registry,
        )
        .unwrap();

        assert_eq!(
            snapshot.patches()[0].voice_limit(),
            VoiceLimit::new(5).unwrap()
        );
        assert_eq!(
            snapshot.patches()[1].voice_limit(),
            VoiceLimit::new(41).unwrap()
        );
        assert_eq!(provider.descriptor().id().as_str(), BRAIDS_CAPABILITY_ID);

        // The effect-carrying projection copies the same canonical value.
        let with_effects = ParameterSnapshot::project_patches_with_effects(
            8,
            GraphRevision::new(2).unwrap(),
            global(),
            MixerState::default(),
            &patches,
            &registry,
            &crate::synth::EffectCapabilityRegistry::new(Vec::new()).unwrap(),
        )
        .unwrap();
        assert_eq!(
            with_effects
                .patches()
                .iter()
                .map(|entry| entry.voice_limit().value())
                .collect::<Vec<_>>(),
            vec![5, 41]
        );
    }

    fn occupied_effects(seed: u32) -> [RtPostEffectParameters; MAX_EFFECT_SLOTS] {
        std::array::from_fn(|position| {
            let slot =
                EffectSlotId::new((seed * MAX_EFFECT_SLOTS as u32) as u16 + position as u16 + 1)
                    .unwrap();
            let scalars: Vec<f32> = (0..EFFECT_SCALAR_FIXTURE_COUNT)
                .map(|index| (seed as f32 + position as f32 + index as f32) * 0.01)
                .collect();
            RtPostEffectParameters::new(slot, &scalars).unwrap()
        })
    }

    /// T026: a fully occupied 16 x 3 snapshot constructs with every slot
    /// correctly initialized at its exact position — no construction site
    /// leaves a position at `EMPTY` by accident.
    #[test]
    fn fully_occupied_snapshot_initializes_every_slot_at_its_exact_position() {
        let patches: Vec<RtPatchParameters> = (1..=MAX_ACTIVE_PATCHES as u32)
            .map(|id| {
                RtPatchParameters::projected_with_effects(
                    PatchId::new(id).unwrap(),
                    PatchOutput::to_track(MixerTrackId::new(((id - 1) % 16) as u8).unwrap()),
                    VoiceEnvelope::DEFAULT,
                    RtInstrumentParameters::new(&[id as f32]).unwrap(),
                    occupied_effects(id),
                )
            })
            .collect();
        let snapshot =
            ParameterSnapshot::new(3, global(), MixerState::default(), &patches).unwrap();

        assert_eq!(snapshot.patch_count(), MAX_ACTIVE_PATCHES);
        for (index, patch) in snapshot.patches().iter().enumerate() {
            let expected = occupied_effects(index as u32 + 1);
            assert_eq!(patch.effects(), &expected);
            for position in 0..MAX_EFFECT_SLOTS {
                let entry = patch.effect(position).unwrap();
                assert!(entry.is_active(), "patch {index} position {position}");
                assert_eq!(entry.scalar_count(), EFFECT_SCALAR_FIXTURE_COUNT);
            }
            assert!(patch.effect(MAX_EFFECT_SLOTS).is_none());
        }
    }

    fn gapped_chain_patch() -> crate::synth::Patch {
        use crate::adapter::braids_capability::{BraidsCapability, BRAIDS_CAPABILITY_ID};
        use crate::adapter::production_effects::production_chorus_config;
        use crate::synth::effect_slot_id::EffectSlotIndex;
        use crate::synth::DescriptorDefaultConfigFactory;

        let braids = BraidsCapability::new().unwrap();
        let capabilities =
            crate::synth::instrument_capability::CapabilityRegistry::new(vec![braids.descriptor()])
                .unwrap();
        let factory = DescriptorDefaultConfigFactory::new(capabilities, vec![Box::new(braids)]);
        let mut patch = crate::synth::Patch::new(
            PatchId::new(1).unwrap(),
            "Gapped".to_owned(),
            factory
                .create(&crate::synth::CapabilityId::new(BRAIDS_CAPABILITY_ID).unwrap())
                .unwrap(),
            crate::kernel::midi_channel::MidiChannel::new(0).unwrap(),
            PatchOutput::default(),
        )
        .with_effect_slot(
            EffectSlotIndex::new(0).unwrap(),
            production_chorus_config(EffectSlotId::new(1).unwrap()).unwrap(),
        )
        .with_effect_slot(
            EffectSlotIndex::new(1).unwrap(),
            production_chorus_config(EffectSlotId::new(2).unwrap()).unwrap(),
        )
        .with_effect_slot(
            EffectSlotIndex::new(2).unwrap(),
            production_chorus_config(EffectSlotId::new(3).unwrap()).unwrap(),
        );
        patch
            .set_slot_occupancy(EffectSlotIndex::new(1).unwrap(), None)
            .unwrap();
        patch
    }

    /// A gapped Patch chain feeds the snapshot per-position: each occupied
    /// entry lands at its true slot index and the cleared middle position
    /// stays inactive at its own index — the feed never compacts.
    #[test]
    fn gapped_patch_chain_feeds_occupied_entries_at_true_positions() {
        let patch = gapped_chain_patch();
        let braids = crate::adapter::braids_capability::BraidsCapability::new().unwrap();
        let capabilities =
            crate::synth::instrument_capability::CapabilityRegistry::new(vec![braids.descriptor()])
                .unwrap();
        let effects = crate::adapter::production_effects::production_effect_registry().unwrap();

        let snapshot = ParameterSnapshot::project_patches_with_effects(
            5,
            GraphRevision::INITIAL,
            global(),
            MixerState::default(),
            std::slice::from_ref(&patch),
            &capabilities,
            &effects,
        )
        .unwrap();

        let projected = &snapshot.patches()[0];
        assert!(projected.effects()[0].is_active());
        assert_eq!(
            projected.effects()[0].slot_id(),
            Some(EffectSlotId::new(1).unwrap())
        );
        assert!(
            !projected.effects()[1].is_active(),
            "the cleared middle position must stay inactive, never compacted over"
        );
        assert_eq!(projected.effects()[1].slot_id(), None);
        assert!(projected.effects()[2].is_active());
        assert_eq!(
            projected.effects()[2].slot_id(),
            Some(EffectSlotId::new(3).unwrap())
        );
    }

    /// An instance identity can never occupy two positions of one Patch: the
    /// aggregate refuses the second occupancy outright, so the duplicate the
    /// feed used to catch cannot reach it. The feed's own refusal keeps its
    /// strength for what remains representable — an occupied position holding
    /// a config the effect registry does not recognise is still refused as an
    /// invalid effect configuration for that Patch.
    #[test]
    fn duplicate_instance_identity_is_refused_before_the_feed_sees_it() {
        use crate::adapter::braids_capability::{BraidsCapability, BRAIDS_CAPABILITY_ID};
        use crate::adapter::production_effects::production_chorus_config;
        use crate::synth::effect_slot_id::EffectSlotIndex;
        use crate::synth::patch::EffectSlotOccupancyError;
        use crate::synth::{DescriptorDefaultConfigFactory, EffectCapabilityId, PostEffectConfig};

        let braids = BraidsCapability::new().unwrap();
        let capabilities =
            crate::synth::instrument_capability::CapabilityRegistry::new(vec![braids.descriptor()])
                .unwrap();
        let factory =
            DescriptorDefaultConfigFactory::new(capabilities.clone(), vec![Box::new(braids)]);
        let effects = crate::adapter::production_effects::production_effect_registry().unwrap();
        let mut patch = crate::synth::Patch::new(
            PatchId::new(1).unwrap(),
            "Duplicated".to_owned(),
            factory
                .create(&crate::synth::CapabilityId::new(BRAIDS_CAPABILITY_ID).unwrap())
                .unwrap(),
            crate::kernel::midi_channel::MidiChannel::new(0).unwrap(),
            PatchOutput::default(),
        )
        .with_effect_slot(
            EffectSlotIndex::new(0).unwrap(),
            production_chorus_config(EffectSlotId::new(9).unwrap()).unwrap(),
        );

        assert_eq!(
            patch.set_slot_occupancy(
                EffectSlotIndex::new(1).unwrap(),
                Some(production_chorus_config(EffectSlotId::new(9).unwrap()).unwrap()),
            ),
            Err(EffectSlotOccupancyError::DuplicateSlotId(
                EffectSlotId::new(9).unwrap()
            ))
        );
        assert!(
            patch.effect_slots()[1].is_none(),
            "the refused duplicate never lands, and never replaces the original"
        );

        patch
            .set_slot_occupancy(
                EffectSlotIndex::new(1).unwrap(),
                Some(PostEffectConfig::from_parts(
                    EffectSlotId::new(10).unwrap(),
                    EffectCapabilityId::new("effect.unregistered").unwrap(),
                    Vec::new(),
                    Vec::new(),
                )),
            )
            .unwrap();

        assert_eq!(
            ParameterSnapshot::project_patches_with_effects(
                5,
                GraphRevision::INITIAL,
                global(),
                MixerState::default(),
                std::slice::from_ref(&patch),
                &capabilities,
                &effects,
            )
            .unwrap_err(),
            ParameterSnapshotError::InvalidEffectConfig { index: 0 }
        );
    }

    /// T028: the eight return entries derive only from the canonical
    /// `BusReturnBank` — an unsupplied bank projects all-inactive, and the
    /// production default occupancy projects reverb/delay at returns 0 and 1
    /// with their descriptor default scalars and levels.
    #[test]
    fn returns_project_only_from_the_canonical_bus_return_bank() {
        let snapshot = ParameterSnapshot::new(1, global(), MixerState::default(), &[]).unwrap();
        assert_eq!(snapshot.returns().len(), MAX_BUS_RETURNS);
        assert!(snapshot.returns().iter().all(|entry| !entry.is_active()));

        let registry = crate::adapter::production_effects::production_effect_registry().unwrap();
        let bank =
            crate::adapter::production_effects::production_default_bus_returns(&registry).unwrap();
        let returns = ParameterSnapshot::project_returns(&registry, &bank).unwrap();

        assert!(returns[0].is_active());
        assert_eq!(returns[0].slot_id(), Some(EffectSlotId::new(1).unwrap()));
        assert_eq!(returns[0].scalars(), &[0.5, 0.5]);
        assert_eq!(returns[0].return_level(), 0.5);
        assert!(returns[1].is_active());
        assert_eq!(returns[1].slot_id(), Some(EffectSlotId::new(2).unwrap()));
        assert_eq!(returns[1].scalars(), &[250.0, 0.5]);
        assert_eq!(returns[1].return_level(), 0.5);
        assert!(returns[2..].iter().all(|entry| !entry.is_active()));

        let projected = snapshot.with_returns(returns.clone());
        assert_eq!(projected.bus_return(BusId::new(1).unwrap()), &returns[1]);
    }

    /// T028: capacity and finiteness errors fire through the reused
    /// effect-projection error paths, including the return level convention.
    #[test]
    fn bus_return_projection_validates_finiteness_without_scalar_caps() {
        let slot = EffectSlotId::new(1).unwrap();

        assert_eq!(
            RtBusReturnParameters::new(slot, &[0.0; 257], 0.5)
                .unwrap()
                .scalar_count(),
            257
        );
        assert_eq!(
            RtBusReturnParameters::new(slot, &[0.5, f32::NAN], 0.5),
            Err(ParameterSnapshotError::NonFiniteEffectScalar { index: 1 })
        );
        // The return level is validated as the entry's final scalar-position
        // value: its index is the declared scalar count.
        assert_eq!(
            RtBusReturnParameters::new(slot, &[0.5, 0.25], f32::INFINITY),
            Err(ParameterSnapshotError::NonFiniteEffectScalar { index: 2 })
        );

        let entry = RtBusReturnParameters::new(slot, &[0.5, 0.25], 0.75).unwrap();
        assert!(entry.is_active());
        assert_eq!(entry.scalar_count(), 2);
        assert_eq!(entry.scalar(1), Some(0.25));
        assert_eq!(entry.effect_parameters().scalars(), &[0.5, 0.25]);
        assert!(!RtBusReturnParameters::EMPTY.is_active());
        assert_eq!(RtBusReturnParameters::EMPTY.return_level(), 0.0);
    }

    /// Explicit returns are the only source of live return entries, and they
    /// change exactly the audio-observed return values.
    #[test]
    fn explicit_returns_are_the_only_live_return_source() {
        let mut returns = [const { RtBusReturnParameters::EMPTY }; MAX_BUS_RETURNS];
        returns[5] =
            RtBusReturnParameters::new(EffectSlotId::new(6).unwrap(), &[0.1, 0.2, 0.3], 1.0)
                .unwrap();
        let bare = ParameterSnapshot::new(1, global(), MixerState::default(), &[]).unwrap();
        let explicit = ParameterSnapshot::for_graph_with_returns(
            1,
            GraphRevision::INITIAL,
            global(),
            MixerState::default(),
            &[],
            returns.clone(),
        )
        .unwrap();

        assert!(bare.returns().iter().all(|entry| !entry.is_active()));
        assert!(!explicit.returns()[0].is_active());
        assert!(explicit.returns()[5].is_active());
        assert_eq!(explicit.returns()[5].scalar_count(), 3);
        assert_eq!(
            bare.clone().with_returns(returns.clone()).returns(),
            &returns
        );
        assert!(!bare.audio_values_equal(&explicit));
    }

    /// All configured sends cross the boundary per track and are observable
    /// in `BusId` order.
    #[test]
    fn every_track_carries_all_default_indexed_sends() {
        let mut sends = [0.0; MAX_BUS_RETURNS];
        for (index, send) in sends.iter_mut().enumerate() {
            *send = index as f32 / (MAX_BUS_RETURNS - 1) as f32;
        }
        let track = MixerTrackParameters::from_values(0.0, 0.0, false, false, sends).unwrap();
        let snapshot = ParameterSnapshot::new(
            1,
            global(),
            MixerState::default().with_track(MixerTrackId::new(3).unwrap(), track),
            &[],
        )
        .unwrap();

        assert_eq!(
            snapshot.mixer_track(MixerTrackId::new(3).unwrap()).sends(),
            sends
        );
    }
}
