//! Catalog-wide named controls preserve numeric configs and RT scalar encoding.
use crest_synth::adapter::upstream_audio;
use crest_synth::control::{
    AppEvent, AppState, Direction, InteractionMode, SavedSession, SemanticAction,
    SemanticControlValue, StateProjector, SurfaceId, TopLevelContext,
};
use crest_synth::kernel::{MidiChannel, PatchId};
use crest_synth::mixer::{global_parameters::GlobalParameters, patch_output::PatchOutput};
use crest_synth::synth::effect_slot_id::EffectSlotIndex;
use crest_synth::synth::*;

const PLAITS: &[&str] = &[
    "Virtual Analog + VCF",
    "Phase Distortion",
    "6-Operator FM Bank 1",
    "6-Operator FM Bank 2",
    "6-Operator FM Bank 3",
    "Wave Terrain",
    "String Machine",
    "Chiptune",
    "Virtual Analog",
    "Waveshaping",
    "2-Operator FM",
    "Granular Formant",
    "Additive",
    "Wavetable",
    "Chords",
    "Speech",
    "Swarm",
    "Filtered Noise",
    "Particle Noise",
    "String",
    "Modal Resonator",
    "Bass Drum",
    "Snare Drum",
    "Hi-Hat",
];

const RINGS: &[&str] = &[
    "Modal Resonator",
    "Sympathetic Strings",
    "Inharmonic String",
    "FM Voice",
    "Quantized Sympathetic Strings",
    "String + Reverb",
];
const ELEMENTS: &[&str] = &[
    "Modal Resonator",
    "Inharmonic String",
    "String Chords",
    "Ominous FM",
];
const CHORDS: &[&str] = &[
    "Octaves",
    "Minor 7",
    "Minor",
    "Minor add9",
    "Minor add11",
    "Fifths",
    "Major add11",
    "Major add9",
    "Major",
    "Major 7",
    "Sus4",
];

