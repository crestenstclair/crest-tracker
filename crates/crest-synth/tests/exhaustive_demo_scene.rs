mod support;

use crest_synth::control::app_state::EventRejection;
use crest_synth::control::event_record::{EventOutcome, EventSource};
use crest_synth::control::{EngineSelectionStatusKind, PatchControlId};
use crest_synth::synth::{EffectSlotId, ParameterId, VoiceEnvelope};
use crest_synth::testing::demo_scene_report::DemoCoverageGroup;
use serde_json::Value;
use std::collections::BTreeSet;

const COVERAGE_GROUPS: [DemoCoverageGroup; 11] = [
    DemoCoverageGroup::Inputs,
    DemoCoverageGroup::Events,
    DemoCoverageGroup::Contexts,
    DemoCoverageGroup::Directions,
    DemoCoverageGroup::MidiKinds,
    DemoCoverageGroup::EditableParameters,
    DemoCoverageGroup::PatchControls,
    DemoCoverageGroup::SerializedProperties,
    DemoCoverageGroup::Rejections,
    DemoCoverageGroup::Projections,
    DemoCoverageGroup::AudioEffects,
];

fn stable_control_surface(tree: &Value) -> Vec<Value> {
    [
        "/mixer",
        "/global",
        "/interaction",
        "/patchPage",
        "/projection/context",
        "/projection/selectedLine",
        "/parameters/patches",
        "/parameters/mixerTracks",
        "/parameters/global",
    ]
    .into_iter()
    .map(|pointer| {
        tree.pointer(pointer)
            .expect("required StateTree branch exists")
            .clone()
    })
    .collect()
}

fn source_name(source: EventSource) -> &'static str {
    match source {
        EventSource::Startup => "startup",
        EventSource::Keyboard => "keyboard",
        EventSource::Controller => "controller",
        EventSource::AutomaticMidi => "automatic-midi",
        EventSource::PhysicalMidi => "physical-midi",
        EventSource::DemoScene => "demo-scene",
        EventSource::Worker => "worker",
        EventSource::System => "system",
    }
}

