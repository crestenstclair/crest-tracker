use crest_synth::adapter::braids_capability::{
    BraidsCapability, BRAIDS_CAPABILITY_ID, BRAIDS_COLOR_PARAMETER_ID, BRAIDS_MODELS,
    BRAIDS_MODEL_PARAMETER_ID, BRAIDS_TIMBRE_PARAMETER_ID,
};
use crest_synth::adapter::hidef_soundfont_capability::HIDEF_SOUNDFONT_PATH;
use crest_synth::adapter::hidef_soundfont_capability::{
    HIDEF_CAPABILITY_ID, SOUNDFONT_FILE_PARAMETER_ID, SOUNDFONT_PRESET_PARAMETER_ID,
};
use crest_synth::adapter::lock_free_audio_boundary::LockFreeAudioBoundary;
use crest_synth::adapter::production_instruments::production_capability_registry;
use crest_synth::control::app_event::{AppEvent, Direction};
use crest_synth::control::app_loop::AppLoop;
use crest_synth::control::app_state::{AppState, EventRejection};
use crest_synth::control::event_record::EventSource;
use crest_synth::control::state_projector::StateProjector;
use crest_synth::control::StateTree;
use crest_synth::control::TopLevelContext;
use crest_synth::kernel::midi_channel::MidiChannel;
use crest_synth::kernel::midi_message::MidiMessageKind;
use crest_synth::kernel::patch_id::PatchId;
use crest_synth::mixer::global_parameters::GlobalParameters;
use crest_synth::mixer::mixer_state::MixerState;
use crest_synth::mixer::mixer_track_id::MixerTrackId;
use crest_synth::mixer::patch_output::PatchOutput;
use crest_synth::real_time::audio_boundary::AudioBoundary;
use crest_synth::real_time::parameter_snapshot::ParameterSnapshot;
use crest_synth::synth::sound_font_instrument::SoundFontInstrument;
use crest_synth::synth::{
    CapabilityDescriptor, CapabilityError, CapabilityId, CapabilityRegistry, CapabilitySection,
    CapabilityVisualization, InstrumentCapabilityProvider, InstrumentConfig, ParameterAssignment,
    ParameterDefault, ParameterId, ParameterKind, ParameterPredicate, ParameterRange,
    ParameterSpec, ParameterUpdate, ParameterValue, Patch, PatchInteraction, VoicePolicy,
};
use crest_synth::testing::automatic_midi_test::create_soundfont_config;
use serde_json::Value;

fn globals() -> GlobalParameters {
    GlobalParameters::new(-3.0).unwrap()
}

fn parameter_id(value: &str) -> ParameterId {
    ParameterId::new(value).unwrap()
}

fn patch(id: u32, channel: u8, name: &str, config: InstrumentConfig) -> Patch {
    Patch::new(
        PatchId::new(id).unwrap(),
        name.to_owned(),
        config,
        MidiChannel::new(channel).unwrap(),
        PatchOutput::to_track(MixerTrackId::new(channel).unwrap()),
    )
}

fn app_loop(
    registry: CapabilityRegistry,
) -> (
    AppLoop<crest_synth::adapter::lock_free_audio_boundary::LockFreeControlHandle>,
    crest_synth::adapter::lock_free_audio_boundary::LockFreeAudioHandle,
) {
    let initial = ParameterSnapshot::new(0, globals(), MixerState::default(), &[]).unwrap();
    let boundary = LockFreeAudioBoundary::new(16, initial);
    let (control, audio) = boundary.into_handles();
    (
        AppLoop::new(
            AppState::new(registry, globals()),
            StateProjector::new(),
            control,
        )
        .unwrap(),
        audio,
    )
}

fn candidate(valid: &InstrumentConfig, values: Vec<ParameterAssignment>) -> InstrumentConfig {
    InstrumentConfig::from_parts(
        valid.capability_id().clone(),
        values,
        valid.asset_references().to_vec(),
    )
}

#[test]
fn instrument_and_effect_descriptors_share_ordered_generic_visualization_metadata() {
    let instruments = production_capability_registry().unwrap();
    for descriptor in instruments.descriptors() {
        assert!(matches!(
            descriptor.visualizations().first(),
            Some(CapabilityVisualization::Envelope { id, label })
                if id == "shared.envelope" && label == "Envelope"
        ));
    }

    let effect = crest_synth::adapter::production_effects::production_effect_registry()
        .unwrap()
        .descriptors()[0]
        .clone()
        .with_visualizations([CapabilityVisualization::status("shared.status", "Status").unwrap()])
        .unwrap();
    assert!(matches!(
        effect.visualizations(),
        [CapabilityVisualization::Status { id, label }]
            if id == "shared.status" && label == "Status"
    ));
}