// Exhaustively exercise installed named controls through the reducer and both
// projections, including nonzero defaults and parameters belonging to effects.
#[test]
fn every_named_parameter_edits_projects_and_persists_with_unchanged_scalars() {
    let instruments = upstream_audio::instrument_ports().unwrap();
    let effects = upstream_audio::effect_ports().unwrap();
    let base = instruments
        .iter()
        .find(|port| port.descriptor().id().as_str() == "instrument.mutable.rings")
        .unwrap()
        .descriptor();
    let mut cases = Vec::new();
    for port in &instruments {
        let descriptor = port.descriptor();
        for (index, spec) in descriptor.parameters().enumerate() {
            if has_labels(spec) {
                cases.push((descriptor.clone(), None, index));
            }
        }
    }
    for port in &effects {
        let descriptor = port.descriptor();
        for (index, spec) in descriptor.parameters().enumerate() {
            if has_labels(spec) {
                cases.push((base.clone(), Some(descriptor.clone()), index));
            }
        }
    }
    let mut rendered_rows = Vec::new();
    for (descriptor, effect, parameter_index) in cases {
        let spec = match &effect {
            Some(effect) => effect.parameters().nth(parameter_index).unwrap(),
            None => descriptor.parameters().nth(parameter_index).unwrap(),
        };
        let range = spec.range().unwrap();
        let initial_value = match spec.kind() {
            ParameterKind::Stepped => ParameterValue::Stepped(range.minimum() as i64),
            _ => ParameterValue::Continuous(range.minimum()),
        };
        let assignments = |specs: Vec<&ParameterSpec>, set_initial: bool| {
            specs
                .into_iter()
                .map(|candidate| {
                    let ParameterDefault::Value(value) = candidate.default_value() else {
                        panic!("named catalog controls need no assets")
                    };
                    ParameterAssignment::new(
                        candidate.id().clone(),
                        if set_initial && candidate.id() == spec.id() {
                            initial_value.clone()
                        } else {
                            value.clone()
                        },
                    )
                })
                .collect::<Vec<_>>()
        };
        let config = descriptor
            .create_config(
                &assignments(descriptor.parameters().collect(), effect.is_none()),
                &[],
            )
            .unwrap();
        let registry = CapabilityRegistry::new(vec![descriptor.clone()]).unwrap();
        let mut patch = Patch::new(
            PatchId::new(1).unwrap(),
            "Named parameter".into(),
            config,
            MidiChannel::new(0).unwrap(),
            PatchOutput::default(),
        );
        if let Some(effect) = &effect {
            patch = patch.with_effect_slot(
                EffectSlotIndex::ALL[0],
                effect
                    .create_config(
                        EffectSlotId::new(1).unwrap(),
                        &assignments(effect.parameters().collect(), true),
                        &[],
                    )
                    .unwrap(),
            );
        }
        let mut state = AppState::new_with_effects(
            registry.clone(),
            EffectCapabilityRegistry::new(effect.clone().into_iter().collect()).unwrap(),
            GlobalParameters::new(0.0).unwrap(),
        );
        state.apply(AppEvent::InstallPatches(vec![patch])).unwrap();
        state
            .apply(AppEvent::SelectContext(TopLevelContext::Patch))
            .unwrap();
        if effect.is_some() {
            state.apply(AppEvent::Navigate(Direction::Down)).unwrap();
        }
        state
            .apply(AppEvent::EnterSurface(SurfaceId::PatchDetail))
            .unwrap();
        for _ in 0..parameter_index {
            state.apply(AppEvent::Navigate(Direction::Down)).unwrap();
        }
        state
            .apply_semantic_action(SemanticAction::SetInteractionMode(InteractionMode::Adjust))
            .unwrap();
        let steps =
            ((range.maximum() - range.minimum()) / spec.fine_step().unwrap()).round() as usize;
        let mut expected_value = initial_value;
        for index in 0..=steps {
            let (_, page, _, shell, parameters) =
                StateProjector::new().project_with_shell(&state).unwrap();
            let control = shell
                .semantic_model()
                .surface(SurfaceId::PatchDetail)
                .unwrap()
                .controls()
                .iter()
                .find(|control| control.focused())
                .unwrap();
            let expected_label = spec.value_label(&expected_value);
            assert_eq!(
                control.selected_label(),
                expected_label.as_deref(),
                "{} at {index}",
                spec.id()
            );
            assert_eq!(
                control.value(),
                &SemanticControlValue::Parameter(expected_value.clone()),
                "{}",
                spec.id()
            );
            let row = page
                .as_ref()
                .unwrap()
                .detail()
                .unwrap()
                .sections()
                .iter()
                .flat_map(|section| section.parameters())
                .find(|row| row.label() == control.label())
                .unwrap();
            assert_eq!(row.selected_label(), expected_label.as_deref());
            let scalars = if effect.is_some() {
                parameters.patches()[0].effect(0).unwrap().scalars()
            } else {
                parameters.patches()[0].instrument().values()
            };
            assert_eq!(
                scalars[parameter_index],
                spec.scalar_value(&expected_value).unwrap()
            );
            rendered_rows.push(serde_json::to_value(control).unwrap());
            let saved = SavedSession::capture(&state);
            assert_eq!(
                SavedSession::from_json(&saved.to_json().unwrap(), &registry).unwrap(),
                saved
            );
            if index < steps {
                state.apply(AppEvent::Adjust(Direction::Right)).unwrap();
                expected_value = spec
                    .adjusted_scalar_value(&expected_value, ParameterAdjustment::FineIncrease)
                    .unwrap();
            }
        }
    }
    if let Some(path) = std::env::var_os("CREST_NAMED_MODEL_ROWS") {
        std::fs::write(path, serde_json::to_vec_pretty(&rendered_rows).unwrap()).unwrap();
    }
}

fn has_labels(spec: &ParameterSpec) -> bool {
    let json = serde_json::to_value(spec).unwrap();
    ["steppedLabels", "continuousLabels"]
        .iter()
        .any(|field| !json[*field].as_array().unwrap().is_empty())
}

