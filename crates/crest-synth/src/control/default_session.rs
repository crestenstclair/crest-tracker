use crate::control::{AppEvent, AppState, EventRejection, SavedSession, TopLevelContext};
use crate::kernel::{MidiChannel, PatchId, MAX_ACTIVE_PATCHES};
use crate::mixer::bus_return::BusReturnBank;
use crate::mixer::global_parameters::GlobalParameters;
use crate::mixer::mixer_state::MixerState;
use crate::mixer::patch_output::PatchOutput;
use crate::synth::{
    CapabilityError, CapabilityId, DescriptorDefaultConfigFactory, EffectCapabilityRegistry,
    InstrumentConfig, Patch, PostEffectConfig, VoiceEnvelope, VoiceLimit, VoicePolicy,
};

/// Identity-free defaults used to inspect the trailing empty Patch position.
///
/// This value deliberately cannot enter saved state or the audio graph: it has
/// no `PatchId`, name, or guaranteed route. At active capacity the route fields
/// are absent while the product-authored Engine, envelope, voice policy, and
/// empty effect layout remain inspectable.
#[derive(Clone, Debug, PartialEq)]
pub struct ProspectivePatch {
    instrument_config: InstrumentConfig,
    channel: Option<MidiChannel>,
    envelope: VoiceEnvelope,
    output: Option<PatchOutput>,
    effects: [Option<PostEffectConfig>; crate::synth::effect_slot_id::MAX_EFFECT_SLOTS],
    voice_limit: VoiceLimit,
}

impl ProspectivePatch {
    pub const fn instrument_config(&self) -> &InstrumentConfig {
        &self.instrument_config
    }

    pub const fn channel(&self) -> Option<MidiChannel> {
        self.channel
    }

    pub const fn envelope(&self) -> &VoiceEnvelope {
        &self.envelope
    }

    pub const fn output(&self) -> Option<PatchOutput> {
        self.output
    }

    pub const fn effect_slots(
        &self,
    ) -> &[Option<PostEffectConfig>; crate::synth::effect_slot_id::MAX_EFFECT_SLOTS] {
        &self.effects
    }

    pub const fn voice_limit(&self) -> VoiceLimit {
        self.voice_limit
    }

    pub const fn creation_available(&self) -> bool {
        self.channel.is_some() && self.output.is_some()
    }
}

/// Immutable product-authored defaults shared by New and implicit Patch
/// creation. The exact provider-authored config is captured once; registry
/// order can never participate in later candidate construction.
#[derive(Clone, Debug, PartialEq)]
pub struct PatchCreationBlueprint {
    instrument_config: InstrumentConfig,
    voice_policy: VoicePolicy,
    installed_defaults: Vec<(InstrumentConfig, VoicePolicy)>,
    structural_choice_defaults: Vec<(crate::synth::ParameterId, String, InstrumentConfig)>,
}

impl PatchCreationBlueprint {
    pub fn resolve(
        instrument_capability_id: &CapabilityId,
        config_factory: &DescriptorDefaultConfigFactory,
    ) -> Result<Self, CapabilityError> {
        let instrument_config = config_factory.create(instrument_capability_id)?;
        let descriptor = config_factory
            .registry()
            .descriptor(instrument_capability_id)
            .ok_or_else(|| CapabilityError::UnknownCapability(instrument_capability_id.clone()))?;
        let voice_policy = descriptor.voice_policy();
        let mut installed_defaults = Vec::new();
        let mut structural_choice_defaults = Vec::new();
        for descriptor in config_factory.registry().descriptors() {
            let Ok(config) = config_factory.create(descriptor.id()) else {
                continue;
            };
            for spec in descriptor.parameters().filter(|spec| {
                spec.update() == crate::synth::ParameterUpdate::Structural
                    && spec.kind() == crate::synth::ParameterKind::Choice
            }) {
                for choice in spec.choices() {
                    if let Ok(candidate) =
                        config_factory.replace_structural_choice(&config, spec.id(), choice.id())
                    {
                        structural_choice_defaults.push((
                            spec.id().clone(),
                            choice.id().to_owned(),
                            candidate,
                        ));
                    }
                }
            }
            installed_defaults.push((config, descriptor.voice_policy()));
        }
        Ok(Self {
            instrument_config,
            voice_policy,
            installed_defaults,
            structural_choice_defaults,
        })
    }

