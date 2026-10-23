//! Reproducible test-only authoring through Synth's canonical types. Runtime
//! loads the resulting native document without constructing a second sound model.
use super::*;
use crest_synth::adapter::braids_capability::{
    BraidsCapability, BRAIDS_MODEL_PARAMETER_ID, BRAIDS_TIMBRE_PARAMETER_ID,
};
use crest_synth::adapter::production_effects::{
    production_chorus_config, production_effect_registry,
};
use crest_synth::control::{AppEvent, AppState, SavedSession};
use crest_synth::kernel::MidiChannel;
use crest_synth::mixer::{bus_id::BusId, bus_return::BusReturnBank};
use crest_synth::mixer::{
    global_parameters::GlobalParameters, mixer_state::MixerState, mixer_track_id::MixerTrackId,
    mixer_track_parameters::MixerTrackParameter, patch_output::PatchOutput,
};
use crest_synth::synth::{effect_slot_id::EffectSlotIndex, EffectCapabilityId};
use crest_synth::synth::{
    CapabilityRegistry, InstrumentCapabilityProvider, ParameterId, ParameterValue, Patch,
    VoiceEnvelope,
};
use crest_tracker::mpe::{BendRange, MpeProfile, MpeProvenance, MpeZone};

const SOUNDS: [(&str, &str, f32, f32); 5] = [
    ("Glass lead", "braids.model.sine-triangle", 0.35, -0.15),
    ("Round bass", "braids.model.square-sub", 0.25, 0.0),
    ("Soft harmony", "braids.model.triple-sine", 0.4, 0.15),
    ("Bell steps", "braids.model.struck-bell", 0.45, -0.4),
    ("Wood pulse", "braids.model.struck-drum", 0.3, 0.3),
];

pub(super) fn synth_session(count: usize, sustained: bool) -> SavedSession {
    let capability = BraidsCapability::new().unwrap();
    let registry = CapabilityRegistry::new(vec![capability.descriptor()]).unwrap();
    let effects = production_effect_registry().unwrap();
    let mut returns = BusReturnBank::default();
    if sustained {
        for (index, id) in ["effect.reverb", "effect.delay"].into_iter().enumerate() {
            returns
                .set_return_occupancy(
                    &effects,
                    BusId::ALL[index],
                    Some(&EffectCapabilityId::new(id).unwrap()),
                )
                .unwrap();
        }
    }
    let patches = (0..count)
        .map(|index| patch(&capability, index, sustained))
        .collect();
    let mut mixer = MixerState::default();
    for index in 0..count {
        let track = MixerTrackId::ALL[index];
        let parameters = mixer
            .track(track)
            .clone()
            .with_scalar_value(MixerTrackParameter::Level, -6.0)
            .unwrap()
            .with_scalar_value(MixerTrackParameter::Pan, SOUNDS[index % SOUNDS.len()].3)
            .unwrap();
        mixer.set_track(track, parameters);
    }
    let mut state = AppState::new_with_effects(
        registry,
        effects,
        GlobalParameters::new(if sustained { -24.0 } else { -12.0 }).unwrap(),
    )
    .with_initial_mixer(mixer)
    .with_initial_returns(returns);
    state.apply(AppEvent::InstallPatches(patches)).unwrap();
    SavedSession::capture(&state)
}

fn patch(capability: &BraidsCapability, index: usize, sustained: bool) -> Patch {
    let (name, model, timbre, _) = SOUNDS[index % SOUNDS.len()];
    let model = if sustained {
        "braids.model.sine-triangle"
    } else {
        model
    };
    let config = capability
        .default_config()
        .unwrap()
        .with_scalar_value(
            &capability.descriptor(),
            &ParameterId::new(BRAIDS_MODEL_PARAMETER_ID).unwrap(),
            ParameterValue::Choice(model.into()),
        )
        .unwrap()
        .with_scalar_value(
            &capability.descriptor(),
            &ParameterId::new(BRAIDS_TIMBRE_PARAMETER_ID).unwrap(),
            ParameterValue::continuous(f64::from(timbre)).unwrap(),
        )
        .unwrap();
    let patch = Patch::new(
        PatchId::new(index as u32 + 1).unwrap(),
        name.into(),
        config,
        MidiChannel::new(index as u8).unwrap(),
        PatchOutput::to_track(MixerTrackId::ALL[index]),
    )
    .with_envelope(VoiceEnvelope::new(8.0, 110.0, 0.72, 180.0).unwrap());
    if sustained {
        let slot = EffectSlotIndex::ALL[0];
        patch
            .with_effect_slot(
                slot,
                production_chorus_config(slot.instance_identity()).unwrap(),
            )
            .with_sends(
                BusId::ALL
                    .iter()
                    .enumerate()
                    .map(|(index, _)| match index {
                        0 => 0.12,
                        1 => 0.08,
                        _ => 0.0,
                    })
                    .collect(),
            )
            .unwrap()
    } else {
        patch
    }
}