#[test]
fn entire_installed_catalog_names_categorical_steps_and_preserves_real_counts() {
    let instruments =
        crest_synth::adapter::production_instruments::production_capability_registry().unwrap();
    let effects = crest_synth::adapter::production_effects::production_effect_registry().unwrap();
    let specs = instruments
        .descriptors()
        .iter()
        .flat_map(|d| d.parameters())
        .chain(effects.descriptors().iter().flat_map(|d| d.parameters()));
    let counts = [
        "daisy.phaser.parameter-0",
        "stk.mesh2d.parameter-0",
        "stk.mesh2d.parameter-1",
    ];
    let mut found_counts = Vec::new();
    for spec in specs {
        if spec.kind() == ParameterKind::Choice {
            assert!(spec
                .choices()
                .iter()
                .all(|choice| !choice.label().trim().is_empty()));
        }
        if spec.kind() != ParameterKind::Stepped {
            continue;
        }
        let range = spec.range().unwrap();
        for value in range.minimum() as i64..=range.maximum() as i64 {
            let label = spec.value_label(&ParameterValue::Stepped(value));
            if counts.contains(&spec.id().as_str()) {
                assert!(label.is_none(), "count must stay numeric: {}", spec.id());
            } else {
                let label =
                    label.unwrap_or_else(|| panic!("unlabeled selector: {} = {value}", spec.id()));
                assert!(
                    !label.trim().is_empty() && label.parse::<f64>().is_err(),
                    "{}",
                    spec.id()
                );
            }
        }
        if counts.contains(&spec.id().as_str()) {
            found_counts.push(spec.id().as_str());
        }
    }
    assert_eq!(found_counts.len(), counts.len());
    for (id, index, names) in [
        ("instrument.mutable.plaits", 0, PLAITS),
        ("instrument.mutable.rings", 0, RINGS),
        ("instrument.mutable.rings", 5, CHORDS),
        ("instrument.mutable.elements", 0, ELEMENTS),
    ] {
        let spec = instruments
            .descriptor(&CapabilityId::new(id).unwrap())
            .unwrap()
            .parameters()
            .nth(index)
            .unwrap();
        for (value, name) in names.iter().enumerate() {
            assert_eq!(
                spec.value_label(&ParameterValue::Stepped(value as i64))
                    .as_deref(),
                Some(*name)
            );
        }
    }
}

#[test]
fn stepped_names_validate_and_old_metadata_and_configs_remain_readable() {
    let provider = upstream_audio::instrument_ports()
        .unwrap()
        .into_iter()
        .find(|port| port.descriptor().id().as_str() == "instrument.mutable.rings")
        .unwrap();
    let descriptor = provider.descriptor();
    let model = descriptor.parameters().next().unwrap();
    assert!(model
        .clone()
        .with_stepped_labels(vec!["Only one".into()])
        .is_err());
    let mut empty_name = RINGS
        .iter()
        .map(|label| (*label).to_owned())
        .collect::<Vec<_>>();
    empty_name[2] = " ".into();
    assert!(model.clone().with_stepped_labels(empty_name).is_err());
    assert_eq!(model.value_label(&ParameterValue::Stepped(-1)), None);
    assert_eq!(model.value_label(&ParameterValue::Stepped(6)), None);
    let mut old = serde_json::to_value(&descriptor).unwrap();
    for section in old["sections"].as_array_mut().unwrap() {
        for parameter in section["parameters"].as_array_mut().unwrap() {
            parameter.as_object_mut().unwrap().remove("steppedLabels");
            parameter
                .as_object_mut()
                .unwrap()
                .remove("continuousLabels");
        }
    }
    let old: CapabilityDescriptor = serde_json::from_value(old).unwrap();
    CapabilityRegistry::new(vec![old.clone()]).unwrap();
    for index in 0..RINGS.len() {
        let values = old
            .parameters()
            .map(|spec| {
                let ParameterDefault::Value(value) = spec.default_value() else {
                    panic!("Rings has no assets")
                };
                ParameterAssignment::new(
                    spec.id().clone(),
                    if spec.id() == model.id() {
                        ParameterValue::Stepped(index as i64)
                    } else {
                        value.clone()
                    },
                )
            })
            .collect::<Vec<_>>();
        let old_config = old.create_config(&values, &[]).unwrap();
        let new_config = descriptor.create_config(old_config.values(), &[]).unwrap();
        assert_eq!(
            serde_json::to_string(&old_config).unwrap(),
            serde_json::to_string(&new_config).unwrap()
        );
        assert_eq!(
            model
                .scalar_value(&ParameterValue::Stepped(index as i64))
                .unwrap(),
            index as f32
        );
    }
}