    pub const fn instrument_config(&self) -> &InstrumentConfig {
        &self.instrument_config
    }

    pub const fn instrument_capability_id(&self) -> &CapabilityId {
        self.instrument_config.capability_id()
    }

    pub const fn voice_policy(&self) -> VoicePolicy {
        self.voice_policy
    }

    pub fn installed_default(
        &self,
        capability_id: &CapabilityId,
    ) -> Option<(&InstrumentConfig, VoicePolicy)> {
        self.installed_defaults
            .iter()
            .find(|(config, _)| config.capability_id() == capability_id)
            .map(|(config, policy)| (config, *policy))
    }

    /// Resolves identity-free prospective data for the empty position.
    pub fn prospective(
        &self,
        existing_count: usize,
        capability_id: &CapabilityId,
    ) -> Result<ProspectivePatch, PatchCreationError> {
        let (instrument_config, voice_policy) = self
            .installed_default(capability_id)
            .ok_or(PatchCreationError::DefaultUnavailable)?;
        let route = (existing_count < MAX_ACTIVE_PATCHES).then(|| {
            // Patch count is independent of MIDI channel/mixer lane count.
            // New patches cycle the existing routes; the user can edit either.
            (
                MidiChannel::new((existing_count % (usize::from(MidiChannel::MAX) + 1)) as u8)
                    .expect("valid MIDI route"),
                crate::mixer::mixer_track_id::MixerTrackId::ALL
                    [existing_count % crate::mixer::mixer_track_id::MixerTrackId::COUNT],
            )
        });
        Ok(ProspectivePatch {
            instrument_config: instrument_config.clone(),
            channel: route.map(|(channel, _)| channel),
            envelope: VoiceEnvelope::default(),
            output: route.map(|(_, track)| PatchOutput::to_track(track)),
            effects: std::array::from_fn(|_| None),
            voice_limit: VoiceLimit::seeded_from(voice_policy),
        })
    }

    /// Constructs the deterministic prospective Patch without installing it.
    pub fn candidate(&self, existing: &[Patch]) -> Result<Patch, PatchCreationError> {
        self.candidate_with_capability(existing, self.instrument_capability_id())
    }

    pub fn candidate_with_capability(
        &self,
        existing: &[Patch],
        capability_id: &CapabilityId,
    ) -> Result<Patch, PatchCreationError> {
        if existing.len() >= MAX_ACTIVE_PATCHES {
            return Err(PatchCreationError::CapacityReached);
        }
        let prospective = self.prospective(existing.len(), capability_id)?;
        let next_raw = existing
            .iter()
            .map(|patch| patch.id().value())
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(PatchCreationError::IdentityExhausted)?;
        let patch_id = PatchId::new(next_raw).map_err(|_| PatchCreationError::IdentityExhausted)?;
        let mut patch = Patch::new(
            patch_id,
            format!("{:02}", existing.len()),
            prospective.instrument_config().clone(),
            prospective
                .channel()
                .ok_or(PatchCreationError::CapacityReached)?,
            prospective
                .output()
                .ok_or(PatchCreationError::CapacityReached)?,
        );
        patch.seed_voice_limit(
            self.installed_default(capability_id)
                .ok_or(PatchCreationError::DefaultUnavailable)?
                .1,
        );
        Ok(patch)
    }

    pub fn candidate_with_structural_choice(
        &self,
        existing: &[Patch],
        parameter_id: &crate::synth::ParameterId,
        choice_id: &str,
    ) -> Result<Patch, PatchCreationError> {
        let config = self
            .structural_choice_defaults
            .iter()
            .find(|(candidate_parameter, candidate_choice, config)| {
                candidate_parameter == parameter_id
                    && candidate_choice == choice_id
                    && config.capability_id() == self.instrument_capability_id()
            })
            .map(|(_, _, config)| config.clone())
            .ok_or(PatchCreationError::DefaultUnavailable)?;
        let mut candidate = self.candidate(existing)?;
        candidate.replace_instrument_config(config, self.voice_policy());
        Ok(candidate)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PatchCreationError {
    #[error("the prepared audio graph capacity of {MAX_ACTIVE_PATCHES} Patches is reached")]
    CapacityReached,
    #[error("the greatest Patch identity has no representable successor")]
    IdentityExhausted,
    #[error("the selected Engine has no exact provider-authored creation default")]
    DefaultUnavailable,
}

/// Product-authored recipe for the one canonical session created at startup
/// and by New. Registry order is deliberately absent: the instrument is
/// resolved only by this exact stable capability identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DefaultSessionBlueprint {
    instrument_capability_id: CapabilityId,
}

impl DefaultSessionBlueprint {
    pub const PATCH_NAME: &'static str = "00";