fn document() -> Document {
    let saved = synth_session(5, false);
    let mut song = Song {
        key: Key::new(0, crest_tracker::harmony::ScaleKind::Major).unwrap(),
        ..Song::default()
    };
    let mut pins = Vec::new();
    let mut mappings = BTreeMap::new();
    for (index, (name, _, _, _)) in SOUNDS.iter().enumerate() {
        let id = InstrumentId::new(17 + index as u64).unwrap();
        let patch = PatchId::new(index as u32 + 1).unwrap();
        let definition = serde_json::to_value(saved.sound_definition(patch).unwrap()).unwrap();
        let pin = PinnedSound::new(
            format!("f8449390-b10d-4428-850e-{:012}", index + 1),
            definition,
        )
        .unwrap();
        song.instruments.insert(
            id,
            InstrumentUse {
                name: (*name).into(),
                resource: Some(ResourceRevision {
                    resource: pin.resource.clone(),
                    revision: pin.revision.clone(),
                }),
            },
        );
        mappings.insert(id, patch.value());
        pins.push(pin);
    }
    let targets = std::array::from_fn(|index| Target::Instrument {
        id: InstrumentId::new(17 + index as u64).unwrap(),
        channel: Channel::new(2).unwrap(),
    });
    let command = crate::app::demo::arrangement(&song, targets).unwrap();
    let mut editor = Editor::new(song).unwrap();
    editor.apply(0, command).unwrap();
    let mut song = editor.song().clone();
    add_expression(&mut song);
    let mut document = Document::new(song, serde_json::to_value(saved).unwrap(), mappings);
    document.pinned_sounds = pins;
    document.validate().unwrap();
    document
}

fn add_expression(song: &mut Song) {
    for phrase in song
        .phrases
        .values_mut()
        .filter(|phrase| matches!(phrase.form, PhraseForm::PianoRoll))
    {
        add_phrase_expression(phrase);
    }
}

fn add_phrase_expression(phrase: &mut Phrase) {
    for event in &mut phrase.events {
        expressive_note(event.note.as_mut().unwrap());
    }
}

fn expressive_note(note: &mut Note) {
    let profile = MpeProfile::new(
        "example:window-light".into(),
        MpeZone::lower(15).unwrap(),
        BendRange::new(2, 0).unwrap(),
        BendRange::new(2, 0).unwrap(),
    )
    .unwrap();
    note.mpe = Some(MpeProvenance::new(profile, Channel::new(2).unwrap()).unwrap());
    note.release_velocity = 47;
    note.expression_duration = Some(Beat::new(1, 2).unwrap());
    note.expression = vec![
        point(0, 1, Control::Bend { value: 8192 }),
        point(0, 1, Control::Pressure { value: 105 }),
        point(
            0,
            1,
            Control::Cc {
                number: 74,
                value: 45,
            },
        ),
        point(1, 8, Control::Bend { value: 8704 }),
        point(1, 4, Control::Pressure { value: 118 }),
        point(7, 16, Control::Bend { value: 8192 }),
    ];
    note.master_expression = vec![
        point(0, 1, Control::Bend { value: 8192 }),
        point(0, 1, Control::Pressure { value: 120 }),
    ];
}

fn point(numerator: u64, denominator: u64, control: Control) -> Expression {
    Expression {
        offset: Beat::new(numerator, denominator).unwrap(),
        control,
        sequence: None,
    }
}

#[test]
fn bundled_example_matches_canonical_authoring_and_pins_all_sounds() {
    let authored = document();
    if std::env::var_os("CREST_WRITE_EXAMPLE").is_some() {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/window-light.crest-song");
        std::fs::write(path, serde_json::to_vec_pretty(&authored).unwrap()).unwrap();
        return;
    }
    let mut bundled = demo::document().unwrap();
    // Normalize the retained legacy fixture through Synth's schema migration.
    let registry =
        CapabilityRegistry::new(vec![BraidsCapability::new().unwrap().descriptor()]).unwrap();
    bundled.synth = serde_json::to_value(
        SavedSession::from_json(&bundled.synth.to_string(), &registry).unwrap(),
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(&bundled).unwrap(),
        serde_json::to_value(&authored).unwrap()
    );
    assert_eq!(bundled.pinned_sounds.len(), 5);
    assert!(bundled
        .song
        .instruments
        .values()
        .all(|instrument| instrument.resource.is_some()));
    assert!(
        bundled.resources.is_empty(),
        "Braids example requires no external assets"
    );
    assert!(bundled
        .song
        .phrases
        .values()
        .flat_map(|phrase| &phrase.events)
        .filter_map(|event| event.note.as_ref())
        .any(|note| note.mpe.is_some() && !note.master_expression.is_empty()));
}