fn expected_continuous_label(id: &str, scalar: f32) -> Option<String> {
    let v = f64::from(scalar);
    Some(match id {
        "airwindows.biquad2.parameter-0" => ["Lowpass", "Highpass", "Bandpass", "Notch"]
            [((v * 3.999 + 0.00001).ceil() as usize) - 1]
            .into(),
        "mda.jx10.parameter-3" => [
            "Poly",
            "Poly",
            "Poly Legato",
            "Poly Glide",
            "Mono",
            "Mono",
            "Mono Legato",
            "Mono Glide",
        ][(7.9_f32 * scalar) as usize]
            .into(),
        "mda.jx10.parameter-22" => [
            "−2 Octaves",
            "−1 Octave",
            "Unison",
            "+1 Octave",
            "+2 Octaves",
        ][(4.9 * v).floor() as usize]
            .into(),
        "mda.leslie.parameter-0" => if scalar < 0.1_f32 {
            "Stop"
        } else if scalar < 0.5_f32 {
            "Slow"
        } else {
            "Fast"
        }
        .into(),
        "mda.talkbox.parameter-2" => if scalar > 0.5_f32 { "Left" } else { "Right" }.into(),
        "mda.epiano.parameter-4" => {
            if scalar > 0.5_f32 {
                format!("Tremolo {:.0}%", 200.0 * v - 100.0)
            } else {
                format!("Pan {:.0}%", 100.0 - 200.0 * v)
            }
        }
        "mda.jx10.parameter-20" => {
            if scalar < 0.5_f32 {
                format!("PWM {:.0}%", 100.0 - 200.0 * v)
            } else {
                format!("Vibrato {:.0}%", 200.0 * v - 100.0)
            }
        }
        "mda.jx10.parameter-10" => {
            if scalar < 0.05_f32 {
                "Off".into()
            } else {
                return None;
            }
        }
        "mda.dynamics.parameter-1" => {
            if v > 0.58 && v < 0.62 {
                "Limit".into()
            } else {
                return None;
            }
        }
        "mda.dynamics.parameter-5" => {
            if v > 0.98 {
                "Off".into()
            } else {
                return None;
            }
        }
        "mda.dynamics.parameter-6" => {
            if v < 0.02 {
                "Off".into()
            } else {
                return None;
            }
        }
        "mda.dubdelay.parameter-1" => {
            if scalar > 0.5_f32 {
                format!("Limiting {:.0}%", 220.0 * v - 110.0)
            } else {
                format!("Saturating {:.0}%", 110.0 - 220.0 * v)
            }
        }
        "mda.dubdelay.parameter-2" => {
            if scalar > 0.5_f32 {
                format!("High {:.0}%", 200.0 * v - 100.0)
            } else {
                format!("Low {:.0}%", 100.0 - 200.0 * v)
            }
        }
        _ => panic!("new normalized label needs an upstream boundary oracle: {id}"),
    })
}

