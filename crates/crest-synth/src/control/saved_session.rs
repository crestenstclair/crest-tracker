use crate::control::{
    AppEvent, AppState, SessionReplacementPayload, StateProjectionError, StateProjector,
};
use crate::kernel::{MidiChannel, PatchId};
use crate::mixer::bus_id::{BusId, DEFAULT_BUS_RETURNS};
use crate::mixer::bus_return::{BusReturn, BusReturnBank};
use crate::mixer::global_parameters::GlobalParameters;
use crate::mixer::mixer_state::MixerState;
use crate::mixer::mixer_track_id::MixerTrackId;
use crate::mixer::mixer_track_parameters::{MixerTrackParameters, BUS_SEND_DESCRIPTOR};
use crate::mixer::patch_output::PatchOutput;
use crate::real_time::{GraphPreparationError, GraphRevision, PreparedGraph, PreparedGraphBuilder};
use crate::synth::effect_slot_id::{EffectSlotIndex, MAX_EFFECT_SLOTS};
use crate::synth::{
    AssetAssignment, AssetReference, CapabilityRegistry, EffectCapabilityRegistry, EffectPreparer,
    InstrumentConfig, InstrumentPreparer, Patch, PostEffectConfig, VoiceEnvelope,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

pub const SAVED_SESSION_VERSION: u32 = 4;
pub const SOUND_DEFINITION_VERSION: u32 = 1;

/// Reusable sound content, independent of where a host installs or mixes it.
/// Identity and revision/digest belong to the host's sound library. Installation
/// still requires complete saved-session validation and graph preparation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SoundDefinition {
    version: u32,
    name: String,
    instrument: InstrumentConfig,
    envelope: VoiceEnvelope,
    effects: [Option<PostEffectConfig>; MAX_EFFECT_SLOTS],
    voice_limit: u16,
}

impl SoundDefinition {
    pub const fn version(&self) -> u32 {
        self.version
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn instrument(&self) -> &InstrumentConfig {
        &self.instrument
    }

    pub const fn envelope(&self) -> &VoiceEnvelope {
        &self.envelope
    }

    /// Ordered slots preserve empty positions as well as effect configuration.
    pub const fn effects(&self) -> &[Option<PostEffectConfig>; MAX_EFFECT_SLOTS] {
        &self.effects
    }

    pub const fn voice_limit(&self) -> u16 {
        self.voice_limit
    }

    /// Lists distinct instrument and effect references in first-use order.
    /// Inventory does not resolve locators or admit their contents.
    pub fn asset_references(&self) -> Vec<AssetReference> {
        unique_asset_references(sound_assets(&self.instrument, &self.effects))
    }

    /// Relinks every use of each original reference in one immutable candidate.
    /// Mappings apply simultaneously, preserving values and asset-scoped choices.
    /// See [`SavedSession::with_relinked_assets`] for validation responsibilities.
    pub fn with_relinked_assets(
        &self,
        replacements: &[(AssetReference, AssetReference)],
    ) -> Result<Self, AssetRelinkError> {
        let relinks = AssetRelinks::new(self.asset_references(), replacements)?;
        let mut candidate = self.clone();
        relinks.relink_sound(&mut candidate.instrument, &mut candidate.effects);
        Ok(candidate)
    }

    pub fn to_json(&self) -> Result<String, SoundDefinitionError> {
        self.validate_version()?;
        serde_json::to_string_pretty(self).map_err(|_| SoundDefinitionError::Encode)
    }

    /// Decodes portable content. Capability and asset admission happen during
    /// installation against the receiving Synth's registries and preparers.
    pub fn from_json(json: &str) -> Result<Self, SoundDefinitionError> {
        let definition: Self =
            serde_json::from_str(json).map_err(|_| SoundDefinitionError::Decode)?;
        definition.validate_version()?;
        Ok(definition)
    }

    fn validate_version(&self) -> Result<(), SoundDefinitionError> {
        if self.version != SOUND_DEFINITION_VERSION {
            return Err(SoundDefinitionError::UnsupportedVersion(self.version));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SoundDefinitionError {
    #[error("sound definition could not be encoded")]
    Encode,
    #[error("sound definition could not be decoded")]
    Decode,
    #[error("unsupported sound definition version {0}")]
    UnsupportedVersion(u32),
    #[error("sound definition target Patch {0:?} is not installed")]
    UnknownPatch(PatchId),
}

/// A rejected relink leaves its source document unchanged. These failures
/// describe the mapping, not filesystem availability or capability admission.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum AssetRelinkError {
    #[error("asset relink contains an invalid reference: {0:?}")]
    InvalidReference(AssetReference),
    #[error("asset relink repeats an original reference: {0:?}")]
    DuplicateOriginal(AssetReference),
    #[error("asset relink original reference is absent: {0:?}")]
    UnknownOriginal(AssetReference),
    #[error("asset relink changes asset kind from {original:?} to {replacement:?}")]
    KindMismatch {
        original: AssetReference,
        replacement: AssetReference,
    },
}

fn sound_assets<'a>(
    instrument: &'a InstrumentConfig,
    effects: &'a [Option<PostEffectConfig>; MAX_EFFECT_SLOTS],
) -> impl Iterator<Item = &'a AssetAssignment> {
    instrument.asset_references().iter().chain(
        effects
            .iter()
            .flatten()
            .flat_map(PostEffectConfig::asset_references),
    )
}

fn unique_asset_references<'a>(
    assignments: impl Iterator<Item = &'a AssetAssignment>,
) -> Vec<AssetReference> {
    let mut seen = HashSet::new();
    assignments
        .map(AssetAssignment::reference)
        .filter(|reference| seen.insert(*reference))
        .cloned()
        .collect()
}

/// Validated simultaneous mapping shared by session and reusable-sound edits.
struct AssetRelinks {
    replacements: HashMap<AssetReference, AssetReference>,
}

impl AssetRelinks {
    fn new(
        inventory: Vec<AssetReference>,
        replacements: &[(AssetReference, AssetReference)],
    ) -> Result<Self, AssetRelinkError> {
        let inventory: HashSet<_> = inventory.into_iter().collect();
        let mut mapped = HashMap::new();
        for (original, replacement) in replacements {
            Self::validate_reference(original)?;
            Self::validate_reference(replacement)?;
            if original.kind() != replacement.kind() {
                return Err(AssetRelinkError::KindMismatch {
                    original: original.clone(),
                    replacement: replacement.clone(),
                });
            }
            if !inventory.contains(original) {
                return Err(AssetRelinkError::UnknownOriginal(original.clone()));
            }
            if mapped
                .insert(original.clone(), replacement.clone())
                .is_some()
            {
                return Err(AssetRelinkError::DuplicateOriginal(original.clone()));
            }
        }
        Ok(Self {
            replacements: mapped,
        })
    }

    fn validate_reference(reference: &AssetReference) -> Result<(), AssetRelinkError> {
        // Serde can bypass constructors. AssetReference owns locator syntax;
        // catalog-specific schemes and filesystem policy stay with resolvers.
        AssetReference::new(reference.kind(), reference.locator())
            .map(|_| ())
            .map_err(|_| AssetRelinkError::InvalidReference(reference.clone()))
    }

    fn relink_assignments(&self, assignments: &[AssetAssignment]) -> Vec<AssetAssignment> {
        assignments
            .iter()
            .map(|assignment| {
                let replacement = self
                    .replacements
                    .get(assignment.reference())
                    .unwrap_or_else(|| assignment.reference());
                AssetAssignment::new(assignment.parameter_id().clone(), replacement.clone())
            })
            .collect()
    }

    fn relink_effect(&self, effect: &PostEffectConfig) -> PostEffectConfig {
        PostEffectConfig::from_parts(
            effect.slot_id(),
            effect.capability_id().clone(),
            effect.values().to_vec(),
            self.relink_assignments(effect.asset_references()),
        )
    }

    fn relink_sound(
        &self,
        instrument: &mut InstrumentConfig,
        effects: &mut [Option<PostEffectConfig>; MAX_EFFECT_SLOTS],
    ) {
        // Ordinary bank replacement resets scoped choices. Relocation must
        // retain them exactly for the receiving preparer's later validation.
        *instrument = InstrumentConfig::from_parts(
            instrument.capability_id().clone(),
            instrument.values().to_vec(),
            self.relink_assignments(instrument.asset_references()),
        );
        for effect in effects.iter_mut().flatten() {
            *effect = self.relink_effect(effect);
        }
    }
}

/// Versioned control-side session state. Runtime and interaction state are
/// absent by construction: no focus/modal/browser/request/preview, decoded
/// PCM, prepared graph, device value, or absolute library root has a field.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedSession {
    version: u32,
    #[serde(default = "default_patch_capacity")]
    patch_capacity: usize,
    patches: Vec<SavedPatch>,
    mixer: MixerState,
    master_gain_db: f32,
    returns: Vec<SavedReturn>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedPatch {
    id: u32,
    name: String,
    instrument: InstrumentConfig,
    channel: u8,
    envelope: VoiceEnvelope,
    output: PatchOutput,
    effects: [Option<PostEffectConfig>; MAX_EFFECT_SLOTS],
    voice_limit: u16,
    sends: Vec<f32>,
}