#[test]
fn capability_schema_is_exact_generic_and_rejected_without_fallback() {
    let provider =
        crest_synth::adapter::production_instruments::production_soundfont_capability().unwrap();
    let descriptor = provider.descriptor();
    let braids_provider = BraidsCapability::new().unwrap();
    let braids_descriptor = braids_provider.descriptor();
    let registry = production_capability_registry().unwrap();

    assert_eq!(
        registry.descriptors().len(),
        4 + crest_synth::adapter::upstream_audio::instrument_ports()
            .unwrap()
            .len()
    );
    assert_eq!(registry.descriptors()[0], descriptor);
    assert_eq!(registry.descriptors()[1], braids_descriptor);
    assert_eq!(
        registry.descriptors()[2].id().as_str(),
        crest_synth::adapter::sample_capability::SAMPLE_CAPABILITY_ID
    );
    assert_eq!(
        registry.descriptors()[3].id().as_str(),
        crest_synth::adapter::drum_rack_capability::DRUM_RACK_CAPABILITY_ID
    );
    assert_eq!(descriptor.id().as_str(), HIDEF_CAPABILITY_ID);
    assert_eq!(descriptor.label(), "HiDef SoundFont");
    assert_eq!(descriptor.semantic_accent(), "instrument.soundfont");
    assert_eq!(
        descriptor.voice_policy(),
        VoicePolicy::Configurable { default_voices: 64 }
    );
    assert_eq!(
        descriptor.supported_midi_kinds(),
        MidiMessageKind::ALL
            .iter()
            .copied()
            .filter(|kind| *kind != MidiMessageKind::ProgramChange)
            .collect::<Vec<_>>()
    );
    let parameters = descriptor.sections()[0].parameters();
    assert_eq!(
        parameters
            .iter()
            .map(|parameter| parameter.id().as_str())
            .collect::<Vec<_>>(),
        [SOUNDFONT_PRESET_PARAMETER_ID, SOUNDFONT_FILE_PARAMETER_ID,]
    );
    assert!(parameters
        .iter()
        .all(|parameter| parameter.update() == ParameterUpdate::Structural));
    let catalog = crest_synth::adapter::production_instruments::production_soundfont_asset()
        .unwrap()
        .catalog();
    assert_eq!(parameters[0].kind(), ParameterKind::Choice);
    assert_eq!(
        parameters[0].patch_interaction(),
        PatchInteraction::StructuralChoice
    );
    assert_eq!(parameters[0].choices().len(), catalog.entries().len());
    assert_eq!(parameters[1].kind(), ParameterKind::Asset);
    assert_eq!(
        parameters[1].patch_interaction(),
        PatchInteraction::ReadOnly
    );
    assert_eq!(
        parameters[1].default_value(),
        &ParameterDefault::Asset(
            crest_synth::synth::AssetReference::new(
                crest_synth::synth::AssetKind::SoundFont,
                HIDEF_SOUNDFONT_PATH,
            )
            .unwrap()
        )
    );

    assert_eq!(braids_descriptor.id().as_str(), BRAIDS_CAPABILITY_ID);
    assert_eq!(
        braids_descriptor.voice_policy(),
        VoicePolicy::Configurable { default_voices: 16 }
    );
    assert!(braids_descriptor.asset_requirements().is_empty());
    assert_eq!(braids_descriptor.scalar_parameter_count(), 3);
    let braids_parameters = braids_descriptor.sections()[0].parameters();
    assert_eq!(
        braids_parameters
            .iter()
            .map(|parameter| parameter.id().as_str())
            .collect::<Vec<_>>(),
        [
            BRAIDS_MODEL_PARAMETER_ID,
            BRAIDS_TIMBRE_PARAMETER_ID,
            BRAIDS_COLOR_PARAMETER_ID,
        ]
    );
    assert_eq!(braids_parameters[0].choices().len(), BRAIDS_MODELS.len());
    assert!(braids_parameters
        .iter()
        .all(|parameter| parameter.update() == ParameterUpdate::Scalar));

    let lead = create_soundfont_config(&provider, SoundFontInstrument::new(0, 80, false).unwrap())
        .unwrap();
    let drums = create_soundfont_config(&provider, SoundFontInstrument::new(128, 0, true).unwrap())
        .unwrap();
    registry.validate_config(&lead).unwrap();
    registry.validate_config(&drums).unwrap();
    let braids = braids_provider.default_config().unwrap();
    registry.validate_config(&braids).unwrap();
    assert_ne!(lead, drums);
    assert_eq!(
        lead.value(&parameter_id(SOUNDFONT_PRESET_PARAMETER_ID)),
        Some(&ParameterValue::Choice(
            SoundFontInstrument::new(0, 80, false)
                .unwrap()
                .preset_id()
                .choice_id()
        ))
    );
    assert_eq!(
        drums.value(&parameter_id(SOUNDFONT_PRESET_PARAMETER_ID)),
        Some(&ParameterValue::Choice(
            SoundFontInstrument::new(128, 0, true)
                .unwrap()
                .preset_id()
                .choice_id()
        ))
    );

    let (mut installed, _audio) = app_loop(registry.clone());
    installed
        .dispatch_from(
            AppEvent::InstallPatches(vec![
                patch(1, 0, "Lead", lead.clone()),
                patch(2, 1, "Braids", braids.clone()),
            ]),
            EventSource::Startup,
        )
        .unwrap();
    let tree: Value = serde_json::from_str(installed.current_state_tree().json()).unwrap();
    assert_eq!(tree["schemaVersion"], StateTree::SCHEMA_VERSION);
    assert_eq!(tree["parameters"]["graphRevision"], 1);
    assert_eq!(
        tree["capabilities"]["descriptors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|descriptor| descriptor["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        registry
            .descriptors()
            .iter()
            .map(|descriptor| descriptor.id().as_str())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        tree["patches"][0]["instrument"],
        serde_json::to_value(&lead).unwrap()
    );
    assert_eq!(
        tree["patches"][1]["instrument"],
        serde_json::to_value(&braids).unwrap()
    );
    assert_eq!(
        installed.patches()[0]
            .editable_targets(&descriptor)
            .unwrap()
            .len(),
        4
    );
    assert_eq!(
        installed.patches()[1]
            .editable_targets(&braids_descriptor)
            .unwrap()
            .len(),
        7
    );
    assert!(installed
        .current_parameters()
        .patch(PatchId::new(1).unwrap())
        .unwrap()
        .instrument()
        .values()
        .is_empty());
    assert_eq!(
        installed
            .current_parameters()
            .patch(PatchId::new(2).unwrap())
            .unwrap()
            .instrument()
            .values(),
        &[0.0, 0.5, 0.5]
    );
    installed
        .dispatch_from(
            AppEvent::SelectContext(TopLevelContext::Patch),
            EventSource::Keyboard,
        )
        .unwrap();
    let text = installed.current_text().body().to_owned();
    let preset = text.find(SOUNDFONT_PRESET_PARAMETER_ID).unwrap();
    let file = text.find(SOUNDFONT_FILE_PARAMETER_ID).unwrap();
    assert!(preset < file);
    assert!(!text.contains(BRAIDS_MODEL_PARAMETER_ID));
    assert!(!text.contains(BRAIDS_TIMBRE_PARAMETER_ID));
    assert!(!text.contains(BRAIDS_COLOR_PARAMETER_ID));
    assert!(!text.lines().any(|line| {
        line.starts_with('>')
            && (line.contains(SOUNDFONT_FILE_PARAMETER_ID) || line.contains("SoundFont File"))
    }));

    let valid_values = lead.values().to_vec();
    let mut duplicate = valid_values.clone();
    duplicate.push(valid_values[0].clone());
    assert!(matches!(
        registry.validate_config(&candidate(&lead, duplicate)),
        Err(CapabilityError::DuplicateAssignment(_))
    ));
    assert!(matches!(
        registry.validate_config(&candidate(&lead, valid_values[1..].to_vec())),
        Err(CapabilityError::MissingParameter(_))
    ));
    let mut undeclared = valid_values.clone();
    undeclared.push(ParameterAssignment::new(
        parameter_id("soundfont.unknown"),
        ParameterValue::Stepped(1),
    ));
    assert!(matches!(
        registry.validate_config(&candidate(&lead, undeclared)),
        Err(CapabilityError::UndeclaredParameter(_))
    ));
    let mut wrong_kind = valid_values.clone();
    wrong_kind[0] = ParameterAssignment::new(
        parameter_id(SOUNDFONT_PRESET_PARAMETER_ID),
        ParameterValue::Toggle(false),
    );
    assert!(matches!(
        registry.validate_config(&candidate(&lead, wrong_kind)),
        Err(CapabilityError::WrongValueKind(_))
    ));
    let mut unknown_choice = valid_values;
    unknown_choice[0] = ParameterAssignment::new(
        parameter_id(SOUNDFONT_PRESET_PARAMETER_ID),
        ParameterValue::Choice("sf2.bank-65535.program-127".to_owned()),
    );
    assert!(matches!(
        registry.validate_config(&candidate(&lead, unknown_choice)),
        Err(CapabilityError::UnknownChoice(_))
    ));
    let mut non_finite = braids.values().to_vec();
    non_finite[1] = ParameterAssignment::new(
        parameter_id(BRAIDS_TIMBRE_PARAMETER_ID),
        ParameterValue::Continuous(f64::NAN),
    );
    assert!(matches!(
        registry.validate_config(&candidate(&braids, non_finite)),
        Err(CapabilityError::NonFiniteContinuousValue)
    ));
    let missing_asset = InstrumentConfig::from_parts(
        lead.capability_id().clone(),
        lead.values().to_vec(),
        Vec::new(),
    );
    assert!(matches!(
        descriptor.create_config(missing_asset.values(), missing_asset.asset_references()),
        Err(CapabilityError::MissingAsset(_))
    ));
    // Asset-scoped SoundFont catalog resolution rejects the missing bank
    // before its descriptor can validate individual assignments.
    assert_eq!(
        registry.validate_config(&missing_asset),
        Err(CapabilityError::UnknownCapability(
            lead.capability_id().clone()
        ))
    );
    let unknown = InstrumentConfig::from_parts(
        CapabilityId::new("instrument.unknown").unwrap(),
        Vec::new(),
        Vec::new(),
    );
    assert!(matches!(
        registry.validate_config(&unknown),
        Err(CapabilityError::UnknownCapability(_))
    ));

    let (mut rejected, _audio) = app_loop(registry.clone());
    let before_tree = rejected.current_state_tree();
    let before_text = rejected.current_text();
    assert_eq!(
        rejected.dispatch_from(
            AppEvent::InstallPatches(vec![patch(9, 4, "Invalid", unknown)]),
            EventSource::Startup,
        ),
        Err(EventRejection::InvalidInstrumentConfig)
    );
    assert_eq!(rejected.current_state_tree(), before_tree);
    assert_eq!(rejected.current_text(), before_text);
    let rejected_log = rejected.event_log();
    let rejection = &rejected_log.records()[0];
    assert_eq!(
        rejection.rejection(),
        Some(EventRejection::InvalidInstrumentConfig)
    );
    assert!(rejection.emitted_events().is_empty());
    rejected
        .dispatch_from(AppEvent::Navigate(Direction::Down), EventSource::System)
        .unwrap();
    assert_eq!(rejected.current_state_tree().generation(), 1);

    let dependency_descriptor = dependency_descriptor();
    let dependency_registry = CapabilityRegistry::new(vec![dependency_descriptor.clone()]).unwrap();
    let dependency_config = InstrumentConfig::from_parts(
        dependency_descriptor.id().clone(),
        vec![
            ParameterAssignment::new(
                parameter_id("dependent.enabled"),
                ParameterValue::Toggle(false),
            ),
            ParameterAssignment::new(parameter_id("dependent.value"), ParameterValue::Stepped(1)),
        ],
        Vec::new(),
    );
    assert!(matches!(
        dependency_registry.validate_config(&dependency_config),
        Err(CapabilityError::DependencyUnsatisfied(_))
    ));

    println!("CREST_ACCEPTANCE capability_schema passed");
}

fn dependency_descriptor() -> CapabilityDescriptor {
    let enabled = ParameterSpec::new(
        parameter_id("dependent.enabled"),
        "Enabled",
        ParameterKind::Toggle,
        ParameterUpdate::Structural,
        ParameterDefault::Value(ParameterValue::Toggle(false)),
        None,
        Vec::new(),
        None,
        None,
        None,
        "toggle",
        None,
        None,
    )
    .unwrap();
    let value = ParameterSpec::new(
        parameter_id("dependent.value"),
        "Value",
        ParameterKind::Stepped,
        ParameterUpdate::Structural,
        ParameterDefault::Value(ParameterValue::Stepped(0)),
        Some(ParameterRange::new(0.0, 8.0).unwrap()),
        Vec::new(),
        Some(1.0),
        Some(2.0),
        None,
        "integer",
        Some(ParameterPredicate::new(
            parameter_id("dependent.enabled"),
            ParameterValue::Toggle(true),
        )),
        None,
    )
    .unwrap();
    CapabilityDescriptor::new(
        CapabilityId::new("instrument.dependent").unwrap(),
        "Dependent",
        "instrument.dependent",
        vec![CapabilitySection::new("main", "Main", vec![enabled, value]).unwrap()],
        Vec::new(),
        VoicePolicy::FixedPerPatch { voices: 1 },
        vec![MidiMessageKind::NoteOn],
    )
    .unwrap()
}
