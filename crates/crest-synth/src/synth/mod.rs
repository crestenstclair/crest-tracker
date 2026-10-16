pub mod file_browser;
pub use file_browser::{
    AssetFileId, FileBrowserFolderId, FileBrowserListing, FileBrowserRow, FileBrowserRowKind,
};
pub mod capability_id;
pub mod capability_visualization;
pub mod descriptor_default_config_factory;
pub mod effect_capability;
pub mod effect_capability_id;
pub mod effect_capability_provider;
pub mod effect_composition;
pub mod effect_preparer;
pub mod effect_slot_id;
pub mod instrument_capability;
pub mod instrument_capability_provider;
pub mod instrument_composition;
pub mod instrument_preparer;
pub mod parameter_id;
pub mod patch;
pub mod prepared_audition;
pub mod prepared_engine_rack_builder;
pub mod prepared_instrument;
pub mod prepared_post_effect;
pub mod prepared_post_effect_rack_builder;
pub mod sample_asset;
pub mod sample_preparation;
pub mod sound_font_preset;

pub use capability_id::{CapabilityId, IdentifierError};
pub use capability_visualization::{
    CapabilityVisualization, WaveformLandmarkRole, WaveformLandmarkSpec,
};
pub use descriptor_default_config_factory::DescriptorDefaultConfigFactory;
pub use effect_capability::{
    EffectCapabilityDescriptor, EffectCapabilityError, EffectCapabilityRegistry, EffectCategory,
    PostEffectConfig, MAX_POST_EFFECTS_PER_PATCH,
};
pub use effect_capability_id::EffectCapabilityId;
pub use effect_capability_provider::EffectCapabilityProvider;
pub use effect_composition::{compose_effect_registry, EffectCompositionError};
pub use effect_preparer::{EffectPreparationError, EffectPreparer};
pub use effect_slot_id::{EffectSlotId, EffectSlotIdError};
pub use instrument_capability::{
    AssetAssignment, AssetKind, AssetReference, AssetRequirement, CapabilityAvailability,
    CapabilityDescriptor, CapabilityError, CapabilityRegistry, CapabilitySection,
    ContinuousValueLabel, InstrumentCategory, InstrumentConfig, ParameterAdjustment,
    ParameterAssignment, ParameterChoice, ParameterDefault, ParameterKind, ParameterPredicate,
    ParameterRange, ParameterSpec, ParameterUpdate, ParameterValue, PatchInteraction, VoicePolicy,
};
pub use instrument_capability_provider::InstrumentCapabilityProvider;
pub use instrument_composition::{compose_instrument_registry, InstrumentCompositionError};
pub use instrument_preparer::{InstrumentPreparationError, InstrumentPreparer};
pub use patch::{resolve_patch_editable_targets, Patch, PatchEditableTarget, VoiceLimitCarryOver};
pub use prepared_audition::PreparedAudition;
pub use prepared_engine_rack_builder::{PreparedEngineRackBuilder, RackPreparationError};
pub use prepared_instrument::{
    PreparedAssetFootprint, PreparedInstrument, PreparedInstrumentError,
};
pub use prepared_post_effect::{PreparedEffectError, PreparedPostEffect};
pub use prepared_post_effect_rack_builder::{
    EffectRackPreparationError, PreparedPostEffectRackBuilder,
};
pub use sample_asset::{
    DecodedSample, PreparedSampleLandmarks, PreparedSamplePcm, PreparedSampleVisualization,
    SampleAssetCatalogPort, SampleAssetError, SampleDecoderPort, SampleEncoding, SampleLoopMode,
    SampleMetadata, SamplePlaybackConfig, WaveformPair, MAX_LOOP_CROSSFADE_MILLISECONDS,
    MAX_SAMPLE_DURATION_SECONDS, MAX_SAMPLE_GRAPH_PCM_BYTES, MAX_SAMPLE_RATE, MAX_SAMPLE_SCALARS,
    MAX_SAMPLE_SOURCE_BYTES, MAX_WAVEFORM_PAIRS, MIN_SAMPLE_RATE, SAMPLE_VOICE_COUNT,
};
pub use sample_preparation::{
    prepare_sample_pcm, summarize_waveform, validate_sample_graph_budget,
};
pub mod sound_font_instrument;
pub mod voice_envelope;
pub mod voice_envelope_state;
pub mod voice_limit;
pub use parameter_id::ParameterId;
pub use sound_font_preset::{
    SoundFontPresetCatalog, SoundFontPresetCatalogEntry, SoundFontPresetCatalogError,
    SoundFontPresetCollision, SoundFontPresetId, SoundFontPresetIdError, SoundFontPresetSource,
};
pub use voice_envelope::{
    VoiceEnvelope, VoiceEnvelopeError, VoiceEnvelopeParameter, VoiceEnvelopeParameterDescriptor,
};
pub use voice_envelope_state::{VoiceEnvelopeStage, VoiceEnvelopeState};
pub use voice_limit::{
    VoiceLimit, VoiceLimitDescriptor, VoiceLimitError, ENGINE_MANAGED_POLYPHONY_CEILING,
    VOICE_LIMIT_FIELD,
};

pub use prepared_instrument::{
    MidiExpressionScope, MidiSource, MidiVoiceId, OwnedExpressionState, OwnedVoiceInitialization,
};