/// Prior schemas owned sends only on Mixer tracks.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedPatchV3 {
    id: u32,
    name: String,
    instrument: InstrumentConfig,
    channel: u8,
    envelope: VoiceEnvelope,
    output: PatchOutput,
    effects: [Option<PostEffectConfig>; MAX_EFFECT_SLOTS],
    voice_limit: u16,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedReturn {
    name: String,
    effects: Vec<PostEffectConfig>,
    return_level: f32,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedReturnV2 {
    effect: Option<PostEffectConfig>,
    return_level: f32,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedSessionV4 {
    version: u32,
    #[serde(default = "default_patch_capacity")]
    patch_capacity: usize,
    patches: Vec<SavedPatch>,
    mixer: MixerState,
    master_gain_db: f32,
    returns: Vec<SavedReturn>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedSessionV3 {
    patches: Vec<SavedPatchV3>,
    mixer: MixerState,
    master_gain_db: f32,
    returns: Vec<SavedReturn>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedSessionV2 {
    version: u32,
    patches: Vec<SavedPatchV3>,
    mixer: MixerState,
    master_gain_db: f32,
    returns: Vec<SavedReturnV2>,
}

/// Phase-6 predecessor: the same canonical values before Patch voice limit
/// became explicit persistence. Migration derives the limit from the saved
/// capability policy; it never guesses an asset or capability.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedSessionV1 {
    #[serde(default)]
    version: Option<u32>,
    patches: Vec<SavedPatchV1>,
    mixer: MixerState,
    master_gain_db: f32,
    returns: Vec<SavedReturnV2>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedPatchV1 {
    id: u32,
    name: String,
    instrument: InstrumentConfig,
    channel: u8,
    envelope: VoiceEnvelope,
    output: PatchOutput,
    effects: [Option<PostEffectConfig>; MAX_EFFECT_SLOTS],
}

fn default_patch_capacity() -> usize {
    crate::kernel::MAX_ACTIVE_PATCHES
}

impl SavedSession {
    pub fn capture(state: &AppState) -> Self {
        Self {
            version: SAVED_SESSION_VERSION,
            patch_capacity: state.patch_capacity(),
            patches: state
                .patches()
                .iter()
                .map(|patch| SavedPatch {
                    id: patch.id().value(),
                    name: patch.name().to_owned(),
                    instrument: patch.instrument_config().clone(),
                    channel: patch.channel().value(),
                    envelope: *patch.envelope(),
                    output: patch.output(),
                    effects: patch.effect_slots().clone(),
                    voice_limit: patch.voice_limit().value(),
                    sends: patch.sends().to_vec(),
                })
                .collect(),
            mixer: state.mixer().clone(),
            master_gain_db: state.global().master_gain_db(),
            returns: state
                .bus_returns()
                .returns()
                .iter()
                .map(|bus_return| SavedReturn {
                    name: bus_return.name().to_owned(),
                    effects: bus_return.effects().to_vec(),
                    return_level: bus_return.return_level(),
                })
                .collect(),
        }
    }

    pub const fn version(&self) -> u32 {
        self.version
    }

    /// Lists distinct references in Patch order (instrument, then effect slots),
    /// followed by return-chain order. Shared references appear only once.
    /// Inventory does not resolve locators or admit their contents.
    pub fn asset_references(&self) -> Vec<AssetReference> {
        let patches = self
            .patches
            .iter()
            .flat_map(|patch| sound_assets(&patch.instrument, &patch.effects));
        let returns = self
            .returns
            .iter()
            .flat_map(|bus_return| &bus_return.effects)
            .flat_map(PostEffectConfig::asset_references);
        unique_asset_references(patches.chain(returns))
    }

    /// Relinks every use of each original reference in one immutable candidate.
    /// Mappings apply simultaneously; duplicate or absent originals, invalid
    /// canonical references, and kind changes fail without modifying this value.
    /// Unmapped assets and all non-asset content remain exact, including scoped
    /// preset choices, effect identities/order, installation, routing, and mix.
    ///
    /// This checks `AssetReference` invariants only, preserving catalog-specific
    /// locator schemes. It performs no I/O and proves neither content identity
    /// nor path confinement. A collecting host/resolver must verify those;
    /// `prepare_restore` or the hosted lifecycle must admit all assets and exact
    /// choices before the candidate can replace the active session.
    pub fn with_relinked_assets(
        &self,
        replacements: &[(AssetReference, AssetReference)],
    ) -> Result<Self, AssetRelinkError> {
        let relinks = AssetRelinks::new(self.asset_references(), replacements)?;
        let mut candidate = self.clone();
        for patch in &mut candidate.patches {
            relinks.relink_sound(&mut patch.instrument, &mut patch.effects);
        }
        for effect in candidate
            .returns
            .iter_mut()
            .flat_map(|bus| &mut bus.effects)
        {
            *effect = relinks.relink_effect(effect);
        }
        Ok(candidate)
    }

    pub fn sound_definition(
        &self,
        patch_id: PatchId,
    ) -> Result<SoundDefinition, SoundDefinitionError> {
        let patch = self
            .patches
            .iter()
            .find(|patch| patch.id == patch_id.value())
            .ok_or(SoundDefinitionError::UnknownPatch(patch_id))?;
        Ok(SoundDefinition {
            version: SOUND_DEFINITION_VERSION,
            name: patch.name.clone(),
            instrument: patch.instrument.clone(),
            envelope: patch.envelope,
            effects: patch.effects.clone(),
            voice_limit: patch.voice_limit,
        })
    }

    /// Produces a candidate, preserving installation identity, channel, output,
    /// trim, sends, mixer, and returns. Call `prepare_restore` or the hosted
    /// lifecycle before committing it; copying content does not admit assets.
    pub fn with_sound_definition(
        &self,
        patch_id: PatchId,
        definition: SoundDefinition,
    ) -> Result<Self, SoundDefinitionError> {
        definition.validate_version()?;
        let index = self
            .patches
            .iter()
            .position(|patch| patch.id == patch_id.value())
            .ok_or(SoundDefinitionError::UnknownPatch(patch_id))?;
        let mut candidate = self.clone();
        let patch = &mut candidate.patches[index];
        patch.name = definition.name;
        patch.instrument = definition.instrument;
        patch.envelope = definition.envelope;
        patch.effects = definition.effects;
        patch.voice_limit = definition.voice_limit;
        Ok(candidate)
    }

    pub fn to_json(&self) -> Result<String, SavedSessionError> {
        serde_json::to_string_pretty(self).map_err(|_| SavedSessionError::Encode)
    }

    pub fn from_json(
        json: &str,
        capabilities: &CapabilityRegistry,
    ) -> Result<Self, SavedSessionError> {
        let value: serde_json::Value =
            serde_json::from_str(json).map_err(|_| SavedSessionError::Decode)?;
        let version = value.get("version").and_then(serde_json::Value::as_u64);
        match version {
            Some(version) if version == u64::from(SAVED_SESSION_VERSION) => {
                let decoded: SavedSessionV4 =
                    serde_json::from_value(value).map_err(|_| SavedSessionError::Decode)?;
                if decoded.version != SAVED_SESSION_VERSION {
                    return Err(SavedSessionError::UnsupportedVersion(decoded.version));
                }
                let saved = Self {
                    version: decoded.version,
                    patch_capacity: decoded.patch_capacity,
                    patches: decoded.patches,
                    mixer: decoded.mixer,
                    master_gain_db: decoded.master_gain_db,
                    returns: decoded.returns,
                };
                saved.validate_shape()?;
                Ok(saved)
            }
            Some(3) => {
                let decoded: SavedSessionV3 =
                    serde_json::from_value(value).map_err(|_| SavedSessionError::Decode)?;
                Self::migrate_track_sends(
                    decoded.patches,
                    decoded.mixer,
                    decoded.master_gain_db,
                    decoded.returns,
                )
            }
            Some(2) => {
                let decoded: SavedSessionV2 =
                    serde_json::from_value(value).map_err(|_| SavedSessionError::Decode)?;
                if decoded.version != 2 {
                    return Err(SavedSessionError::UnsupportedVersion(decoded.version));
                }
                Self::migrate_legacy(
                    decoded.patches,
                    decoded.mixer,
                    decoded.master_gain_db,
                    decoded.returns,
                )
            }
            None | Some(1) => {
                let decoded: SavedSessionV1 =
                    serde_json::from_value(value).map_err(|_| SavedSessionError::Decode)?;
                if decoded.version.is_some_and(|value| value != 1) {
                    return Err(SavedSessionError::UnsupportedVersion(
                        decoded.version.unwrap_or_default(),
                    ));
                }
                let patches = decoded
                    .patches
                    .into_iter()
                    .map(|patch| {
                        let descriptor = capabilities
                            .descriptor_for_config(&patch.instrument)
                            .ok_or(SavedSessionError::InvalidCapability)?;
                        Ok(SavedPatchV3 {
                            id: patch.id,
                            name: patch.name,
                            instrument: patch.instrument,
                            channel: patch.channel,
                            envelope: patch.envelope,
                            output: patch.output,
                            effects: patch.effects,
                            voice_limit: descriptor.voice_policy().polyphony_ceiling(),
                        })
                    })
                    .collect::<Result<Vec<_>, SavedSessionError>>()?;
                Self::migrate_legacy(
                    patches,
                    decoded.mixer,
                    decoded.master_gain_db,
                    decoded.returns,
                )
            }
            Some(version) => Err(SavedSessionError::UnsupportedVersion(
                u32::try_from(version).unwrap_or(u32::MAX),
            )),
        }
    }

    fn migrate_legacy(
        patches: Vec<SavedPatchV3>,
        mut mixer: MixerState,
        master_gain_db: f32,
        returns: Vec<SavedReturnV2>,
    ) -> Result<Self, SavedSessionError> {
        // Versions 1 and 2 had exactly eight positional returns. Validate the
        // original shape before extending it, so migration never drops routes.
        const LEGACY_RETURN_COUNT: usize = 8;
        if returns.len() != LEGACY_RETURN_COUNT
            || mixer
                .tracks()
                .iter()
                .any(|track| track.sends().len() != LEGACY_RETURN_COUNT)
        {
            return Err(SavedSessionError::InvalidShape);
        }
        for id in MixerTrackId::ALL {
            let track = mixer
                .track(id)
                .clone()
                .with_send_count(DEFAULT_BUS_RETURNS)
                .map_err(|_| SavedSessionError::InvalidShape)?;
            mixer.set_track(id, track);
        }
        let mut returns = returns
            .into_iter()
            .map(|saved| SavedReturn {
                name: "INIT".to_owned(),
                effects: saved.effect.into_iter().collect(),
                return_level: saved.return_level,
            })
            .collect::<Vec<_>>();
        returns.resize_with(DEFAULT_BUS_RETURNS, || SavedReturn {
            name: "INIT".to_owned(),
            effects: Vec::new(),
            return_level: crate::mixer::bus_return::RETURN_LEVEL_DESCRIPTOR.default(),
        });
        Self::migrate_track_sends(patches, mixer, master_gain_db, returns)
    }

    fn migrate_track_sends(
        patches: Vec<SavedPatchV3>,
        mut mixer: MixerState,
        master_gain_db: f32,
        returns: Vec<SavedReturn>,
    ) -> Result<Self, SavedSessionError> {
        if returns.is_empty()
            || returns.len() > usize::from(BusId::MAX) + 1
            || mixer
                .tracks()
                .iter()
                .any(|track| track.sends().len() != returns.len())
        {
            return Err(SavedSessionError::InvalidShape);
        }
        let mut routed = [false; MixerTrackId::COUNT];
        // Read every original track before clearing any shared route, so all
        // Patches on that track retain the same audible contribution.
        let patches = patches
            .into_iter()
            .map(|patch| {
                let track = patch.output.track_id();
                routed[track.index()] = true;
                SavedPatch {
                    id: patch.id,
                    name: patch.name,
                    instrument: patch.instrument,
                    channel: patch.channel,
                    envelope: patch.envelope,
                    output: patch.output,
                    effects: patch.effects,
                    voice_limit: patch.voice_limit,
                    sends: mixer.track(track).sends().to_vec(),
                }
            })
            .collect();
        for track_id in MixerTrackId::ALL {
            if routed[track_id.index()] {
                let track = mixer.track(track_id);
                let cleared = MixerTrackParameters::from_values(
                    track.level_db(),
                    track.pan(),
                    track.mute(),
                    track.solo(),
                    vec![BUS_SEND_DESCRIPTOR.default(); returns.len()],
                )
                .map_err(|_| SavedSessionError::InvalidShape)?;
                mixer.set_track(track_id, cleared);
            }
        }
        let saved = Self {
            version: SAVED_SESSION_VERSION,
            patch_capacity: default_patch_capacity(),
            patches,
            mixer,
            master_gain_db,
            returns,
        };
        saved.validate_shape()?;
        Ok(saved)
    }

    /// Validates and prepares a complete replacement session without exposing
    /// the candidate canonical state separately from its callback-ready graph.
    /// Callers may stage [`PreparedSavedSession::into_replacement`] only after
    /// this returns `Ok`; every failure leaves their active session and graph
    /// untouched.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_restore(
        &self,
        capabilities: CapabilityRegistry,
        effects: EffectCapabilityRegistry,
        instrument_preparers: &[Box<dyn InstrumentPreparer>],
        effect_preparers: &[Box<dyn EffectPreparer>],
        graph_revision: GraphRevision,
        sample_rate: f32,
        max_frames: usize,
    ) -> Result<PreparedSavedSession, SavedSessionRestoreError> {
        self.validate_shape()?;
        let mut capabilities = capabilities;
        for patch in &self.patches {
            let preparer = instrument_preparers
                .iter()
                .find(|preparer| preparer.capability_id() == patch.instrument.capability_id())
                .ok_or(SavedSessionError::InvalidCapability)?;
            if let Some(descriptor) = preparer
                .asset_descriptor(&patch.instrument)
                .map_err(SavedSessionRestoreError::AssetMetadata)?
            {
                capabilities = capabilities
                    .with_asset_descriptor(descriptor)
                    .map_err(|_| SavedSessionError::InvalidCapability)?;
            }
        }
        let state = self.restore_candidate(capabilities, effects, graph_revision)?;
        let parameters = StateProjector::for_graph(graph_revision)
            .project(&state)
            .map_err(SavedSessionRestoreError::Projection)?
            .2;
        let graph = PreparedGraphBuilder::new(state.capabilities(), instrument_preparers)
            .with_effects(state.effects(), effect_preparers)
            .with_returns(state.bus_returns())
            .build(
                graph_revision,
                state.patches(),
                parameters,
                sample_rate,
                max_frames,
            )
            .map_err(SavedSessionRestoreError::Preparation)?;
        Ok(PreparedSavedSession { state, graph })
    }

    /// Reconstructs one private candidate with fresh transient runtime and
    /// interaction state. It is deliberately not public: persistence callers
    /// must pass through [`Self::prepare_restore`] so canonical state cannot be
    /// committed before complete-graph preparation succeeds.
    fn restore_candidate(
        &self,
        capabilities: CapabilityRegistry,
        effects: EffectCapabilityRegistry,
        graph_revision: GraphRevision,
    ) -> Result<AppState, SavedSessionError> {
        self.validate_shape()?;
        let mut patches = Vec::with_capacity(self.patches.len());
        for saved in &self.patches {
            if saved
                .instrument
                .asset_references()
                .iter()
                .any(|assignment| {
                    crate::synth::AssetFileId::new(assignment.reference().locator())
                        .is_ok_and(|asset| asset.is_external())
                })
            {
                return Err(SavedSessionError::InvalidCapability);
            }
            capabilities
                .validate_config(&saved.instrument)
                .map_err(|_| SavedSessionError::InvalidCapability)?;
            let descriptor = capabilities
                .descriptor_for_config(&saved.instrument)
                .ok_or(SavedSessionError::InvalidCapability)?;
            if saved.voice_limit == 0
                || saved.voice_limit > descriptor.voice_policy().polyphony_ceiling()
            {
                return Err(SavedSessionError::InvalidVoiceLimit);
            }
            let envelope = VoiceEnvelope::new(
                saved.envelope.attack_milliseconds(),
                saved.envelope.decay_milliseconds(),
                saved.envelope.sustain(),
                saved.envelope.release_milliseconds(),
            )
            .map_err(|_| SavedSessionError::InvalidPatch)?;
            let mut patch = Patch::new(
                PatchId::new(saved.id).map_err(|_| SavedSessionError::InvalidPatch)?,
                saved.name.clone(),
                saved.instrument.clone(),
                MidiChannel::new(saved.channel).map_err(|_| SavedSessionError::InvalidPatch)?,
                saved.output,
            )
            .with_envelope(envelope)
            .with_voice_limit(saved.voice_limit)
            .map_err(|_| SavedSessionError::InvalidVoiceLimit)?
            .with_sends(saved.sends.clone())
            .map_err(|_| SavedSessionError::InvalidPatchSend)?;
            for (index, occupant) in saved.effects.iter().enumerate() {
                if let Some(config) = occupant {
                    effects
                        .validate_config(config)
                        .map_err(|_| SavedSessionError::InvalidEffect)?;
                    patch = patch.with_effect_slot(
                        EffectSlotIndex::new(index)
                            .map_err(|_| SavedSessionError::InvalidEffect)?,
                        config.clone(),
                    );
                }
            }
            patches.push(patch);
        }

        let mut returns = BusReturnBank::with_count(self.returns.len())
            .map_err(|_| SavedSessionError::InvalidReturn)?;
        for (index, saved) in self.returns.iter().enumerate() {
            let bus = BusId::new(index as u16).map_err(|_| SavedSessionError::InvalidReturn)?;
            for config in &saved.effects {
                effects
                    .validate_config(config)
                    .map_err(|_| SavedSessionError::InvalidEffect)?;
            }
            let bus_return = BusReturn::unoccupied(bus)
                .with_name(&saved.name)
                .and_then(|value| value.with_effects(saved.effects.clone()))
                .and_then(|value| value.with_return_level(saved.return_level))
                .map_err(|_| SavedSessionError::InvalidReturn)?;
            returns
                .replace_return(bus_return)
                .map_err(|_| SavedSessionError::InvalidReturn)?;
        }

        let limits = patches
            .iter()
            .map(|patch| (patch.id(), patch.voice_limit().value()))
            .collect::<Vec<_>>();
        let mut state = AppState::for_graph_with_effects(
            capabilities,
            effects,
            GlobalParameters::new(self.master_gain_db)
                .map_err(|_| SavedSessionError::InvalidGlobal)?,
            graph_revision,
        )
        .with_initial_mixer(self.mixer.clone())
        .with_initial_returns(returns)
        .with_initial_patch_capacity(self.patch_capacity)
        .map_err(|_| SavedSessionError::InvalidShape)?;
        state
            .apply(AppEvent::InstallPatches(patches))
            .map_err(|_| SavedSessionError::InvalidPatch)?;
        state
            .restore_voice_limits(&limits)
            .map_err(|_| SavedSessionError::InvalidVoiceLimit)?;
        Ok(state)
    }

    /// Bound asset resolution before any preparer can read a saved bank.
    fn validate_shape(&self) -> Result<(), SavedSessionError> {
        if self.version != SAVED_SESSION_VERSION
            || self.returns.is_empty()
            || self.returns.len() > usize::from(BusId::MAX) + 1
            || self
                .mixer
                .tracks()
                .iter()
                .any(|track| track.sends().len() != self.returns.len())
        {
            return Err(SavedSessionError::InvalidShape);
        }
        if self.patch_capacity == 0
            || self.patch_capacity > crate::kernel::MAX_ACTIVE_PATCHES
            || self.patches.len() > self.patch_capacity
        {
            return Err(SavedSessionError::InvalidPatch);
        }
        if self.patches.iter().any(|patch| {
            patch.sends.len() != self.returns.len()
                || patch
                    .sends
                    .iter()
                    .any(|value| !BUS_SEND_DESCRIPTOR.contains(*value))
        }) {
            return Err(SavedSessionError::InvalidPatchSend);
        }
        Ok(())
    }
}

/// One completely validated persistence replacement. The state and graph are
/// consumed together so application composition can perform one atomic
/// control-side install/handoff rather than publishing an unprepared session.
pub struct PreparedSavedSession {
    state: AppState,
    graph: PreparedGraph,
}

impl PreparedSavedSession {
    pub const fn state(&self) -> &AppState {
        &self.state
    }

    pub const fn graph(&self) -> &PreparedGraph {
        &self.graph
    }

    /// Consumes the private candidate into the only reducer payload allowed to
    /// accompany its exact prepared graph. No mutable candidate `AppState`
    /// escapes the saved-session boundary.
    pub fn into_replacement(self) -> (SessionReplacementPayload, PreparedGraph) {
        let visualizations = self
            .state
            .patches()
            .iter()
            .filter_map(|patch| {
                let summaries = self.graph.prepared_sample_visualizations(patch.id());
                (!summaries.is_empty()).then_some((patch.id(), summaries))
            })
            .collect();
        let payload = SessionReplacementPayload::from_prepared_state(
            &self.state,
            self.graph.revision(),
            visualizations,
        );
        (payload, self.graph)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SavedSessionRestoreError {
    #[error("saved instrument asset could not be resolved: {0}")]
    AssetMetadata(crate::synth::InstrumentPreparationError),
    #[error("saved session validation failed: {0}")]
    Session(#[from] SavedSessionError),
    #[error("saved session projection failed: {0}")]
    Projection(StateProjectionError),
    #[error("saved session graph preparation failed: {0}")]
    Preparation(GraphPreparationError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SavedSessionError {
    #[error("saved session could not be encoded")]
    Encode,
    #[error("saved session could not be decoded")]
    Decode,
    #[error("unsupported saved session version {0}")]
    UnsupportedVersion(u32),
    #[error("saved session shape is invalid")]
    InvalidShape,
    #[error("saved Patch is invalid")]
    InvalidPatch,
    #[error("saved Patch sends have invalid values or bank shape")]
    InvalidPatchSend,
    #[error("saved capability configuration is unavailable or invalid")]
    InvalidCapability,
    #[error("saved effect configuration is unavailable or invalid")]
    InvalidEffect,
    #[error("saved return is invalid")]
    InvalidReturn,
    #[error("saved global parameters are invalid")]
    InvalidGlobal,
    #[error("saved voice limit is invalid for its capability")]
    InvalidVoiceLimit,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::sample_capability::{
        SampleCapability, SAMPLE_ASSET_PARAMETER_ID, SAMPLE_PLAYBACK_START_PARAMETER_ID,
    };
    use crate::kernel::midi_message::MidiMessage;
    use crate::kernel::MidiChannel;
    use crate::mixer::patch_output::PatchOutput;
    use crate::real_time::RtPatchParameters;
    use crate::synth::{
        AssetFileId, AssetKind, AssetReference, CapabilityId, InstrumentCapabilityProvider,
        InstrumentPreparationError, ParameterId, ParameterValue, PreparedAssetFootprint,
        PreparedInstrument, PreparedInstrumentError, RackPreparationError, SampleAssetError,
        MAX_SAMPLE_GRAPH_PCM_BYTES,
    };

    fn state() -> AppState {
        let provider = SampleCapability::new(AssetFileId::new("folder/kick.wav").unwrap()).unwrap();
        let registry = CapabilityRegistry::new(vec![provider.descriptor()]).unwrap();
        let descriptor = provider.descriptor();
        let config = provider
            .default_config()
            .unwrap()
            .with_scalar_value(
                &descriptor,
                &ParameterId::new(SAMPLE_PLAYBACK_START_PARAMETER_ID).unwrap(),
                ParameterValue::continuous(0.25).unwrap(),
            )
            .unwrap();
        let patch = Patch::new(
            PatchId::new(1).unwrap(),
            "Saved Sample".to_owned(),
            config,
            MidiChannel::new(0).unwrap(),
            PatchOutput::default(),
        );
        let mut state = AppState::new(registry, GlobalParameters::new(-3.0).unwrap());
        state.apply(AppEvent::InstallPatches(vec![patch])).unwrap();
        state
    }

    fn asset_effect(slot: u16, assets: &[(AssetKind, &str)]) -> PostEffectConfig {
        use crate::synth::{EffectCapabilityId, EffectSlotId, ParameterAssignment};
        PostEffectConfig::from_parts(
            EffectSlotId::new(slot).unwrap(),
            EffectCapabilityId::new("effect.test.resources").unwrap(),
            vec![ParameterAssignment::new(
                ParameterId::new("effect.mix").unwrap(),
                ParameterValue::Continuous(0.375),
            )],
            assets
                .iter()
                .enumerate()
                .map(|(index, (kind, locator))| {
                    AssetAssignment::new(
                        ParameterId::new(format!("effect.asset-{index}")).unwrap(),
                        AssetReference::new(*kind, *locator).unwrap(),
                    )
                })
                .collect(),
        )
    }

    fn asset_rich_session() -> SavedSession {
        use crate::adapter::drum_rack_capability::{pad_parameter_id, DrumRackCapability};
        let mut saved = SavedSession::capture(&state());
        let drum = DrumRackCapability::new(AssetFileId::new("folder/kick.wav").unwrap()).unwrap();
        let assets = [
            (0, "folder/kick.wav"),
            (3, "folder/snare.wav"),
            (9, "folder/kick.wav"),
        ]
        .into_iter()
        .map(|(pad, locator)| {
            AssetAssignment::new(
                pad_parameter_id(pad, SAMPLE_ASSET_PARAMETER_ID),
                AssetReference::new(AssetKind::Sample, locator).unwrap(),
            )
        })
        .collect::<Vec<_>>();
        saved.patches[0].instrument = drum
            .descriptor()
            .create_config(drum.default_config().unwrap().values(), &assets)
            .unwrap();
        saved.patches[0].effects[1] = Some(asset_effect(
            7,
            &[(AssetKind::NeuralModel, "models/amp.nam")],
        ));
        saved.patches[0].effects[2] = Some(asset_effect(
            2,
            &[
                (AssetKind::ImpulseResponse, "impulses/cab.wav"),
                (AssetKind::Other, "catalog:resource/17"),
            ],
        ));
        let mut second = saved.patches[0].clone();
        second.id = 11;
        saved.patches.push(second);
        saved.patches[0].channel = 4;
        saved.patches[0].output = PatchOutput::new(MixerTrackId::new(3).unwrap(), -9.0).unwrap();
        saved.patches[0].sends[1] = 0.625;
        saved.returns[1].name = "Resource return".to_owned();
        saved.returns[1].return_level = 0.375;
        saved.returns[1].effects = vec![
            asset_effect(15, &[(AssetKind::NeuralModel, "models/amp.nam")]),
            asset_effect(4, &[(AssetKind::ImpulseResponse, "impulses/room.wav")]),
        ];
        saved
    }

    #[test]
    fn asset_inventory_covers_all_drum_pads_patch_slots_and_return_effects_once() {
        let saved = asset_rich_session();
        let expected = [
            (AssetKind::Sample, "folder/kick.wav"),
            (AssetKind::Sample, "folder/snare.wav"),
            (AssetKind::NeuralModel, "models/amp.nam"),
            (AssetKind::ImpulseResponse, "impulses/cab.wav"),
            (AssetKind::Other, "catalog:resource/17"),
            (AssetKind::ImpulseResponse, "impulses/room.wav"),
        ]
        .into_iter()
        .map(|(kind, locator)| AssetReference::new(kind, locator).unwrap())
        .collect::<Vec<_>>();
        assert_eq!(saved.asset_references(), expected);
        let sound = saved.sound_definition(PatchId::new(1).unwrap()).unwrap();
        assert_eq!(sound.asset_references(), expected[..5]);
        assert_eq!(sound.instrument().asset_references().len(), 3);
    }

    #[test]
    fn relink_changes_only_exact_references_and_preserves_complete_saved_content() {
        let saved = asset_rich_session();
        let before = saved.to_json().unwrap();
        let replacements = saved
            .asset_references()
            .into_iter()
            .enumerate()
            .map(|(index, original)| {
                let replacement =
                    AssetReference::new(original.kind(), format!("collected/resource-{index}"))
                        .unwrap();
                (original, replacement)
            })
            .collect::<Vec<_>>();
        let candidate = saved.with_relinked_assets(&replacements).unwrap();
        let expected_json = replacements
            .iter()
            .fold(before.clone(), |json, (old, new)| {
                json.replace(
                    &serde_json::to_string(old.locator()).unwrap(),
                    &serde_json::to_string(new.locator()).unwrap(),
                )
            });
        assert_eq!(candidate.to_json().unwrap(), expected_json);
        assert_eq!(saved.to_json().unwrap(), before);
        assert_eq!(saved.with_relinked_assets(&[]).unwrap(), saved);

        let sound = saved.sound_definition(PatchId::new(1).unwrap()).unwrap();
        let relinked_sound = sound.with_relinked_assets(&replacements[..5]).unwrap();
        assert_eq!(
            candidate
                .sound_definition(PatchId::new(1).unwrap())
                .unwrap(),
            relinked_sound,
        );
        assert_eq!(sound.with_relinked_assets(&[]).unwrap(), sound);
        assert_eq!(
            SoundDefinition::from_json(&relinked_sound.to_json().unwrap()).unwrap(),
            relinked_sound,
        );
    }

    #[test]
    fn relink_is_simultaneous_and_keys_include_asset_kind() {
        let mut saved = asset_rich_session();
        saved.patches[0].effects[0] =
            Some(asset_effect(99, &[(AssetKind::Other, "folder/kick.wav")]));
        let kick = AssetReference::new(AssetKind::Sample, "folder/kick.wav").unwrap();
        let snare = AssetReference::new(AssetKind::Sample, "folder/snare.wav").unwrap();
        let swapped = saved
            .with_relinked_assets(&[(kick.clone(), snare.clone()), (snare.clone(), kick.clone())])
            .unwrap();
        let pad_assets = swapped.patches[0].instrument.asset_references();
        assert_eq!(pad_assets[0].reference(), &snare);
        assert_eq!(pad_assets[1].reference(), &kick);
        assert_eq!(pad_assets[2].reference(), &snare);
        assert_eq!(swapped.patches[0].effects[0], saved.patches[0].effects[0]);
        assert_eq!(
            swapped
                .with_relinked_assets(&[(kick.clone(), snare.clone()), (snare, kick)])
                .unwrap(),
            saved,
        );
    }

    #[test]
    fn relink_rejects_invalid_duplicate_unknown_and_wrong_kind_mappings_atomically() {
        let saved = SavedSession::capture(&state());
        let sound = saved.sound_definition(PatchId::new(1).unwrap()).unwrap();
        let original = saved.asset_references()[0].clone();
        let target = AssetReference::new(AssetKind::Sample, "relocated/kick.wav").unwrap();
        let unknown = AssetReference::new(AssetKind::Sample, "missing.wav").unwrap();
        let wrong_kind = AssetReference::new(AssetKind::SoundFont, "bank.sf2").unwrap();
        let invalid: AssetReference = serde_json::from_value(serde_json::json!({
            "kind": "sample", "locator": "",
        }))
        .unwrap();
        let cases = [
            (
                vec![
                    (original.clone(), target.clone()),
                    (original.clone(), original.clone()),
                ],
                AssetRelinkError::DuplicateOriginal(original.clone()),
            ),
            (
                vec![
                    (original.clone(), target.clone()),
                    (unknown.clone(), target.clone()),
                ],
                AssetRelinkError::UnknownOriginal(unknown),
            ),
            (
                vec![(original.clone(), wrong_kind.clone())],
                AssetRelinkError::KindMismatch {
                    original: original.clone(),
                    replacement: wrong_kind,
                },
            ),
            (
                vec![(original.clone(), invalid.clone())],
                AssetRelinkError::InvalidReference(invalid.clone()),
            ),
            (
                vec![(invalid.clone(), target)],
                AssetRelinkError::InvalidReference(invalid),
            ),
        ];
        let before = saved.to_json().unwrap();
        for (mapping, expected) in cases {
            assert_eq!(saved.with_relinked_assets(&mapping), Err(expected.clone()));
            assert_eq!(sound.with_relinked_assets(&mapping), Err(expected));
        }
        assert_eq!(saved.to_json().unwrap(), before);
        assert_eq!(
            saved.sound_definition(PatchId::new(1).unwrap()).unwrap(),
            sound
        );
        assert_eq!(
            saved
                .with_relinked_assets(&[(original.clone(), original)])
                .unwrap(),
            saved
        );
    }

    #[test]
    fn relink_preserves_canonical_catalog_locator_schemes_without_admitting_them() {
        let saved = SavedSession::capture(&state());
        let original = saved.asset_references()[0].clone();
        for locator in ["./sf2/HiDef.sf2", "@bundled/tone.wav", "catalog:sample/17"] {
            let replacement = AssetReference::new(AssetKind::Sample, locator).unwrap();
            let candidate = saved
                .with_relinked_assets(&[(original.clone(), replacement.clone())])
                .unwrap();
            assert_eq!(candidate.asset_references(), vec![replacement]);
        }
    }

    fn assert_scoped_relink_preserves_choice(
        original_descriptor: crate::synth::CapabilityDescriptor,
        relocated_descriptor: crate::synth::CapabilityDescriptor,
        preset: &str,
        choice: &str,
    ) {
        use crate::synth::ParameterAssignment;
        let values = vec![ParameterAssignment::new(
            ParameterId::new(preset).unwrap(),
            ParameterValue::Choice(choice.to_owned()),
        )];
        let mut saved = SavedSession::capture(&state());
        saved.patches[0].instrument = original_descriptor
            .create_config(&values, &original_descriptor.default_assets())
            .unwrap();
        let mapping = [(
            original_descriptor.default_assets()[0].reference().clone(),
            relocated_descriptor.default_assets()[0].reference().clone(),
        )];
        let candidate = saved.with_relinked_assets(&mapping).unwrap();
        let registry = CapabilityRegistry::new(vec![original_descriptor])
            .unwrap()
            .with_asset_descriptor(relocated_descriptor)
            .unwrap();
        registry
            .validate_config(&candidate.patches[0].instrument)
            .unwrap();
        assert_eq!(
            candidate.patches[0].instrument.values(),
            saved.patches[0].instrument.values()
        );
        let sound = saved.sound_definition(PatchId::new(1).unwrap()).unwrap();
        assert_eq!(
            sound.with_relinked_assets(&mapping).unwrap(),
            candidate
                .sound_definition(PatchId::new(1).unwrap())
                .unwrap()
        );
    }

    #[test]
    fn relink_preserves_nondefault_soundfont_and_dx7_presets() {
        use crate::adapter::dx7_library::{Dx7Library, Dx7Preset, PRESET};
        use crate::adapter::hidef_soundfont_capability::{
            HiDefSoundFontCapability, SOUNDFONT_PRESET_PARAMETER_ID,
        };
        use crate::synth::{SoundFontPresetCatalog, SoundFontPresetSource};
        let catalog = std::sync::Arc::new(
            SoundFontPresetCatalog::from_sources([
                SoundFontPresetSource::new(0, 0, 0, "Default", true),
                SoundFontPresetSource::new(1, 128, 7, "Saved Kit", true),
            ])
            .unwrap(),
        );
        let original = HiDefSoundFontCapability::new(catalog.clone()).unwrap();
        let relocated = HiDefSoundFontCapability::for_asset(
            catalog,
            AssetReference::new(AssetKind::SoundFont, "collected/bank.sf2").unwrap(),
        )
        .unwrap();
        assert_scoped_relink_preserves_choice(
            original.descriptor(),
            relocated.descriptor(),
            SOUNDFONT_PRESET_PARAMETER_ID,
            "sf2.bank-128.program-7",
        );

        let library = Dx7Library {
            presets: vec![
                Dx7Preset {
                    id: "message-0.voice-0".into(),
                    name: "Default".into(),
                    data: [0; 155],
                },
                Dx7Preset {
                    id: "message-1.voice-17".into(),
                    name: "Saved Voice".into(),
                    data: [0; 155],
                },
            ],
        };
        assert_scoped_relink_preserves_choice(
            library
                .descriptor(AssetReference::new(AssetKind::SysEx, "bank.syx").unwrap())
                .unwrap(),
            library
                .descriptor(AssetReference::new(AssetKind::SysEx, "collected/bank.syx").unwrap())
                .unwrap(),
            PRESET,
            "message-1.voice-17",
        );
    }

    #[test]
    fn relink_defers_missing_asset_admission_to_preparation_without_changing_source() {
        let active = state();
        let saved = SavedSession::capture(&active);
        let replacement = AssetReference::new(AssetKind::Sample, "missing.wav").unwrap();
        let candidate = saved
            .with_relinked_assets(&[(saved.asset_references()[0].clone(), replacement)])
            .unwrap();
        assert!(matches!(
            candidate.prepare_restore(
                active.capabilities().clone(),
                active.effects().clone(),
                &sample_preparers(Ok(vec![1]), Ok(decoded("folder/kick.wav"))),
                &[],
                GraphRevision::INITIAL.checked_next().unwrap(),
                48_000.0,
                64,
            ),
            Err(SavedSessionRestoreError::Preparation(_))
        ));
        assert_eq!(SavedSession::capture(&active), saved);
    }

    #[test]
    fn sound_definition_round_trip_excludes_installation_and_mix() {
        let saved = SavedSession::capture(&state());
        let definition = saved.sound_definition(PatchId::new(1).unwrap()).unwrap();
        let json = definition.to_json().unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 6);
        assert_eq!(value["version"], SOUND_DEFINITION_VERSION);
        assert_eq!(value["name"], "Saved Sample");
        for field in [
            "id",
            "channel",
            "output",
            "sends",
            "mixer",
            "masterGainDb",
            "returns",
        ] {
            assert!(value.get(field).is_none(), "unexpected field {field}");
        }
        assert_eq!(SoundDefinition::from_json(&json).unwrap(), definition);
        assert_eq!(
            serde_json::from_str::<SoundDefinition>(&json).unwrap(),
            definition
        );
    }

    #[test]
    fn sound_install_preserves_local_routing_mix_and_other_patches_after_preparation() {
        use crate::adapter::chorus_capability::ChorusCapability;
        use crate::synth::{EffectCapabilityProvider, EffectSlotId};

        let active = state();
        let target = PatchId::new(1).unwrap();
        let provider = ChorusCapability::new().unwrap();
        let effects = EffectCapabilityRegistry::new(vec![provider.descriptor()]).unwrap();
        let mut saved = SavedSession::capture(&active);
        let mut other = saved.patches[0].clone();
        other.id = 7;
        saved.patches.push(other.clone());
        saved.patches[0].channel = 5;
        saved.patches[0].output = PatchOutput::new(MixerTrackId::new(3).unwrap(), -9.0).unwrap();
        saved.patches[0].sends[2] = 0.65;
        saved.mixer.set_track(
            MixerTrackId::new(3).unwrap(),
            MixerTrackParameters::from_values(
                -6.0,
                0.3,
                false,
                true,
                vec![0.2; saved.returns.len()],
            )
            .unwrap(),
        );
        saved.master_gain_db = -12.0;
        saved.returns[2].name = "Local return".to_owned();
        saved.returns[2].return_level = 0.4;

        let mut definition = saved.sound_definition(target).unwrap();
        definition.name = "Reusable sound".to_owned();
        definition.envelope = VoiceEnvelope::new(12.0, 90.0, 0.4, 180.0).unwrap();
        definition.voice_limit = 8;
        definition.instrument = definition
            .instrument
            .with_scalar_value(
                active
                    .capabilities()
                    .descriptor_for_config(&definition.instrument)
                    .unwrap(),
                &ParameterId::new(SAMPLE_PLAYBACK_START_PARAMETER_ID).unwrap(),
                ParameterValue::continuous(0.5).unwrap(),
            )
            .unwrap();
        definition.effects[1] = Some(
            provider
                .default_config(EffectSlotId::new(9).unwrap())
                .unwrap(),
        );
        definition.effects[2] = Some(
            provider
                .default_config(EffectSlotId::new(2).unwrap())
                .unwrap(),
        );
        let definition = SoundDefinition::from_json(&definition.to_json().unwrap()).unwrap();
        let candidate = saved
            .with_sound_definition(target, definition.clone())
            .unwrap();
        assert_eq!(candidate.sound_definition(target).unwrap(), definition);
        assert_eq!(candidate.patches[0].id, saved.patches[0].id);
        assert_eq!(candidate.patches[0].channel, 5);
        assert_eq!(candidate.patches[0].output, saved.patches[0].output);
        assert_eq!(candidate.patches[0].sends, saved.patches[0].sends);
        assert_eq!(candidate.patches[1], other);
        assert_eq!(candidate.mixer, saved.mixer);
        assert_eq!(candidate.master_gain_db, saved.master_gain_db);
        assert_eq!(candidate.returns, saved.returns);

        let effect_preparers: Vec<Box<dyn EffectPreparer>> = vec![Box::new(
            crate::adapter::chorus_preparer::ChorusPreparer::new().unwrap(),
        )];
        let prepared = candidate
            .prepare_restore(
                active.capabilities().clone(),
                effects,
                &sample_preparers(Ok(vec![1]), Ok(decoded("folder/kick.wav"))),
                &effect_preparers,
                GraphRevision::INITIAL.checked_next().unwrap(),
                48_000.0,
                64,
            )
            .unwrap();
        assert_eq!(SavedSession::capture(prepared.state()), candidate);
        assert_eq!(
            saved.sound_definition(target).unwrap().name(),
            "Saved Sample"
        );
    }

    #[test]
    fn sound_install_rejects_unknown_targets_and_unsupported_versions() {
        let saved = SavedSession::capture(&state());
        let target = PatchId::new(1).unwrap();
        let missing = PatchId::new(99).unwrap();
        let definition = saved.sound_definition(target).unwrap();
        assert_eq!(
            saved.sound_definition(missing),
            Err(SoundDefinitionError::UnknownPatch(missing))
        );
        assert_eq!(
            saved.with_sound_definition(missing, definition.clone()),
            Err(SoundDefinitionError::UnknownPatch(missing))
        );
        let mut value = serde_json::to_value(&definition).unwrap();
        value["version"] = 42.into();
        assert_eq!(
            SoundDefinition::from_json(&value.to_string()),
            Err(SoundDefinitionError::UnsupportedVersion(42))
        );
        // Direct serde decoding is also checked at the installation boundary.
        let unsupported: SoundDefinition = serde_json::from_value(value).unwrap();
        assert_eq!(
            saved.with_sound_definition(target, unsupported),
            Err(SoundDefinitionError::UnsupportedVersion(42))
        );
        let mut value = serde_json::to_value(&definition).unwrap();
        value["channel"] = 9.into();
        assert_eq!(
            SoundDefinition::from_json(&value.to_string()),
            Err(SoundDefinitionError::Decode)
        );
        assert_eq!(saved.sound_definition(target).unwrap(), definition);
    }

    #[test]
    fn invalid_sound_values_fail_canonical_candidate_preparation() {
        use crate::adapter::chorus_capability::ChorusCapability;
        use crate::synth::EffectSlotId;

        let active = state();
        let saved = SavedSession::capture(&active);
        let target = PatchId::new(1).unwrap();
        let definition = saved.sound_definition(target).unwrap();
        let mut invalid_voice_limit = definition.clone();
        invalid_voice_limit.voice_limit = 0;
        let mut invalid_envelope = serde_json::to_value(&definition).unwrap();
        invalid_envelope["envelope"]["sustain"] = 2.0.into();
        let invalid_envelope = SoundDefinition::from_json(&invalid_envelope.to_string()).unwrap();
        let mut unavailable_effect = definition.clone();
        unavailable_effect.effects[0] = Some(
            ChorusCapability::new()
                .unwrap()
                .default_config(EffectSlotId::new(1).unwrap())
                .unwrap(),
        );
        let preparers = sample_preparers(Ok(vec![1]), Ok(decoded("folder/kick.wav")));
        for (invalid, expected) in [
            (invalid_voice_limit, SavedSessionError::InvalidVoiceLimit),
            (invalid_envelope, SavedSessionError::InvalidPatch),
            (unavailable_effect, SavedSessionError::InvalidEffect),
        ] {
            let candidate = saved.with_sound_definition(target, invalid).unwrap();
            assert!(matches!(candidate.prepare_restore(
                active.capabilities().clone(), active.effects().clone(), &preparers, &[],
                GraphRevision::INITIAL.checked_next().unwrap(), 48_000.0, 64,
            ), Err(SavedSessionRestoreError::Session(error)) if error == expected));
        }
        assert_eq!(SavedSession::capture(&active), saved);
    }

    #[test]
    fn oversized_restore_is_rejected_before_asset_resolution() {
        let active = state();
        let mut saved = SavedSession::capture(&active);
        saved.patches = vec![saved.patches[0].clone(); crate::kernel::MAX_ACTIVE_PATCHES + 1];
        assert!(matches!(
            saved.prepare_restore(
                active.capabilities().clone(),
                active.effects().clone(),
                &[],
                &[],
                GraphRevision::INITIAL,
                48_000.0,
                64,
            ),
            Err(SavedSessionRestoreError::Session(
                SavedSessionError::InvalidPatch
            ))
        ));
    }

    #[test]
    fn saved_assets_cannot_reference_transient_browser_locations() {
        let state = state();
        let document = SavedSession::capture(&state)
            .to_json()
            .unwrap()
            .replace("folder/kick.wav", "@home/Music/kick.wav");
        let saved = SavedSession::from_json(&document, state.capabilities()).unwrap();
        assert!(matches!(
            saved.restore_candidate(
                state.capabilities().clone(),
                state.effects().clone(),
                GraphRevision::INITIAL
            ),
            Err(SavedSessionError::InvalidCapability)
        ));
    }

    #[test]
    fn current_version_round_trip_keeps_relative_asset_and_normalized_values_only() {
        let mut state = state();
        state
            .apply(AppEvent::SetPatchCapacity { capacity: 3 })
            .unwrap();
        let saved = SavedSession::capture(&state);
        let json = saved.to_json().unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["version"], SAVED_SESSION_VERSION);
        assert_eq!(
            value["patches"][0]["instrument"]["assetReferences"][0]["reference"]["locator"],
            "folder/kick.wav"
        );
        assert!(!json.contains("fileBrowser"));
        assert!(!json.contains("preview"));
        assert!(!json.contains("engineSelection"));
        assert!(!json.contains("focus"));
        assert!(!json.contains("libraryRoot"));
        assert!(!json.contains("decoded"));

        let decoded_session = SavedSession::from_json(&json, state.capabilities()).unwrap();
        assert_eq!(decoded_session, saved);
        let preparers: Vec<Box<dyn InstrumentPreparer>> = vec![Box::new(
            crate::adapter::sample_preparer::SamplePreparer::new(
                std::sync::Arc::new(crate::testing::DeterministicSampleCatalog::new(
                    [],
                    [(AssetFileId::new("folder/kick.wav").unwrap(), Ok(vec![1]))],
                )),
                std::sync::Arc::new(crate::testing::DeterministicSampleDecoder::new([(
                    AssetFileId::new("folder/kick.wav").unwrap(),
                    Ok(decoded("folder/kick.wav")),
                )])),
            )
            .unwrap(),
        )];
        let prepared = decoded_session
            .prepare_restore(
                state.capabilities().clone(),
                EffectCapabilityRegistry::default(),
                &preparers,
                &[],
                GraphRevision::INITIAL,
                48_000.0,
                64,
            )
            .unwrap();
        let restored = prepared.state();
        assert_eq!(restored.patch_capacity(), 3);
        assert_eq!(prepared.graph().revision(), GraphRevision::INITIAL);
        assert_eq!(
            restored.patches()[0]
                .instrument_config()
                .asset_reference(&ParameterId::new(SAMPLE_ASSET_PARAMETER_ID).unwrap())
                .unwrap()
                .locator(),
            "folder/kick.wav"
        );
        assert_eq!(
            restored.patches()[0]
                .instrument_config()
                .value(&ParameterId::new(SAMPLE_PLAYBACK_START_PARAMETER_ID).unwrap()),
            Some(&ParameterValue::continuous(0.25).unwrap())
        );
        assert_eq!(
            restored.file_browser().lifecycle(),
            crate::control::SampleAssetLifecycle::Unavailable,
            "restore makes unresolved asset availability explicit and never substitutes"
        );
        let (replacement, graph) = prepared.into_replacement();
        assert_eq!(
            replacement.patch_ids().collect::<Vec<_>>(),
            [PatchId::new(1).unwrap()]
        );
        assert_eq!(replacement.target_graph_revision(), graph.revision());
    }

    #[test]
    fn old_sessions_default_capacity_and_invalid_limits_fail_without_truncation() {
        let state = state();
        let mut json = serde_json::to_value(SavedSession::capture(&state)).unwrap();
        json.as_object_mut().unwrap().remove("patchCapacity");
        let decoded = SavedSession::from_json(&json.to_string(), state.capabilities()).unwrap();
        assert_eq!(decoded.patch_capacity, crate::kernel::MAX_ACTIVE_PATCHES);
        for capacity in [0, crate::kernel::MAX_ACTIVE_PATCHES + 1] {
            json["patchCapacity"] = capacity.into();
            assert!(SavedSession::from_json(&json.to_string(), state.capabilities()).is_err());
        }
    }

    #[test]
    fn exact_capacity_session_round_trips_and_prepares_created_patches_only() {
        let seed = state();
        let config = seed.patches()[0].instrument_config().clone();
        let mut full = AppState::new(
            seed.capabilities().clone(),
            GlobalParameters::new(-3.0).unwrap(),
        );
        full.apply(AppEvent::InstallPatches(
            (1..=crate::kernel::MAX_ACTIVE_PATCHES as u32)
                .map(|id| {
                    Patch::new(
                        PatchId::new(id).unwrap(),
                        format!("Patch {id}"),
                        config.clone(),
                        MidiChannel::new(((id - 1) % 16) as u8).unwrap(),
                        PatchOutput::new(
                            crate::mixer::mixer_track_id::MixerTrackId::new(((id - 1) % 16) as u8)
                                .unwrap(),
                            0.0,
                        )
                        .unwrap(),
                    )
                })
                .collect(),
        ))
        .unwrap();

        let saved = SavedSession::capture(&full);
        let json = saved.to_json().unwrap();
        assert!(!json.contains("trailingEmpty"));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&json).unwrap()["patches"]
                .as_array()
                .unwrap()
                .len(),
            crate::kernel::MAX_ACTIVE_PATCHES
        );
        let restored_saved = SavedSession::from_json(&json, full.capabilities()).unwrap();
        assert_eq!(restored_saved, saved);

        let prepared = restored_saved
            .prepare_restore(
                full.capabilities().clone(),
                EffectCapabilityRegistry::default(),
                &sample_preparers(Ok(vec![1]), Ok(decoded("folder/kick.wav"))),
                &[],
                GraphRevision::INITIAL.checked_next().unwrap(),
                48_000.0,
                64,
            )
            .unwrap();
        assert_eq!(
            prepared.state().patches().len(),
            crate::kernel::MAX_ACTIVE_PATCHES
        );
        assert_eq!(
            prepared.graph().initial_parameters().patch_count(),
            crate::kernel::MAX_ACTIVE_PATCHES
        );
        assert_eq!(
            prepared
                .state()
                .patches()
                .iter()
                .map(Patch::id)
                .collect::<Vec<_>>(),
            (1..=crate::kernel::MAX_ACTIVE_PATCHES as u32)
                .map(|id| PatchId::new(id).unwrap())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn current_version_has_exact_top_level_fields_and_no_midi_device_runtime_data() {
        let value = serde_json::to_value(SavedSession::capture(&state())).unwrap();
        let object = value.as_object().unwrap();
        let mut fields = object.keys().map(String::as_str).collect::<Vec<_>>();
        fields.sort_unstable();
        assert_eq!(
            fields,
            [
                "masterGainDb",
                "mixer",
                "patchCapacity",
                "patches",
                "returns",
                "version"
            ]
        );
        assert_eq!(object["version"], SAVED_SESSION_VERSION);

        fn reject_midi_runtime_fields(value: &serde_json::Value) {
            match value {
                serde_json::Value::Object(object) => {
                    for (key, nested) in object {
                        let normalized = key.to_ascii_lowercase();
                        for forbidden in [
                            "midiinput",
                            "mididevice",
                            "preference",
                            "descriptor",
                            "handle",
                            "connection",
                            "callback",
                            "queue",
                            "timestamp",
                            "observation",
                        ] {
                            assert!(
                                !normalized.contains(forbidden),
                                "SavedSession unexpectedly contains `{key}`"
                            );
                        }
                        reject_midi_runtime_fields(nested);
                    }
                }
                serde_json::Value::Array(values) => {
                    values.iter().for_each(reject_midi_runtime_fields)
                }
                _ => {}
            }
        }
        reject_midi_runtime_fields(&value);
    }

    fn decoded(asset: &str) -> crate::synth::DecodedSample {
        let samples = vec![0.0_f32; 128];
        crate::synth::DecodedSample::new(
            crate::synth::SampleMetadata::new(
                AssetFileId::new(asset).unwrap(),
                256,
                48_000,
                1,
                32,
                crate::synth::SampleEncoding::Float,
                samples.len() as u64,
            )
            .unwrap(),
            samples,
        )
        .unwrap()
    }

    fn sample_preparers(
        read: Result<Vec<u8>, SampleAssetError>,
        decode: Result<crate::synth::DecodedSample, SampleAssetError>,
    ) -> Vec<Box<dyn InstrumentPreparer>> {
        let asset = AssetFileId::new("folder/kick.wav").unwrap();
        vec![Box::new(
            crate::adapter::sample_preparer::SamplePreparer::new(
                std::sync::Arc::new(crate::testing::DeterministicSampleCatalog::new(
                    [],
                    [(asset.clone(), read)],
                )),
                std::sync::Arc::new(crate::testing::DeterministicSampleDecoder::new([(
                    asset, decode,
                )])),
            )
            .unwrap(),
        )]
    }

    struct FootprintPreparer {
        capability_id: CapabilityId,
    }

    impl InstrumentPreparer for FootprintPreparer {
        fn capability_id(&self) -> &CapabilityId {
            &self.capability_id
        }

        fn prepare(
            &self,
            patch: &Patch,
            _sample_rate: f32,
            _max_frames: usize,
        ) -> Result<Box<dyn PreparedInstrument>, InstrumentPreparationError> {
            Ok(Box::new(FootprintInstrument {
                patch_id: patch.id(),
                footprint: PreparedAssetFootprint::new(
                    AssetReference::new(AssetKind::Sample, "folder/kick.wav").unwrap(),
                    1,
                    MAX_SAMPLE_GRAPH_PCM_BYTES + 1,
                ),
            }))
        }
    }

    struct FootprintInstrument {
        patch_id: PatchId,
        footprint: PreparedAssetFootprint,
    }

    impl PreparedInstrument for FootprintInstrument {
        fn patch_id(&self) -> PatchId {
            self.patch_id
        }

        fn dispatch(
            &mut self,
            _message: MidiMessage,
            _parameters: &RtPatchParameters,
        ) -> Result<(), PreparedInstrumentError> {
            Ok(())
        }

        fn render(
            &mut self,
            output: &mut [f32],
            _frame_count: usize,
            _parameters: &RtPatchParameters,
        ) -> Result<(), crate::synth::PreparedInstrumentError> {
            output.fill(0.0);

            Ok(())
        }

        fn all_notes_off(&mut self) {}

        fn prepared_asset_footprint(&self) -> Option<&PreparedAssetFootprint> {
            Some(&self.footprint)
        }
    }

    #[test]
    fn restore_prepares_before_commit_and_keeps_unavailable_invalid_and_over_budget_typed() {
        let active = state();
        let active_json = SavedSession::capture(&active).to_json().unwrap();
        let saved = SavedSession::from_json(&active_json, active.capabilities()).unwrap();

        let ready = sample_preparers(Ok(vec![1]), Ok(decoded("folder/kick.wav")));
        let prepared = saved
            .prepare_restore(
                active.capabilities().clone(),
                EffectCapabilityRegistry::default(),
                &ready,
                &[],
                GraphRevision::INITIAL.checked_next().unwrap(),
                48_000.0,
                64,
            )
            .unwrap();
        assert_eq!(
            prepared.graph().revision(),
            GraphRevision::INITIAL.checked_next().unwrap()
        );
        assert_eq!(
            SavedSession::capture(prepared.state()).to_json().unwrap(),
            active_json
        );

        let before = SavedSession::capture(&active).to_json().unwrap();
        let unavailable = sample_preparers(
            Err(SampleAssetError::Unavailable),
            Err(SampleAssetError::Unavailable),
        );
        assert!(matches!(
            saved.prepare_restore(
                active.capabilities().clone(),
                EffectCapabilityRegistry::default(),
                &unavailable,
                &[],
                GraphRevision::INITIAL.checked_next().unwrap(),
                48_000.0,
                64,
            ),
            Err(SavedSessionRestoreError::Preparation(
                GraphPreparationError::Rack(RackPreparationError::Instrument {
                    source: InstrumentPreparationError::SampleAsset {
                        cause: SampleAssetError::Unavailable,
                        ..
                    },
                    ..
                })
            ))
        ));

        let invalid = sample_preparers(Ok(vec![1]), Err(SampleAssetError::MalformedWave));
        assert!(matches!(
            saved.prepare_restore(
                active.capabilities().clone(),
                EffectCapabilityRegistry::default(),
                &invalid,
                &[],
                GraphRevision::INITIAL.checked_next().unwrap(),
                48_000.0,
                64,
            ),
            Err(SavedSessionRestoreError::Preparation(
                GraphPreparationError::Rack(RackPreparationError::Instrument {
                    source: InstrumentPreparationError::SampleAsset {
                        cause: SampleAssetError::MalformedWave,
                        ..
                    },
                    ..
                })
            ))
        ));

        let over_budget: Vec<Box<dyn InstrumentPreparer>> = vec![Box::new(FootprintPreparer {
            capability_id: active.patches()[0]
                .instrument_config()
                .capability_id()
                .clone(),
        })];
        assert!(matches!(
            saved.prepare_restore(
                active.capabilities().clone(),
                EffectCapabilityRegistry::default(),
                &over_budget,
                &[],
                GraphRevision::INITIAL.checked_next().unwrap(),
                48_000.0,
                64,
            ),
            Err(SavedSessionRestoreError::Preparation(
                GraphPreparationError::Rack(
                    RackPreparationError::PreparedAssetCapacityExceeded { .. }
                )
            ))
        ));
        assert_eq!(
            SavedSession::capture(&active).to_json().unwrap(),
            before,
            "every failed candidate leaves the caller-owned active session unchanged"
        );
    }

    #[test]
    fn current_session_round_trip_keeps_independent_sends_on_a_shared_track() {
        let seed = state();
        let bus = BusId::new(2).unwrap();
        let mut first_sends = vec![0.0; DEFAULT_BUS_RETURNS];
        first_sends[bus.index()] = 0.22;
        let mut second_sends = first_sends.clone();
        second_sends[bus.index()] = 0.81;
        let first = seed.patches()[0].clone().with_sends(first_sends).unwrap();
        let second = Patch::new(
            PatchId::new(2).unwrap(),
            "Independent".to_owned(),
            first.instrument_config().clone(),
            MidiChannel::new(1).unwrap(),
            first.output(),
        )
        .with_sends(second_sends)
        .unwrap();
        let mut active = AppState::new(seed.capabilities().clone(), *seed.global());
        active
            .apply(AppEvent::InstallPatches(vec![first, second]))
            .unwrap();

        let saved = SavedSession::capture(&active);
        let decoded_session =
            SavedSession::from_json(&saved.to_json().unwrap(), active.capabilities()).unwrap();
        assert_eq!(decoded_session, saved);
        let prepared = decoded_session
            .prepare_restore(
                active.capabilities().clone(),
                EffectCapabilityRegistry::default(),
                &sample_preparers(Ok(vec![1]), Ok(decoded("folder/kick.wav"))),
                &[],
                GraphRevision::INITIAL,
                48_000.0,
                64,
            )
            .unwrap();
        let restored = prepared.state();
        assert_eq!(
            restored.patches()[0].output().track_id(),
            restored.patches()[1].output().track_id()
        );
        assert_eq!(restored.patches()[0].send(bus), 0.22);
        assert_eq!(restored.patches()[1].send(bus), 0.81);
        assert_eq!(
            restored.mixer().track(MixerTrackId::default()).send(bus),
            0.0
        );
        assert_eq!(SavedSession::capture(restored), saved);
    }

    #[test]
    fn prior_sessions_migrate_each_shared_track_patch_without_doubling_sends() {
        let active = state();
        let mut saved = SavedSession::capture(&active);
        let mut second = saved.patches[0].clone();
        second.id = 2;
        second.name = "Shared Route".to_owned();
        saved.patches.push(second);
        let routed = MixerTrackId::default();
        let empty = MixerTrackId::new(7).unwrap();
        let bus = BusId::new(2).unwrap();
        let mut routed_sends = vec![0.0; DEFAULT_BUS_RETURNS];
        routed_sends[bus.index()] = 0.62;
        saved.mixer.set_track(
            routed,
            MixerTrackParameters::from_values(-8.0, 0.25, false, true, routed_sends).unwrap(),
        );
        saved.mixer.set_track(
            empty,
            MixerTrackParameters::default()
                .with_send(bus, 0.33)
                .unwrap(),
        );

        for version in [1, 2, 3] {
            let migrated = SavedSession::from_json(
                &serde_json::to_string(&legacy_document(&saved, version)).unwrap(),
                active.capabilities(),
            )
            .unwrap();
            let restored = migrated
                .restore_candidate(
                    active.capabilities().clone(),
                    EffectCapabilityRegistry::default(),
                    GraphRevision::INITIAL,
                )
                .unwrap();
            for patch in restored.patches() {
                assert_eq!(patch.output().track_id(), routed);
                assert_eq!(patch.send(bus), 0.62);
                assert_eq!(patch.sends().len(), DEFAULT_BUS_RETURNS);
            }
            let track = restored.mixer().track(routed);
            assert!(track.sends().iter().all(|amount| *amount == 0.0));
            assert_eq!(track.level_db(), -8.0);
            assert_eq!(track.pan(), 0.25);
            assert!(!track.mute());
            assert!(track.solo());
            assert_eq!(restored.mixer().track(empty).send(bus), 0.33);
            assert_eq!(SavedSession::capture(&restored), migrated);
        }
    }

    #[test]
    fn current_patch_sends_are_required_and_validated_before_asset_preparation() {
        let active = state();
        let saved = SavedSession::capture(&active);
        let mut missing = serde_json::to_value(&saved).unwrap();
        missing["patches"][0]
            .as_object_mut()
            .unwrap()
            .remove("sends");
        assert_eq!(
            SavedSession::from_json(&missing.to_string(), active.capabilities()),
            Err(SavedSessionError::Decode)
        );

        for sends in [
            serde_json::json!([]),
            serde_json::json!([0.0]),
            serde_json::json!(vec![0.0; DEFAULT_BUS_RETURNS + 1]),
        ] {
            let mut malformed = serde_json::to_value(&saved).unwrap();
            malformed["patches"][0]["sends"] = sends;
            assert_eq!(
                SavedSession::from_json(&malformed.to_string(), active.capabilities()),
                Err(SavedSessionError::InvalidPatchSend)
            );
        }
        for amount in [-0.01, 1.01, f32::INFINITY, f32::NAN] {
            let mut invalid = saved.clone();
            invalid.patches[0].sends[0] = amount;
            if amount.is_finite() {
                assert_eq!(
                    SavedSession::from_json(&invalid.to_json().unwrap(), active.capabilities()),
                    Err(SavedSessionError::InvalidPatchSend)
                );
            }
            assert!(matches!(
                invalid.prepare_restore(
                    active.capabilities().clone(),
                    EffectCapabilityRegistry::default(),
                    &[],
                    &[],
                    GraphRevision::INITIAL,
                    48_000.0,
                    64,
                ),
                Err(SavedSessionRestoreError::Session(
                    SavedSessionError::InvalidPatchSend
                ))
            ));
        }
    }

    #[test]
    fn named_ordered_return_chains_round_trip_beyond_default_count_and_prepare() {
        use crate::adapter::chorus_capability::{ChorusCapability, CHORUS_AMOUNT_PARAMETER_ID};
        use crate::synth::{EffectCapabilityProvider, EffectSlotId};

        let seed = state();
        let provider = ChorusCapability::new().unwrap();
        let descriptor = provider.descriptor();
        let effects = EffectCapabilityRegistry::new(vec![descriptor.clone()]).unwrap();
        let first = provider
            .default_config(EffectSlotId::new(9).unwrap())
            .unwrap()
            .with_scalar_value(
                &descriptor,
                &ParameterId::new(CHORUS_AMOUNT_PARAMETER_ID).unwrap(),
                ParameterValue::continuous(0.23).unwrap(),
            )
            .unwrap();
        let second = provider
            .default_config(EffectSlotId::new(2).unwrap())
            .unwrap();
        let last_bus = BusId::new(18).unwrap();
        let mut returns = BusReturnBank::with_count(19).unwrap();
        returns
            .replace_return(
                BusReturn::unoccupied(last_bus)
                    .with_name("Wide Room")
                    .unwrap()
                    .with_effects(vec![first.clone(), second.clone()])
                    .unwrap()
                    .with_return_level(0.83)
                    .unwrap(),
            )
            .unwrap();
        let mixer = MixerState::new(std::array::from_fn(|_| {
            crate::mixer::mixer_track_parameters::MixerTrackParameters::default()
                .with_send_count(19)
                .unwrap()
                .with_send(last_bus, 0.45)
                .unwrap()
        }));
        let mut active = AppState::for_graph_with_effects(
            seed.capabilities().clone(),
            effects.clone(),
            *seed.global(),
            GraphRevision::INITIAL,
        )
        .with_initial_mixer(mixer)
        .with_initial_returns(returns);
        active
            .apply(AppEvent::InstallPatches(seed.patches().to_vec()))
            .unwrap();

        let saved = SavedSession::capture(&active);
        let json = saved.to_json().unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["returns"][18]["name"], "Wide Room");
        assert!(value["returns"][18].get("effect").is_none());
        let decoded_session = SavedSession::from_json(&json, active.capabilities()).unwrap();
        assert_eq!(decoded_session, saved);
        let effect_preparers: Vec<Box<dyn EffectPreparer>> = vec![Box::new(
            crate::adapter::chorus_preparer::ChorusPreparer::new().unwrap(),
        )];
        let restored = decoded_session
            .prepare_restore(
                active.capabilities().clone(),
                effects,
                &sample_preparers(Ok(vec![1]), Ok(decoded("folder/kick.wav"))),
                &effect_preparers,
                GraphRevision::INITIAL.checked_next().unwrap(),
                48_000.0,
                64,
            )
            .unwrap();
        assert_eq!(restored.state().bus_returns().len(), 19);
        let bus_return = restored.state().bus_returns().bus_return(last_bus);
        assert_eq!(bus_return.name(), "Wide Room");
        assert_eq!(bus_return.effects(), &[first, second]);
        assert_eq!(bus_return.return_level(), 0.83);
        assert_eq!(
            restored
                .state()
                .mixer()
                .track(MixerTrackId::default())
                .send(last_bus),
            0.45
        );
        assert_eq!(SavedSession::capture(restored.state()), saved);
    }

    /// Produces the historical wire shape rather than labeling a current
    /// document with an older version number.
    fn legacy_document(saved: &SavedSession, version: u32) -> serde_json::Value {
        let mut value = serde_json::to_value(saved).unwrap();
        value["version"] = serde_json::json!(version);
        if version <= 2 {
            let returns = value["returns"].as_array_mut().unwrap();
            returns.truncate(8);
            for bus_return in returns {
                let object = bus_return.as_object_mut().unwrap();
                let effects = object.remove("effects").unwrap();
                object.remove("name");
                object.insert(
                    "effect".to_owned(),
                    effects
                        .as_array()
                        .unwrap()
                        .first()
                        .cloned()
                        .unwrap_or_default(),
                );
            }
            for track in value["mixer"]["tracks"].as_array_mut().unwrap() {
                track["sends"].as_array_mut().unwrap().truncate(8);
            }
        }
        for patch in value["patches"].as_array_mut().unwrap() {
            patch.as_object_mut().unwrap().remove("sends");
            if version == 1 {
                patch.as_object_mut().unwrap().remove("voiceLimit");
            }
        }
        value
    }

    #[test]
    fn legacy_eight_return_sessions_preserve_effects_levels_and_sends_when_extended() {
        use crate::adapter::chorus_capability::{ChorusCapability, CHORUS_AMOUNT_PARAMETER_ID};
        use crate::synth::{EffectCapabilityProvider, EffectSlotId};

        let active = state();
        let provider = ChorusCapability::new().unwrap();
        let descriptor = provider.descriptor();
        let effects = EffectCapabilityRegistry::new(vec![descriptor.clone()]).unwrap();
        let config = provider
            .default_config(EffectSlotId::new(3).unwrap())
            .unwrap()
            .with_scalar_value(
                &descriptor,
                &ParameterId::new(CHORUS_AMOUNT_PARAMETER_ID).unwrap(),
                ParameterValue::continuous(0.19).unwrap(),
            )
            .unwrap();
        let mut saved = SavedSession::capture(&active);
        saved.returns[2].effects.push(config.clone());
        saved.returns[2].return_level = 0.74;
        let track = MixerTrackId::default();
        saved.mixer.set_track(
            track,
            saved
                .mixer
                .track(track)
                .clone()
                .with_send(BusId::new(2).unwrap(), 0.61)
                .unwrap(),
        );

        for version in [1, 2] {
            let legacy = legacy_document(&saved, version);
            let migrated = SavedSession::from_json(
                &serde_json::to_string(&legacy).unwrap(),
                active.capabilities(),
            )
            .unwrap();
            let restored = migrated
                .restore_candidate(
                    active.capabilities().clone(),
                    effects.clone(),
                    GraphRevision::INITIAL,
                )
                .unwrap();
            assert_eq!(migrated.version(), SAVED_SESSION_VERSION);
            assert_eq!(restored.bus_returns().len(), DEFAULT_BUS_RETURNS);
            let expected_patch = if version == 1 {
                // Version 1 did not save a voice limit. Its existing migration
                // derives one from the capability ceiling, independently of sends.
                active.patches()[0]
                    .clone()
                    .with_voice_limit(
                        active
                            .capabilities()
                            .descriptor_for_config(active.patches()[0].instrument_config())
                            .unwrap()
                            .voice_policy()
                            .polyphony_ceiling(),
                    )
                    .unwrap()
            } else {
                active.patches()[0].clone()
            };
            let mut expected_sends = vec![0.0; DEFAULT_BUS_RETURNS];
            expected_sends[2] = 0.61;
            assert_eq!(
                restored.patches(),
                &[expected_patch.with_sends(expected_sends).unwrap()]
            );
            assert_eq!(
                restored
                    .bus_returns()
                    .bus_return(BusId::new(2).unwrap())
                    .effects(),
                std::slice::from_ref(&config)
            );
            assert_eq!(
                restored
                    .bus_returns()
                    .bus_return(BusId::new(2).unwrap())
                    .return_level(),
                0.74
            );
            assert_eq!(
                restored.mixer().track(track).send(BusId::new(2).unwrap()),
                0.0
            );
            assert_eq!(restored.patches()[0].send(BusId::new(2).unwrap()), 0.61);
            assert!(restored
                .bus_returns()
                .returns()
                .iter()
                .all(|value| value.name() == "INIT"));
            assert!(restored.bus_returns().returns()[8..]
                .iter()
                .all(|value| !value.is_occupied()));
            for track in restored.mixer().tracks() {
                assert_eq!(track.sends().len(), DEFAULT_BUS_RETURNS);
                assert!(track.sends()[8..].iter().all(|value| *value == 0.0));
            }
            assert_eq!(SavedSession::capture(&restored), migrated);
        }
    }

    #[test]
    fn return_restore_rejects_duplicate_identities_invalid_names_and_mismatched_routes() {
        use crate::adapter::chorus_capability::ChorusCapability;
        use crate::synth::{EffectCapabilityProvider, EffectSlotId};

        let active = state();
        let provider = ChorusCapability::new().unwrap();
        let effects = EffectCapabilityRegistry::new(vec![provider.descriptor()]).unwrap();
        let config = provider
            .default_config(EffectSlotId::new(1).unwrap())
            .unwrap();
        let mut saved = SavedSession::capture(&active);
        saved.returns[0].effects = vec![config.clone(), config];
        assert!(matches!(
            saved.restore_candidate(
                active.capabilities().clone(),
                effects.clone(),
                GraphRevision::INITIAL
            ),
            Err(SavedSessionError::InvalidReturn)
        ));
        saved.returns[0].effects.truncate(1);
        assert!(matches!(
            saved.restore_candidate(
                active.capabilities().clone(),
                EffectCapabilityRegistry::default(),
                GraphRevision::INITIAL
            ),
            Err(SavedSessionError::InvalidEffect)
        ));
        for name in [" ", "Room\nTwo"] {
            saved.returns[0].name = name.to_owned();
            assert!(matches!(
                saved.restore_candidate(
                    active.capabilities().clone(),
                    effects.clone(),
                    GraphRevision::INITIAL
                ),
                Err(SavedSessionError::InvalidReturn)
            ));
        }
        saved.returns[0].name = "Room".to_owned();
        saved.returns.pop();
        assert_eq!(saved.validate_shape(), Err(SavedSessionError::InvalidShape));
    }

    #[test]
    fn version_one_without_runtime_fields_migrates_explicitly() {
        let state = state();
        let value = legacy_document(&SavedSession::capture(&state), 1);
        let migrated = SavedSession::from_json(
            &serde_json::to_string(&value).unwrap(),
            state.capabilities(),
        )
        .unwrap();
        assert_eq!(migrated.version(), SAVED_SESSION_VERSION);
        assert!(migrated.to_json().unwrap().contains("voiceLimit"));
    }
}