#[test]
fn exhaustive_scene_proves_exact_coverage_boundaries_and_restoration() {
    let first = support::run_demo();
    let second = support::run_demo();
    let report = &first.report;

    assert!(
        report.is_complete(),
        "coverage report contains a gap: {:?}",
        report.coverage()
    );
    assert_eq!(report.coverage().missing_count(), 0);
    assert_eq!(report.coverage().unexpected_count(), 0);
    assert_eq!(
        report.coverage().expected_count(),
        report.coverage().exercised_count()
    );
    assert_eq!(report.event_log().dropped_records(), 0);
    assert_eq!(
        report.event_log().total_observed(),
        report.event_log().records().len() as u64
    );

    for group in COVERAGE_GROUPS {
        let coverage = report.coverage().group(group);
        assert_eq!(coverage.expected(), coverage.exercised(), "{group:?}");
        assert!(coverage.missing().is_empty(), "{group:?}");
        assert!(coverage.unexpected().is_empty(), "{group:?}");
    }
    // Twenty-four normalized keys in each of two kinds, plus focus loss.
    // Ctrl+4 opens Sends. The remaining gallery digits and bracket keys are
    // exercised as normalized inputs without adding reducer event kinds.
    assert_eq!(
        report
            .coverage()
            .group(DemoCoverageGroup::Inputs)
            .exercised()
            .len(),
        49
    );
    // Exact player/worker event contract, including the Send action reached
    // through the physical Ctrl+4 sweep. Chain edits have dedicated witnesses.
    let expected_events = [
        "event.activate",
        "event.adjust",
        "event.engineActivationAcknowledged",
        "event.enginePreparationFailed",
        "event.enginePrepared",
        "event.engineSelectionLifecycleAdvanced",
        "event.enterSurface",
        "event.installPatches",
        "event.midi",
        "event.navigate",
        "event.navigatePage",
        "event.openMidiSettings",
        "event.openRelated",
        "event.previewStart",
        "event.previewStop",
        "event.return",
        "event.selectContext",
        "event.selectPatch",
        "event.send",
        "event.setInteractionMode",
        "event.setReturnOccupancy",
        "event.setSlotOccupancy",
        "event.topologyPreparationFailed",
        "event.topologyPrepared",
    ]
    .map(str::to_owned);
    assert_eq!(
        report
            .coverage()
            .group(DemoCoverageGroup::Events)
            .exercised(),
        expected_events.as_slice()
    );
    assert_eq!(
        report
            .coverage()
            .group(DemoCoverageGroup::Contexts)
            .exercised(),
        &[
            "context.mixer".to_owned(),
            "context.patch".to_owned(),
            "interactionMode.adjust".to_owned(),
            "interactionMode.navigate".to_owned(),
            // The subordinate detail surface, entered and left through the
            // passive action boundary like the two persistent sides. It was
            // absent for exactly as long as `SurfaceId::is_enterable` withheld
            // `PatchDetail` — a gated action is a refusal, not a
            // demonstration — and its presence here is the scene actually
            // reaching a projected detail state end to end.
            "surface.detail".to_owned(),
            "surface.inspector".to_owned(),
            "surface.utility".to_owned(),
        ]
    );
    let mut expected_patch_controls = PatchControlId::surface_descriptor()
        .iter()
        .map(|control| format!("patchControl.{control}"))
        .collect::<Vec<_>>();
    expected_patch_controls.extend(
        PatchControlId::utility_surface_descriptor()
            .iter()
            .map(|control| format!("patchControl.{control}")),
    );
    expected_patch_controls.push(format!(
        "patchControl.{}",
        PatchControlId::Capability(
            ParameterId::new(
                crest_synth::adapter::hidef_soundfont_capability::SOUNDFONT_PRESET_PARAMETER_ID,
            )
            .unwrap(),
        )
    ));
    for parameter in ["chorus.amount", "chorus.depth"] {
        expected_patch_controls.push(format!(
            "patchControl.{}",
            PatchControlId::Effect(
                EffectSlotId::new(1).unwrap(),
                ParameterId::new(parameter).unwrap(),
            )
        ));
    }
    // Every one of the three occupancy rows is reachable whether occupied or
    // empty, so each contributes one exercised focus identity (FR-002).
    for slot in crest_synth::synth::effect_slot_id::EffectSlotIndex::ALL {
        expected_patch_controls.push(format!("patchControl.{}", PatchControlId::EffectSlot(slot)));
    }
    expected_patch_controls.sort();
    assert_eq!(
        report
            .coverage()
            .group(DemoCoverageGroup::PatchControls)
            .exercised(),
        expected_patch_controls.as_slice()
    );

    let routed_patch_adsr = report
        .checkpoints()
        .iter()
        .filter_map(|checkpoint| checkpoint.patch_adsr())
        .filter(|checkpoint| checkpoint.lifecycle().is_none())
        .collect::<Vec<_>>();
    assert_eq!(routed_patch_adsr.len(), 4);
    assert_eq!(
        routed_patch_adsr
            .iter()
            .map(|checkpoint| checkpoint.parameter())
            .collect::<Vec<_>>(),
        VoiceEnvelope::surface_descriptor()
            .iter()
            .map(|descriptor| descriptor.parameter())
            .collect::<Vec<_>>()
    );
    assert!(routed_patch_adsr.iter().all(|checkpoint| {
        checkpoint.focus_projection_exact()
            && checkpoint.scalar_only()
            && checkpoint.all_envelope_values_exact()
            && checkpoint.untargeted_patches_exact()
            && checkpoint.graph_revision_exact()
    }));

    let preparing_adsr = report
        .checkpoints()
        .iter()
        .filter_map(|checkpoint| checkpoint.patch_adsr())
        .find(|checkpoint| checkpoint.lifecycle() == Some(EngineSelectionStatusKind::Preparing))
        .expect("Preparing carries a source-revision PATCH ADSR checkpoint");
    let activating_adsr = report
        .checkpoints()
        .iter()
        .filter_map(|checkpoint| checkpoint.patch_adsr())
        .find(|checkpoint| checkpoint.lifecycle() == Some(EngineSelectionStatusKind::Activating))
        .expect("Activating carries a replacement-revision PATCH ADSR checkpoint");
    assert!(preparing_adsr.scalar_only() && preparing_adsr.graph_revision_exact());
    assert!(activating_adsr.scalar_only() && activating_adsr.graph_revision_exact());
    assert!(preparing_adsr.all_envelope_values_exact());
    assert!(activating_adsr.all_envelope_values_exact());
    assert_ne!(preparing_adsr.parameter(), activating_adsr.parameter());
    assert!(activating_adsr.renderer_graph_revision() > preparing_adsr.renderer_graph_revision());

    let expected = first
        .expected_coverage
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let observed = report
        .event_log()
        .coverage()
        .exercised()
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    assert_eq!(expected, observed);
    assert_eq!(
        report.event_log().coverage().expected(),
        report.event_log().coverage().exercised()
    );

    for parameter in [
        "trimGainDb",
        "outputTrack",
        "levelDb",
        "pan",
        "mute",
        "solo",
        "sends[0]",
        "sends[7]",
        "attackMilliseconds",
        "decayMilliseconds",
        "sustain",
        "releaseMilliseconds",
        "chorus.amount",
        "chorus.depth",
        "masterGainDb",
        // Return-owned state addressed by BusId: the occupying registry
        // entries' descriptor scalars plus the return-owned level.
        "reverb.room-size",
        "reverb.damping",
        "delay.milliseconds",
        "delay.feedback",
        "B0.returnLevel",
        "B7.returnLevel",
    ] {
        assert!(
            report
                .coverage()
                .group(DemoCoverageGroup::EditableParameters)
                .exercised()
                .iter()
                .any(|identifier| identifier.ends_with(parameter)),
            "missing exact parameter exercise for {parameter}"
        );
    }

    let boundary_rejections = report
        .event_log()
        .records()
        .iter()
        .filter(|record| {
            record.outcome() == EventOutcome::Rejected
                && record.rejection() == Some(EventRejection::ParameterAtBoundary)
        })
        .count();
    let unique_boundary_targets = report
        .coverage()
        .group(DemoCoverageGroup::EditableParameters)
        .expected()
        .iter()
        .filter(|identifier| !identifier.ends_with(".mute") && !identifier.ends_with(".solo"))
        .map(|identifier| {
            if let Some(patch) = identifier.strip_prefix("parameter.patch.") {
                let (_, target) = patch
                    .split_once('.')
                    .expect("Patch parameter identity contains its target");
                format!("patch.{target}")
            } else if let Some(target) = identifier.strip_prefix("parameter.global.") {
                format!("global.{target}")
            } else if let Some(track) = identifier.strip_prefix("parameter.track.") {
                let (_, target) = track
                    .split_once('.')
                    .expect("track parameter identity contains its target");
                // All indexed sends share one descriptor, so they are
                // one boundary target kind exactly as one named track field
                // is one kind across all sixteen tracks.
                if target.starts_with("sends[") {
                    "track.sends".to_owned()
                } else {
                    format!("track.{target}")
                }
            } else if let Some(bus) = identifier.strip_prefix("parameter.return.") {
                let (_, target) = bus
                    .split_once('.')
                    .expect("return parameter identity contains its target");
                format!("return.{target}")
            } else {
                panic!("unexpected editable parameter identity {identifier}");
            }
        })
        .collect::<BTreeSet<_>>()
        .len();
    assert!(
        boundary_rejections >= unique_boundary_targets * 2,
        "each schema-derived editable target kind needs lower and upper boundary evidence; observed {boundary_rejections} for {unique_boundary_targets} target kinds"
    );
    for (index, record) in report.event_log().records().iter().enumerate() {
        if record.rejection() == Some(EventRejection::ParameterAtBoundary) {
            assert!(
                report
                    .event_log()
                    .records()
                    .iter()
                    .skip(index + 1)
                    .any(|later| {
                        later.outcome() == EventOutcome::Accepted
                            && matches!(
                                later.input(),
                                crest_synth::control::event_record::EventInput::Adjust { .. }
                            )
                    }),
                "a boundary rejection must not terminate later valid edits"
            );
        }
    }

    let sources = report
        .event_log()
        .records()
        .iter()
        .map(|record| source_name(record.source()))
        .collect::<BTreeSet<_>>();
    assert_eq!(
        sources,
        BTreeSet::from([
            "startup",
            "keyboard",
            "automatic-midi",
            "demo-scene",
            "system",
            "worker",
        ])
    );

    let final_tree: Value = serde_json::from_str(report.final_state_tree().json())
        .expect("final StateTree is valid JSON");
    assert_eq!(
        stable_control_surface(&first.baseline),
        stable_control_surface(&final_tree),
        "every reversible value and selection must return to baseline"
    );
    let baseline_patches = first.baseline["patches"].as_array().unwrap();
    let final_patches = final_tree["patches"].as_array().unwrap();
    assert_eq!(baseline_patches.len(), final_patches.len());
    for property in ["id", "name", "channel", "envelope", "output", "postEffects"] {
        assert_eq!(baseline_patches[0][property], final_patches[0][property]);
    }
    assert_eq!(baseline_patches[1], final_patches[1]);
    assert_eq!(
        final_patches[0]["instrument"]["capabilityId"],
        "instrument.soundfont.hidef"
    );
    assert_eq!(
        final_patches[0]["instrument"]["values"][0]["value"]["value"],
        "sf2.bank-0.program-0"
    );
    assert_eq!(
        final_patches[0]["instrument"]["values"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        final_patches[0]["instrument"]["assetReferences"][0]["reference"]["locator"],
        "./sf2/HiDef.sf2"
    );
    assert_eq!(final_tree["engineSelection"]["kind"], "ready");
    assert!(report
        .checkpoints()
        .iter()
        .filter_map(|checkpoint| checkpoint.engine_selection())
        .any(|checkpoint| checkpoint.target_patch_peak() > 0.0));

    assert!(report
        .checkpoints()
        .iter()
        .all(|checkpoint| checkpoint.audio_measurement().is_finite()));
    assert!(report.audio_evidence().mixed_engine_stems_nonzero());
    assert!(report.audio_evidence().mixed_engine_parameter_isolation());
    assert!(report.audio_evidence().patch_effect_target_exact());
    assert!(report.audio_evidence().patch_effect_difference_nonzero());
    assert!(report.audio_evidence().patch_effect_side_nonzero());
    assert!(report.audio_evidence().patch_effect_before_mix_stem_exact());
    assert!(report.audio_evidence().unconfigured_patch_isolated());
    assert!(report
        .audio_evidence()
        .patch_effect_structural_preservation());
    assert_eq!(first.expected_coverage, second.expected_coverage);
    assert_eq!(first.baseline, second.baseline);
    assert_eq!(
        report.to_json().expect("first report serializes"),
        second.report.to_json().expect("second report serializes"),
        "fresh identical services must produce byte-identical reports"
    );

    println!("CREST_ACCEPTANCE exhaustive_demo_scene passed");
}