    pub const fn new(instrument_capability_id: CapabilityId) -> Self {
        Self {
            instrument_capability_id,
        }
    }

    pub const fn instrument_capability_id(&self) -> &CapabilityId {
        &self.instrument_capability_id
    }
}

/// Failure to construct the declared default. No variant authorizes choosing
/// a different installed capability or substituting a partial session.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum DefaultSessionError {
    #[error("the declared default instrument could not produce its canonical config: {0}")]
    Capability(#[from] CapabilityError),
    #[error("the declared default session was rejected by canonical state: {0}")]
    State(#[from] EventRejection),
}

/// Captures the canonical default directly into the existing versioned
/// persistence boundary. Callers prepare this `SavedSession` through the same
/// candidate path used by Open before making it active.
pub fn capture_default_session(
    blueprint: &DefaultSessionBlueprint,
    config_factory: &DescriptorDefaultConfigFactory,
    effects: EffectCapabilityRegistry,
    returns: BusReturnBank,
) -> Result<SavedSession, DefaultSessionError> {
    let creation =
        PatchCreationBlueprint::resolve(blueprint.instrument_capability_id(), config_factory)?;
    let mut patch = Patch::new(
        PatchId::new(1).expect("the product-authored default Patch identity is non-zero"),
        DefaultSessionBlueprint::PATCH_NAME.to_owned(),
        creation.instrument_config().clone(),
        MidiChannel::new(0).expect("the product-authored default MIDI channel is valid"),
        PatchOutput::default(),
    );
    patch.seed_voice_limit(creation.voice_policy());
    let mut state = AppState::new_with_effects(
        config_factory.registry().clone(),
        effects,
        GlobalParameters::new(0.0)
            .expect("the product-authored neutral master gain is within its descriptor"),
    )
    .with_initial_mixer(MixerState::default())
    .with_initial_returns(returns);
    state.apply(AppEvent::InstallPatches(vec![patch]))?;
    // Installation initializes both remembered contexts and leaves MIXER
    // active for legacy fixtures. The product default explicitly opens PATCH;
    // the remembered first row is the stable Engine identity.
    state.apply(AppEvent::SelectContext(TopLevelContext::Patch))?;
    Ok(SavedSession::capture(&state))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::hidef_soundfont_capability::HIDEF_CAPABILITY_ID;
    use crate::adapter::lock_free_audio_boundary::LockFreeAudioBoundary;
    use crate::adapter::lock_free_structural_graph_boundary::LockFreeStructuralGraphBoundary;
    use crate::adapter::production_effects::{
        production_effect_preparers, production_effect_registry, production_startup_bus_returns,
    };
    use crate::adapter::production_instruments::{
        production_capability_registry, production_instrument_preparers,
        production_instrument_providers,
    };
    use crate::control::{
        AppEvent, AppLoop, Direction, EventRejection, EventSource, InteractionMode, PatchControlId,
        SavedSession, SavedSessionRestoreError, SessionReplacementPayload, StateProjector,
    };
    use crate::kernel::midi_message::{MidiMessage, MidiMessageKind};
    use crate::mixer::mixer_track_id::MixerTrackId;
    use crate::real_time::{
        AudioBoundary, AudioRenderer, GraphHandoffStatus, GraphRevision, StructuralGraphBoundary,
    };
    use crate::synth::{CapabilityRegistry, VoiceEnvelope, VoiceLimit};

    fn blueprint() -> DefaultSessionBlueprint {
        DefaultSessionBlueprint::new(CapabilityId::new(HIDEF_CAPABILITY_ID).unwrap())
    }

    fn creation_blueprint() -> PatchCreationBlueprint {
        let registry = production_capability_registry().unwrap();
        PatchCreationBlueprint::resolve(
            blueprint().instrument_capability_id(),
            &DescriptorDefaultConfigFactory::new(
                registry,
                production_instrument_providers().unwrap(),
            ),
        )
        .unwrap()
    }

    fn prepare(
        providers: Vec<Box<dyn crate::synth::InstrumentCapabilityProvider>>,
        registry: CapabilityRegistry,
    ) -> Result<crate::control::PreparedSavedSession, SavedSessionRestoreError> {
        let effects = production_effect_registry().unwrap();
        let returns = production_startup_bus_returns(&effects).unwrap();
        let saved = capture_default_session(
            &blueprint(),
            &DescriptorDefaultConfigFactory::new(registry.clone(), providers),
            effects.clone(),
            returns,
        )
        .unwrap();
        saved.prepare_restore(
            registry,
            effects,
            &production_instrument_preparers().unwrap(),
            &production_effect_preparers().unwrap(),
            GraphRevision::INITIAL,
            48_000.0,
            1_024,
        )
    }

    #[test]
    fn canonical_default_is_exact_and_prepares_through_the_saved_session_path() {
        let prepared = prepare(
            production_instrument_providers().unwrap(),
            production_capability_registry().unwrap(),
        )
        .unwrap();
        let state = prepared.state();
        assert_eq!(state.patches().len(), 1);
        let patch = &state.patches()[0];
        assert_eq!(patch.id(), PatchId::new(1).unwrap());
        assert_eq!(patch.name(), "00");
        assert_eq!(
            patch.instrument_config().capability_id().as_str(),
            HIDEF_CAPABILITY_ID
        );
        assert_eq!(patch.channel(), MidiChannel::new(0).unwrap());
        assert_eq!(patch.output(), PatchOutput::default());
        assert_eq!(patch.envelope(), &VoiceEnvelope::default());
        assert_eq!(
            patch.voice_limit().value(),
            VoiceLimit::seeded_from(
                state
                    .capabilities()
                    .descriptor_for_config(patch.instrument_config())
                    .unwrap()
                    .voice_policy()
            )
            .value()
        );
        assert!(patch.effect_slots().iter().all(Option::is_none));
        assert_eq!(state.mixer(), &MixerState::default());
        assert_eq!(state.global().master_gain_db(), 0.0);
        assert_eq!(state.bus_returns().returns().len(), 16);
        assert!(state
            .bus_returns()
            .returns()
            .iter()
            .all(|bus_return| { bus_return.name() == "INIT" && bus_return.effects().is_empty() }));
        assert_eq!(patch.output().track_id(), MixerTrackId::new(0).unwrap());
        assert_eq!(state.context(), TopLevelContext::Mixer);
        // Preparation reconstructs persisted content only. The atomic
        // replacement event owns the required PATCH/Engine reset.
        assert_eq!(
            state.interaction().patch_control_focus(),
            Some(PatchControlId::Engine)
        );
    }

    #[test]
    fn registry_and_provider_order_cannot_change_the_declared_default() {
        let mut providers = production_instrument_providers().unwrap();
        providers.reverse();
        let mut descriptors = production_capability_registry()
            .unwrap()
            .descriptors()
            .to_vec();
        descriptors.reverse();
        let registry = CapabilityRegistry::new(descriptors).unwrap();
        let prepared = prepare(providers, registry).unwrap();
        assert_eq!(
            prepared.state().patches()[0]
                .instrument_config()
                .capability_id()
                .as_str(),
            HIDEF_CAPABILITY_ID
        );
    }

    #[test]
    fn deterministic_creation_candidate_uses_monotonic_identity_and_append_routing() {
        let blueprint = creation_blueprint();
        let existing = Patch::new(
            PatchId::new(7).unwrap(),
            "Existing".to_owned(),
            blueprint.instrument_config().clone(),
            MidiChannel::new(0).unwrap(),
            PatchOutput::default(),
        );
        let candidate = blueprint.candidate(&[existing]).unwrap();

        assert_eq!(candidate.id(), PatchId::new(8).unwrap());
        assert_eq!(candidate.name(), "01");
        assert_eq!(candidate.channel(), MidiChannel::new(1).unwrap());
        assert_eq!(candidate.output().track_id(), MixerTrackId::new(1).unwrap());
        assert_eq!(candidate.output().trim_gain_db(), 0.0);
        assert_eq!(candidate.envelope(), &VoiceEnvelope::default());
        assert_eq!(
            candidate.voice_limit(),
            VoiceLimit::seeded_from(blueprint.voice_policy())
        );
        assert!(candidate.effect_slots().iter().all(Option::is_none));
    }

    #[test]
    fn sparse_identity_capacity_and_exhaustion_are_checked_independently() {
        let blueprint = creation_blueprint();
        let fixture = |id: u32, channel: u8| {
            Patch::new(
                PatchId::new(id).unwrap(),
                format!("Fixture {id}"),
                blueprint.instrument_config().clone(),
                MidiChannel::new(channel).unwrap(),
                PatchOutput::to_track(MixerTrackId::new(channel).unwrap()),
            )
        };
        let sparse = vec![fixture(2, 0), fixture(11, 1)];
        let candidate = blueprint.candidate(&sparse).unwrap();
        assert_eq!(candidate.id(), PatchId::new(12).unwrap());
        assert_eq!(candidate.channel(), MidiChannel::new(2).unwrap());

        let almost_full = (0..MAX_ACTIVE_PATCHES - 1)
            .map(|index| fixture((index + 1) as u32, (index % 16) as u8))
            .collect::<Vec<_>>();
        let last = blueprint.candidate(&almost_full).unwrap();
        assert_eq!(last.id(), PatchId::new(MAX_ACTIVE_PATCHES as u32).unwrap());
        assert_eq!(
            last.channel(),
            MidiChannel::new(((MAX_ACTIVE_PATCHES - 1) % 16) as u8).unwrap()
        );
        assert_eq!(
            last.output().track_id(),
            MixerTrackId::new(((MAX_ACTIVE_PATCHES - 1) % 16) as u8).unwrap()
        );

        let full = (0..MAX_ACTIVE_PATCHES)
            .map(|index| fixture((index + 1) as u32, (index % 16) as u8))
            .collect::<Vec<_>>();
        assert_eq!(
            blueprint.candidate(&full),
            Err(PatchCreationError::CapacityReached)
        );
        assert_eq!(
            blueprint.candidate(&[fixture(u32::MAX, 0)]),
            Err(PatchCreationError::IdentityExhausted)
        );
    }

    #[test]
    fn missing_declared_capability_is_typed_and_never_substituted() {
        let providers = production_instrument_providers().unwrap();
        let registry = CapabilityRegistry::new(
            production_capability_registry()
                .unwrap()
                .descriptors()
                .iter()
                .filter(|descriptor| descriptor.id().as_str() != HIDEF_CAPABILITY_ID)
                .cloned()
                .collect(),
        )
        .unwrap();
        let effects = production_effect_registry().unwrap();
        let error = capture_default_session(
            &blueprint(),
            &DescriptorDefaultConfigFactory::new(registry, providers),
            effects.clone(),
            production_startup_bus_returns(&effects).unwrap(),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            DefaultSessionError::Capability(CapabilityError::UnknownCapability(ref id))
                if id.as_str() == HIDEF_CAPABILITY_ID
        ));
    }

    #[test]
    fn canonical_default_accepts_physical_midi_and_renders_only_after_input() {
        let prepared = prepare(
            production_instrument_providers().unwrap(),
            production_capability_registry().unwrap(),
        )
        .unwrap();
        let capabilities = prepared.state().capabilities().clone();
        let effects = prepared.state().effects().clone();
        let (replacement, graph) = prepared.into_replacement();
        let parameters = graph.initial_parameters().clone();
        let mut state = AppState::for_graph_with_effects(
            capabilities,
            effects,
            GlobalParameters::new(0.0).unwrap(),
            GraphRevision::INITIAL,
        );
        state
            .apply(AppEvent::ReplacePersistedSession(Box::new(replacement)))
            .unwrap();
        let (control, audio) = LockFreeAudioBoundary::new(16, parameters).into_handles();
        let (structural_control, structural_audio) = LockFreeStructuralGraphBoundary::new(
            1,
            1,
            GraphHandoffStatus::with_active(GraphRevision::INITIAL),
        )
        .unwrap()
        .into_handles();
        let mut app = AppLoop::new(
            state,
            StateProjector::for_graph(GraphRevision::INITIAL),
            control,
        )
        .unwrap();
        let audio_config = crate::shell::audio_output::AudioDeviceConfig::new(
            48_000.0,
            2,
            crate::shell::audio_output::AudioSampleFormat::F32,
            1_024,
        )
        .unwrap();
        let runtime_capabilities = app.capabilities().clone();
        let runtime_effects = app.effects().clone();
        app.configure_engine_selection(
            DescriptorDefaultConfigFactory::new(
                runtime_capabilities.clone(),
                production_instrument_providers().unwrap(),
            ),
            crate::testing::DeterministicGraphPreparationWorker::new_with_effects(
                runtime_capabilities,
                production_instrument_preparers().unwrap(),
                runtime_effects,
                production_effect_preparers().unwrap(),
                audio_config,
            ),
            structural_control,
            &graph,
            audio_config,
        )
        .unwrap();
        let mut renderer = AudioRenderer::new(audio, structural_audio, graph);
        let mut output = [0.0_f32; 2_048];
        renderer.render(&mut output);
        assert!(output.iter().all(|sample| sample.abs() <= f32::EPSILON));

        let result = app
            .dispatch_midi_from(
                MidiMessage::try_new(
                    MidiChannel::new(0).unwrap(),
                    MidiMessageKind::NoteOn,
                    60,
                    100,
                )
                .unwrap(),
                EventSource::PhysicalMidi,
            )
            .unwrap();
        assert_eq!(result.subscriber_count(), 1);
        output.fill(0.0);
        renderer.render(&mut output);
        assert!(output.iter().any(|sample| sample.abs() > 0.000_001));
    }

    #[test]
    fn whole_session_event_replaces_persisted_content_and_preserves_runtime_atomically() {
        let initial = prepare(
            production_instrument_providers().unwrap(),
            production_capability_registry().unwrap(),
        )
        .unwrap();
        let mut active = initial.state().clone();
        active.apply(AppEvent::MidiInputScanStarted).unwrap();
        let midi_runtime = active.midi_input().clone();
        let before = SavedSession::capture(&active);

        let mut edited = active.clone();
        edited
            .apply(AppEvent::SetInteractionMode(InteractionMode::Adjust))
            .unwrap();
        edited.apply(AppEvent::Adjust(Direction::Down)).unwrap();
        edited
            .apply(AppEvent::SetPatchCapacity { capacity: 2 })
            .unwrap();
        let expected = SavedSession::capture(&edited);
        assert_ne!(expected, before);
        let target_revision = GraphRevision::new(2).unwrap();
        let prepared = expected
            .prepare_restore(
                active.capabilities().clone(),
                active.effects().clone(),
                &production_instrument_preparers().unwrap(),
                &production_effect_preparers().unwrap(),
                target_revision,
                48_000.0,
                1_024,
            )
            .unwrap();
        let (replacement, _graph) = prepared.into_replacement();
        let generation = active.generation();
        let outcome = active
            .apply(AppEvent::ReplacePersistedSession(Box::new(replacement)))
            .unwrap();

        assert_eq!(SavedSession::capture(&active), expected);
        assert_eq!(active.context(), TopLevelContext::Patch);
        assert_eq!(
            active.interaction().patch_control_focus(),
            Some(PatchControlId::Engine)
        );
        assert_eq!(active.midi_input(), &midi_runtime);
        assert_eq!(
            active.engine_selection().active_graph_revision(),
            target_revision
        );
        assert_eq!(outcome.accepted().generation(), generation + 1);
        assert!(outcome.accepted().saved_session_changed());
    }

    #[test]
    fn invalid_whole_session_event_rejects_without_changing_any_projected_state() {
        let initial = prepare(
            production_instrument_providers().unwrap(),
            production_capability_registry().unwrap(),
        )
        .unwrap();
        let mut active = initial.state().clone();
        active.apply(AppEvent::MidiInputScanStarted).unwrap();
        let before = StateProjector::for_graph(GraphRevision::INITIAL)
            .project(&active)
            .unwrap()
            .0;
        let generation = active.generation();
        let empty = AppState::new_with_effects(
            active.capabilities().clone(),
            active.effects().clone(),
            GlobalParameters::new(-6.0).unwrap(),
        );
        let replacement = SessionReplacementPayload::from_prepared_state(
            &empty,
            GraphRevision::new(2).unwrap(),
            Default::default(),
        );

        assert_eq!(
            active.apply(AppEvent::ReplacePersistedSession(Box::new(replacement))),
            Err(EventRejection::NoPatchesInstalled)
        );
        assert_eq!(active.generation(), generation);
        assert_eq!(
            StateProjector::for_graph(GraphRevision::INITIAL)
                .project(&active)
                .unwrap()
                .0,
            before
        );
    }
}