#[test]
fn normalized_selectors_match_upstream_float_boundaries_without_changing_configs() {
    let instruments = upstream_audio::instrument_ports().unwrap();
    let effects = upstream_audio::effect_ports().unwrap();
    let mut specs = instruments
        .iter()
        .flat_map(|p| p.descriptor().parameters().cloned().collect::<Vec<_>>())
        .collect::<Vec<_>>();
    specs.extend(
        effects
            .iter()
            .flat_map(|p| p.descriptor().parameters().cloned().collect::<Vec<_>>()),
    );
    let expected_ids = [
        "airwindows.biquad2.parameter-0",
        "mda.jx10.parameter-3",
        "mda.jx10.parameter-22",
        "mda.leslie.parameter-0",
        "mda.talkbox.parameter-2",
        "mda.epiano.parameter-4",
        "mda.jx10.parameter-20",
        "mda.jx10.parameter-10",
        "mda.dynamics.parameter-1",
        "mda.dynamics.parameter-5",
        "mda.dynamics.parameter-6",
        "mda.dubdelay.parameter-1",
        "mda.dubdelay.parameter-2",
    ];
    let mut found = Vec::new();
    for spec in specs
        .iter()
        .filter(|spec| spec.kind() == ParameterKind::Continuous && has_labels(spec))
    {
        found.push(spec.id().as_str());
        let metadata = serde_json::to_value(spec).unwrap();
        let mut inputs = (0..=10_000)
            .map(|n| n as f64 / 10_000.0)
            .collect::<Vec<_>>();
        // Check both adjacent DSP floats and f64 values rounding onto each edge.
        for band in metadata["continuousLabels"].as_array().unwrap() {
            for endpoint in ["minimum", "maximum"] {
                let edge = band["range"][endpoint].as_f64().unwrap() as f32;
                for bits in edge.to_bits().saturating_sub(1)..=edge.to_bits() + 1 {
                    let value = f64::from(f32::from_bits(bits));
                    if (0.0..=1.0).contains(&value) {
                        inputs.extend([value, (value + 1e-12).min(1.0)]);
                    }
                }
            }
        }
        let mut old = metadata.clone();
        old.as_object_mut().unwrap().remove("continuousLabels");
        old.as_object_mut().unwrap().remove("steppedLabels");
        let old: ParameterSpec = serde_json::from_value(old).unwrap();
        for value in inputs {
            let typed = ParameterValue::Continuous(value);
            assert_eq!(
                spec.value_label(&typed).as_deref(),
                expected_continuous_label(spec.id().as_str(), value as f32).as_deref(),
                "{} at {value:?}",
                spec.id()
            );
            assert_eq!(
                spec.scalar_value(&typed).unwrap(),
                old.scalar_value(&typed).unwrap()
            );
            assert_eq!(
                spec.adjusted_scalar_value(&typed, ParameterAdjustment::FineIncrease),
                old.adjusted_scalar_value(&typed, ParameterAdjustment::FineIncrease)
            );
        }
        for value in [f64::NAN, f64::INFINITY, -0.1, 1.1] {
            assert!(spec
                .value_label(&ParameterValue::Continuous(value))
                .is_none());
        }
    }
    found.sort_unstable();
    let mut expected_ids = expected_ids.to_vec();
    expected_ids.sort_unstable();
    assert_eq!(found, expected_ids);
}

#[test]
fn continuous_label_metadata_rejects_invalid_ranges_and_amounts() {
    let descriptor = upstream_audio::instrument_ports()
        .unwrap()
        .into_iter()
        .find(|p| p.descriptor().id().as_str() == "instrument.mda.jx10")
        .unwrap()
        .descriptor();
    let spec = descriptor.parameters().nth(3).unwrap();
    let band = |min, max, label: &str| {
        ContinuousValueLabel::new(ParameterRange::new(min, max).unwrap(), label)
    };
    for labels in [
        vec![band(0.0, 0.5, " ")],
        vec![band(-1.0, 0.5, "Outside")],
        vec![band(0.0, 0.5, "A"), band(0.5, 1.0, "Overlap")],
        vec![band(0.5, 1.0, "A"), band(0.0, 0.4, "Unordered")],
        vec![band(0.0, 1.0, "Bad scale").with_amount(f64::INFINITY, 0.0, "%")],
        vec![band(0.0, 1.0, "Bad offset").with_amount(1.0, f64::NAN, "%")],
    ] {
        assert!(spec.clone().with_continuous_labels(labels).is_err());
    }
    let stepped = upstream_audio::instrument_ports()
        .unwrap()
        .into_iter()
        .find(|p| p.descriptor().id().as_str() == "instrument.mutable.rings")
        .unwrap()
        .descriptor();
    assert!(stepped
        .parameters()
        .next()
        .unwrap()
        .clone()
        .with_continuous_labels(vec![band(0.0, 1.0, "Wrong kind")])
        .is_err());
}
