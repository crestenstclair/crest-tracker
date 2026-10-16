use core::alloc::{GlobalAlloc, Layout};
use core::cell::Cell;
use crest_synth::adapter::braids_capability::{
    BraidsCapability, BRAIDS_CAPABILITY_ID, BRAIDS_FIXED_VOICES, BRAIDS_MODELS,
};
use crest_synth::adapter::braids_native::{
    braids_lifecycle_counts, BRAIDS_MODEL_COUNT, BRAIDS_VOICE_COUNT,
};
use crest_synth::adapter::braids_preparer::BraidsPreparer;
use crest_synth::adapter::lock_free_audio_boundary::LockFreeAudioBoundary;
use crest_synth::adapter::production_instruments::{
    production_capability_registry, production_instrument_preparers,
};
use crest_synth::control::app_event::{AppEvent, Direction};
use crest_synth::control::app_state::AppState;
use crest_synth::control::state_projector::StateProjector;
use crest_synth::kernel::midi_channel::MidiChannel;
use crest_synth::kernel::midi_message::{MidiMessage, MidiMessageKind};
use crest_synth::kernel::patch_id::PatchId;
use crest_synth::mixer::global_parameters::GlobalParameters;
use crest_synth::mixer::mixer_track_id::MixerTrackId;
use crest_synth::mixer::patch_output::PatchOutput;
use crest_synth::real_time::audio_boundary::{AudioBoundary, ControlAudioBoundary};
use crest_synth::real_time::audio_command::AudioCommand;
use crest_synth::real_time::audio_renderer::AudioRenderer;
use crest_synth::real_time::parameter_snapshot::{
    ParameterSnapshot, RtInstrumentParameters, RtPatchParameters, MAX_ACTIVE_PATCHES,
};
use crest_synth::real_time::prepared_graph_builder::PreparedGraphBuilder;
use crest_synth::real_time::structural_graph_boundary::NoStructuralGraphChanges;
use crest_synth::real_time::GraphRevision;
use crest_synth::synth::instrument_capability::{CapabilityRegistry, InstrumentConfig};
use crest_synth::synth::sound_font_instrument::SoundFontInstrument;
use crest_synth::synth::{
    CapabilityId, InstrumentCapabilityProvider, InstrumentPreparationError, InstrumentPreparer,
    Patch, PreparedEngineRackBuilder, VoiceEnvelope,
};
use crest_synth::testing::automatic_midi_test::create_soundfont_config;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::alloc::System;
use std::fs;
use std::path::Path;
use std::time::Instant;

const SAMPLE_RATE: f32 = 48_000.0;
const BLOCK_FRAMES: usize = 256;
const UPSTREAM_REVISION: &str = "08460a69a7e1f7a81c5a2abcc7189c9a6b7208d4";
const STMLIB_REVISION: &str = "e3bd7c9cc00e4364166f9905c0509b6ffd0535ec";

const SOURCE_HASHES: [(&str, &str); 17] = [
    (
        "braids/analog_oscillator.cc",
        "dc7fbfe3b34314a2fdd73e81d5e4d65dd0015adf8cd4f45f678dd6ce0fda91c6",
    ),
    (
        "braids/analog_oscillator.h",
        "6870f34dd3d6c67e12227299fca4d49ccb15a651b89293bb0f688f313b964991",
    ),
    (
        "braids/digital_oscillator.cc",
        "2b46fd3d702c4570af7fadc6e187a931d0f40d69532c3bf0a46ef4d1454bad84",
    ),
    (
        "braids/digital_oscillator.h",
        "f9a523bee6cfc04c1560815fb0255fa33d9838a0df42d512737e6b82237fa20a",
    ),
    (
        "braids/excitation.h",
        "e9555383effc411f26afaaf475a92ceabad59a4891569dc3d4c3cd14f3ee3197",
    ),
    (
        "braids/macro_oscillator.cc",
        "715ee728aaae8dfaa3387c50bd6cc1f1327374ff0949c8104e48cf7c11d0d241",
    ),
    (
        "braids/macro_oscillator.h",
        "8e34d2ef5a8914c4252999b7988e371d93cda998b660cd3ed4132a4164fe41fe",
    ),
    (
        "braids/parameter_interpolation.h",
        "4be46037164cb309c56590656e83baa456d3dccae158a76724b7e9906a47cee8",
    ),
    (
        "braids/resources.cc",
        "b28a075193e7568e53621361ba95e3c9ff952b350eafe02e47dfb070b76f6b53",
    ),
    (
        "braids/resources.h",
        "740135940b70a1d26f965c7c80e92aae03ae7f9d843dc81706284e0adef97327",
    ),
    (
        "braids/settings.h",
        "5e2c771a4b29c23fe25056d252864489f43801425deb9adf287ef634a74fec41",
    ),
    (
        "braids/svf.h",
        "17fe7788f2b0da209d0c879a458bd2ef61537666403166f37333dacfe7e090ee",
    ),
    (
        "stmlib/LICENSE",
        "6fde7600ac71ff9e4bdc28b642063d1a65e195f28bb07fb52a9b476efb5aa791",
    ),
    (
        "stmlib/stmlib.h",
        "3afc8589a951e882d9eea16b3eda8623dd6fb0079dd11dfbce817e65be6cfba9",
    ),
    (
        "stmlib/utils/dsp.h",
        "c0fad7f6b5b20f053d184614a35baf595e41f1394db53f80ac742d7467dbbe1b",
    ),
    (
        "stmlib/utils/random.cc",
        "145c4d7a30e373d001fd664ce6f97a475458f7dc8286252d24ca6f592e0cdb93",
    ),
    (
        "stmlib/utils/random.h",
        "423f01e905fa279864878f34e42136ad1eb37a33dad58739affaaab06b793e5d",
    ),
];

thread_local! {
    static COUNT_MEMORY: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    static DEALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

struct AcceptanceAllocator;

#[global_allocator]
static ACCEPTANCE_ALLOCATOR: AcceptanceAllocator = AcceptanceAllocator;

unsafe impl GlobalAlloc for AcceptanceAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record_allocation();
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record_allocation();
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        record_deallocation();
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record_allocation();
        record_deallocation();
        unsafe { System.realloc(pointer, layout, new_size) }
    }
}

fn record_allocation() {
    let _ = COUNT_MEMORY.try_with(|enabled| {
        if enabled.get() {
            let _ = ALLOCATIONS.try_with(|count| count.set(count.get().saturating_add(1)));
        }
    });
}

fn record_deallocation() {
    let _ = COUNT_MEMORY.try_with(|enabled| {
        if enabled.get() {
            let _ = DEALLOCATIONS.try_with(|count| count.set(count.get().saturating_add(1)));
        }
    });
}

fn begin_memory_count() {
    ALLOCATIONS.with(|count| count.set(0));
    DEALLOCATIONS.with(|count| count.set(0));
    COUNT_MEMORY.with(|enabled| enabled.set(true));
}

fn finish_memory_count() -> (usize, usize) {
    COUNT_MEMORY.with(|enabled| enabled.set(false));
    (ALLOCATIONS.with(Cell::get), DEALLOCATIONS.with(Cell::get))
}

#[test]
fn owned_expression_render_and_cleanup_allocate_and_destroy_nothing() {
    const CHILD: &str = "CREST_BRAIDS_OWNED_EXPRESSION_CHILD";
    if std::env::var_os(CHILD).is_none() {
        // Native lifecycle counters are process-wide. Isolate this witness
        // from the production stress test's simultaneous bank destruction.
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "owned_expression_render_and_cleanup_allocate_and_destroy_nothing",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(
            child.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&child.stdout),
            String::from_utf8_lossy(&child.stderr)
        );
        assert!(String::from_utf8_lossy(&child.stdout).contains("1 passed"));
        return;
    }
    prove_owned_expression_callback_safety();
}

fn prove_owned_expression_callback_safety() {
    use crest_synth::synth::{
        MidiExpressionScope::{Master, Member},
        MidiSource, MidiVoiceId,
    };
    use MidiMessageKind::{ChannelPressure, ControlChange as Cc, NoteOff, NoteOn, PitchBend};
    let patch = braids_patch(1, 1);
    let parameters = RtPatchParameters::projected(
        patch.id(),
        PatchOutput::default(),
        VoiceEnvelope::new(0.0, 0.0, 1.0, 10.0).unwrap(),
        RtInstrumentParameters::new(&[3.0, 0.5, 0.5]).unwrap(),
    );
    let mut prepared = BraidsPreparer::new()
        .unwrap()
        .prepare(&patch, SAMPLE_RATE, 64)
        .unwrap();
    let first = Some(MidiVoiceId::new(1).unwrap());
    let second = Some(MidiVoiceId::new(2).unwrap());
    let scheduled = MidiSource::Scheduled;
    let monitor = MidiSource::Monitor;
    let events = [
        (scheduled, first, NoteOn, 60, 100, true),
        (scheduled, first, Cc, 101, 0, true),
        (scheduled, first, Cc, 100, 0, true),
        (scheduled, first, Cc, 6, 48, true),
        (scheduled, first, Cc, 38, 50, true),
        (scheduled, first, PitchBend, 127, 127, true),
        (scheduled, first, Cc, 74, 120, true),
        (scheduled, first, Cc, 64, 127, true),
        (scheduled, first, NoteOff, 60, 0, true),
        (scheduled, second, NoteOn, 60, 100, true),
        (scheduled, second, Cc, 64, 0, true),
        (scheduled, second, PitchBend, 0, 32, true),
        (scheduled, second, Cc, 74, 15, true),
        (monitor, first, NoteOn, 60, 100, true),
        (monitor, first, Cc, 64, 127, true),
        (monitor, first, NoteOff, 60, 0, true),
        (scheduled, None, Cc, 64, 0, true),
        (scheduled, first, Cc, 38, 60, true),
        (scheduled, first, Cc, 38, 100, false),
        (scheduled, first, Cc, 6, 100, false),
        (scheduled, first, PitchBend, 0, 0, true),
        (scheduled, second, NoteOff, 60, 0, true),
        (scheduled, first, Cc, 64, 0, true),
    ]
    .map(|(source, id, kind, data1, data2, accepted)| {
        (
            source,
            id,
            note(patch.channel(), kind, data1, data2),
            accepted,
        )
    });
    let scoped_events = [
        (scheduled, 1, Member, NoteOn, 60, 100, true),
        (scheduled, 1, Member, Cc, 101, 0, true),
        (scheduled, 1, Member, Cc, 100, 0, true),
        (scheduled, 1, Member, Cc, 6, 48, true),
        (scheduled, 1, Member, PitchBend, 0, 96, true),
        (scheduled, 1, Master, Cc, 101, 0, true),
        (scheduled, 1, Master, Cc, 100, 0, true),
        (scheduled, 1, Master, Cc, 6, 12, true),
        (scheduled, 1, Master, PitchBend, 0, 32, true),
        (scheduled, 1, Master, ChannelPressure, 64, 0, true),
        (scheduled, 1, Member, ChannelPressure, 32, 0, true),
        (scheduled, 1, Master, Cc, 64, 127, true),
        (scheduled, 1, Member, NoteOff, 60, 0, true),
        (scheduled, 2, Member, NoteOn, 60, 100, true),
        (scheduled, 1, Member, Cc, 64, 0, true),
        (scheduled, 2, Master, Cc, 101, 0, true),
        (scheduled, 2, Master, Cc, 100, 0, true),
        (scheduled, 2, Master, Cc, 6, 100, true),
        (scheduled, 2, Master, PitchBend, 0, 96, true),
        (scheduled, 2, Member, PitchBend, 127, 127, true),
        (scheduled, 2, Master, PitchBend, 127, 127, false),
        (scheduled, 2, Master, Cc, 74, 10, false),
        (scheduled, 2, Master, NoteOff, 60, 0, false),
        (monitor, 1, Master, Cc, 64, 127, true),
        (monitor, 1, Member, Cc, 64, 0, true),
        (scheduled, 1, Master, Cc, 64, 0, true),
    ]
    .map(|(source, id, scope, kind, data1, data2, accepted)| {
        (
            source,
            MidiVoiceId::new(id).unwrap(),
            scope,
            note(patch.channel(), kind, data1, data2),
            accepted,
        )
    });
    let mut output = [0.0_f32; 128];
    let native_before = braids_lifecycle_counts();
    let mut correct = true;
    begin_memory_count();
    for (source, id, event, accepted) in events {
        correct &= prepared
            .dispatch_voice_from(source, id, event, &parameters)
            .is_ok()
            == accepted;
        correct &= prepared.render(&mut output, 64, &parameters).is_ok();
        correct &= finite(&output) && sounding(&output);
    }
    correct &= prepared.all_notes_off_from(scheduled).is_ok();
    for (source, id, scope, event, accepted) in scoped_events {
        correct &= prepared
            .dispatch_scoped_voice_from(source, id, scope, event, &parameters)
            .is_ok()
            == accepted;
        correct &= prepared.render(&mut output, 64, &parameters).is_ok();
        correct &= finite(&output) && sounding(&output);
    }
    correct &= prepared.all_notes_off_from(scheduled).is_ok();
    correct &= prepared.active_note_count() == Some(1);
    correct &= prepared.render(&mut output, 64, &parameters).is_ok();
    correct &= finite(&output) && sounding(&output);
    correct &= prepared.all_notes_off_from(monitor).is_ok();
    correct &= prepared.render(&mut output, 64, &parameters).is_ok();
    correct &= output.iter().all(|sample| *sample == 0.0);
    let memory = finish_memory_count();
    let native_after = braids_lifecycle_counts();
    assert!(
        correct,
        "owned dispatch, render, or isolated cleanup failed"
    );
    assert_eq!(memory, (0, 0));
    assert_eq!(native_after.created, native_before.created);
    assert_eq!(native_after.destroyed, native_before.destroyed);
}

#[derive(Serialize)]
struct BraidsObservation {
    upstream_revision: &'static str,
    stmlib_revision: &'static str,
    source_hashes_match: bool,
    model_count: usize,
    voices_per_patch: usize,
    braids_patch_count: usize,
    total_braids_voice_capacity: usize,
    capacity_matches_patch_count: bool,
    no_braids_specific_patch_limit: bool,
    independent_patch_banks: bool,
    sixteen_voices_audible: bool,
    exhaustion_preserves_sounding_voices: bool,
    scalar_cases_exercised: usize,
    unsupported_rate_rejected: bool,
    mixed_routing_exact: bool,
    parameter_isolation_exact: bool,
    finite_audio: bool,
    callback_allocations: usize,
    callback_destructions: usize,
    native_callback_destructions: u64,
    p99_render_microseconds: u64,
}

#[test]
fn pinned_braids_engine_satisfies_the_mixed_production_contract() {
    assert_eq!(usize::from(BRAIDS_MODEL_COUNT), BRAIDS_MODELS.len());
    assert_eq!(usize::from(BRAIDS_FIXED_VOICES), BRAIDS_VOICE_COUNT);
    let source_hashes_match = source_hashes_match();
    assert!(source_hashes_match);

    let provider = BraidsCapability::new().expect("Braids descriptor is valid");
    let descriptor = provider.descriptor();
    assert_eq!(descriptor.id().as_str(), BRAIDS_CAPABILITY_ID);
    assert_eq!(descriptor.scalar_parameter_count(), 3);
    assert_eq!(descriptor.parameters().count(), 3);
    assert_eq!(descriptor.parameters().next().unwrap().choices().len(), 47);
    let default_config = provider
        .default_config()
        .expect("Braids defaults are valid");
    descriptor
        .create_config(default_config.values(), default_config.asset_references())
        .expect("Braids defaults round-trip exactly");

    let three_braids_patches = braids_patches(3, 1);
    let braids_registry = CapabilityRegistry::new(vec![descriptor.clone()]).unwrap();
    let braids_preparers: Vec<Box<dyn InstrumentPreparer>> =
        vec![Box::new(BraidsPreparer::new().unwrap())];
    let lifecycle_before = braids_lifecycle_counts();
    let rack = PreparedEngineRackBuilder::build(
        &three_braids_patches,
        &braids_registry,
        &braids_preparers,
        SAMPLE_RATE,
        BLOCK_FRAMES,
    )
    .expect("three independent Braids Patches prepare");
    let lifecycle_during = braids_lifecycle_counts();
    let independent_patch_banks = rack.patch_count() == 3
        && lifecycle_during
            .active
            .saturating_sub(lifecycle_before.active)
            == 3;
    assert!(independent_patch_banks);
    drop(rack);
    assert_eq!(braids_lifecycle_counts().active, lifecycle_before.active);

    let maximum_braids_patches = braids_patches(MAX_ACTIVE_PATCHES, 1);
    let maximum_rack = PreparedEngineRackBuilder::build(
        &maximum_braids_patches,
        &braids_registry,
        &braids_preparers,
        SAMPLE_RATE,
        BLOCK_FRAMES,
    )
    .expect("Braids uses the engine-agnostic rack Patch capacity");
    let no_braids_specific_patch_limit = maximum_rack.patch_count() == MAX_ACTIVE_PATCHES;
    assert!(no_braids_specific_patch_limit);
    drop(maximum_rack);

    let preparer = BraidsPreparer::new().unwrap();
    let probe_patch = braids_patch(1, 0).with_voice_limit(16).unwrap();
    let parameters = braids_parameters(probe_patch.id(), [0.0, 0.5, 0.5]);
    let mut sixteen = preparer
        .prepare(&probe_patch, SAMPLE_RATE, BLOCK_FRAMES)
        .unwrap();
    for key in 40..56 {
        sixteen
            .dispatch(
                note(probe_patch.channel(), MidiMessageKind::NoteOn, key, 112),
                &parameters,
            )
            .unwrap();
    }
    let mut sixteen_output = [0.0_f32; BLOCK_FRAMES * 2];
    sixteen
        .render(&mut sixteen_output, BLOCK_FRAMES, &parameters)
        .unwrap();
    let sixteen_voices_audible = sounding(&sixteen_output) && finite(&sixteen_output);
    assert!(sixteen_voices_audible);

    let exhaustion_preserves_sounding_voices =
        prove_exhaustion_preserves_sounding_voices(&preparer, &probe_patch, &parameters);
    assert!(exhaustion_preserves_sounding_voices);

    let baseline = render_braids_scalar_case(&preparer, &probe_patch, [0.0, 0.5, 0.5]);
    let scalar_variants = [
        render_braids_scalar_case(&preparer, &probe_patch, [1.0, 0.5, 0.5]),
        render_braids_scalar_case(&preparer, &probe_patch, [0.0, 0.8, 0.5]),
        render_braids_scalar_case(&preparer, &probe_patch, [0.0, 0.5, 0.8]),
    ];
    let scalar_cases_exercised = scalar_variants
        .iter()
        .filter(|variant| {
            finite(variant.as_slice())
                && sounding(variant.as_slice())
                && variant.as_slice() != baseline.as_slice()
        })
        .count();
    assert_eq!(scalar_cases_exercised, 3);

    let unsupported_rate_rejected = matches!(
        preparer.prepare(&probe_patch, 44_100.0, BLOCK_FRAMES),
        Err(InstrumentPreparationError::InvalidSampleRate)
    ) && malformed_config_is_rejected(&preparer, &probe_patch);
    assert!(unsupported_rate_rejected);

    let (mixed_routing_exact, parameter_isolation_exact) = prove_mixed_routing_and_isolation();
    assert!(mixed_routing_exact);
    assert!(parameter_isolation_exact);

    let (
        p99_render_microseconds,
        callback_allocations,
        callback_destructions,
        native_callback_destructions,
        finite_audio,
    ) = measure_worst_case_mixed_callback();
    assert_eq!(callback_allocations, 0);
    assert_eq!(callback_destructions, 0);
    assert_eq!(native_callback_destructions, 0);
    assert!(finite_audio);
    if !cfg!(debug_assertions) {
        assert!(
            p99_render_microseconds < 2_666,
            "release-profile p99 render time was {p99_render_microseconds} microseconds"
        );
    }

    let observation = BraidsObservation {
        upstream_revision: UPSTREAM_REVISION,
        stmlib_revision: STMLIB_REVISION,
        source_hashes_match,
        model_count: BRAIDS_MODELS.len(),
        voices_per_patch: BRAIDS_VOICE_COUNT,
        braids_patch_count: three_braids_patches.len(),
        total_braids_voice_capacity: three_braids_patches.len() * BRAIDS_VOICE_COUNT,
        capacity_matches_patch_count: three_braids_patches.len() * BRAIDS_VOICE_COUNT == 48,
        no_braids_specific_patch_limit,
        independent_patch_banks,
        sixteen_voices_audible,
        exhaustion_preserves_sounding_voices,
        scalar_cases_exercised,
        unsupported_rate_rejected,
        mixed_routing_exact,
        parameter_isolation_exact,
        finite_audio,
        callback_allocations,
        callback_destructions,
        native_callback_destructions,
        p99_render_microseconds,
    };
    println!(
        "CREST_BRAIDS_OBSERVATION {}",
        serde_json::to_string(&observation).unwrap()
    );
    println!("CREST_ACCEPTANCE braids_engine passed");
}

fn source_hashes_match() -> bool {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor/braids");
    let provenance = fs::read_to_string(root.join("PROVENANCE.md")).unwrap();
    if !provenance.contains(UPSTREAM_REVISION) || !provenance.contains(STMLIB_REVISION) {
        return false;
    }
    SOURCE_HASHES.iter().all(|(relative, expected)| {
        let bytes = fs::read(root.join(relative)).unwrap();
        let actual = format!("{:x}", Sha256::digest(bytes));
        actual == *expected && provenance.contains(&format!("{expected}  {relative}"))
    })
}

fn globals() -> GlobalParameters {
    GlobalParameters::new(-6.0).unwrap()
}

fn braids_patch(id: u32, channel: u8) -> Patch {
    Patch::new(
        PatchId::new(id).unwrap(),
        format!("Braids {id}"),
        BraidsCapability::new().unwrap().default_config().unwrap(),
        MidiChannel::new(channel).unwrap(),
        PatchOutput::to_track(MixerTrackId::new(channel).unwrap()),
    )
}

fn braids_patches(count: usize, first_id: u32) -> Vec<Patch> {
    (0..count)
        .map(|index| braids_patch(first_id + index as u32, index as u8))
        .collect()
}

fn soundfont_patch(id: u32, channel: u8) -> Patch {
    let provider =
        crest_synth::adapter::production_instruments::production_soundfont_capability().unwrap();
    Patch::new(
        PatchId::new(id).unwrap(),
        format!("SoundFont {id}"),
        create_soundfont_config(&provider, SoundFontInstrument::new(0, 0, false).unwrap()).unwrap(),
        MidiChannel::new(channel).unwrap(),
        PatchOutput::to_track(MixerTrackId::new(channel).unwrap()),
    )
}

fn mixed_patches(count: usize) -> Vec<Patch> {
    (0..count)
        .map(|index| {
            let id = index as u32 + 1;
            let channel = index as u8;
            if index % 2 == 0 {
                soundfont_patch(id, channel)
            } else {
                braids_patch(id, channel)
            }
        })
        .collect()
}

fn installed_state(registry: CapabilityRegistry, patches: Vec<Patch>) -> AppState {
    let mut state = AppState::new(registry, globals());
    state.apply(AppEvent::InstallPatches(patches)).unwrap();
    state
}

fn projected_parameters(state: &AppState) -> ParameterSnapshot {
    StateProjector::for_graph(GraphRevision::INITIAL)
        .project(state)
        .unwrap()
        .2
}

fn braids_parameters(patch_id: PatchId, scalars: [f32; 3]) -> RtPatchParameters {
    RtPatchParameters::projected(
        patch_id,
        PatchOutput::default(),
        VoiceEnvelope::DEFAULT,
        RtInstrumentParameters::new(&scalars).unwrap(),
    )
}

fn note(channel: MidiChannel, kind: MidiMessageKind, key: u8, velocity: u8) -> MidiMessage {
    MidiMessage::try_new(channel, kind, key, velocity).unwrap()
}

fn finite(output: &[f32]) -> bool {
    output.iter().all(|sample| sample.is_finite())
}

fn sounding(output: &[f32]) -> bool {
    output.iter().any(|sample| sample.abs() > 1.0e-6)
}

fn prove_exhaustion_preserves_sounding_voices(
    preparer: &BraidsPreparer,
    patch: &Patch,
    parameters: &RtPatchParameters,
) -> bool {
    let mut full = preparer.prepare(patch, SAMPLE_RATE, BLOCK_FRAMES).unwrap();
    let mut control = preparer.prepare(patch, SAMPLE_RATE, BLOCK_FRAMES).unwrap();
    for key in 40..56 {
        let message = note(patch.channel(), MidiMessageKind::NoteOn, key, 112);
        full.dispatch(message, parameters).unwrap();
        control.dispatch(message, parameters).unwrap();
    }
    assert!(matches!(
        full.dispatch(
            note(patch.channel(), MidiMessageKind::NoteOn, 56, 112),
            parameters
        ),
        Err(crest_synth::synth::PreparedInstrumentError::DispatchRejected)
    ));
    let mut after_refusal = [0.0_f32; BLOCK_FRAMES * 2];
    let mut reference = [0.0_f32; BLOCK_FRAMES * 2];
    full.render(&mut after_refusal, BLOCK_FRAMES, parameters)
        .unwrap();
    control
        .render(&mut reference, BLOCK_FRAMES, parameters)
        .unwrap();
    after_refusal == reference && sounding(&after_refusal) && finite(&after_refusal)
}

fn render_braids_scalar_case(
    preparer: &BraidsPreparer,
    patch: &Patch,
    scalars: [f32; 3],
) -> [f32; BLOCK_FRAMES * 2] {
    let parameters = braids_parameters(patch.id(), scalars);
    let mut instrument = preparer.prepare(patch, SAMPLE_RATE, BLOCK_FRAMES).unwrap();
    instrument
        .dispatch(
            note(patch.channel(), MidiMessageKind::NoteOn, 60, 120),
            &parameters,
        )
        .unwrap();
    let mut output = [0.0_f32; BLOCK_FRAMES * 2];
    instrument
        .render(&mut output, BLOCK_FRAMES, &parameters)
        .unwrap();
    output
}

fn malformed_config_is_rejected(preparer: &BraidsPreparer, patch: &Patch) -> bool {
    let malformed = Patch::new(
        PatchId::new(99).unwrap(),
        "Malformed Braids".to_owned(),
        InstrumentConfig::from_parts(
            CapabilityId::new(BRAIDS_CAPABILITY_ID).unwrap(),
            patch.instrument_config().values()[..2].to_vec(),
            Vec::new(),
        ),
        MidiChannel::new(0).unwrap(),
        PatchOutput::default(),
    );
    matches!(
        preparer.prepare(&malformed, SAMPLE_RATE, BLOCK_FRAMES),
        Err(InstrumentPreparationError::InvalidConfiguration { .. })
    )
}

fn prove_mixed_routing_and_isolation() -> (bool, bool) {
    let registry = production_capability_registry().unwrap();
    let patches = mixed_patches(4);
    let baseline_state = installed_state(registry.clone(), patches);
    let mut edited_state = baseline_state.clone();
    edited_state
        .apply(AppEvent::Navigate(Direction::Right))
        .unwrap();
    edited_state
        .apply(AppEvent::Adjust(Direction::Right))
        .unwrap();

    let baseline_parameters = projected_parameters(&baseline_state);
    let edited_parameters = projected_parameters(&edited_state);
    let preparers = production_instrument_preparers().unwrap();
    let baseline_graph = PreparedGraphBuilder::new(&registry, &preparers)
        .build(
            GraphRevision::INITIAL,
            baseline_state.patches(),
            baseline_parameters.clone(),
            SAMPLE_RATE,
            BLOCK_FRAMES,
        )
        .unwrap();
    let edited_graph = PreparedGraphBuilder::new(&registry, &preparers)
        .build(
            GraphRevision::INITIAL,
            edited_state.patches(),
            edited_parameters.clone(),
            SAMPLE_RATE,
            BLOCK_FRAMES,
        )
        .unwrap();
    let baseline_boundary = LockFreeAudioBoundary::new(32, baseline_parameters);
    let edited_boundary = LockFreeAudioBoundary::new(32, edited_parameters);
    let (mut baseline_control, baseline_audio) = baseline_boundary.into_handles();
    let (mut edited_control, edited_audio) = edited_boundary.into_handles();
    for patch in baseline_state.patches() {
        let command = AudioCommand::patch_midi(
            patch.id(),
            note(patch.channel(), MidiMessageKind::NoteOn, 60, 110),
        );
        baseline_control.push_command(command).unwrap();
        edited_control.push_command(command).unwrap();
    }
    let mut baseline_renderer = AudioRenderer::new(
        baseline_audio,
        NoStructuralGraphChanges::new(),
        baseline_graph,
    );
    let mut edited_renderer =
        AudioRenderer::new(edited_audio, NoStructuralGraphChanges::new(), edited_graph);
    let mut baseline_output = [0.0_f32; BLOCK_FRAMES * 2];
    let mut edited_output = [0.0_f32; BLOCK_FRAMES * 2];
    baseline_renderer.render(&mut baseline_output);
    edited_renderer.render(&mut edited_output);

    let baseline_stems = baseline_renderer.active_patch_audio();
    let edited_stems = edited_renderer.active_patch_audio();
    let mixed_routing_exact = baseline_state
        .patches()
        .iter()
        .enumerate()
        .all(|(index, patch)| {
            baseline_stems
                .stem(index, patch.id())
                .is_some_and(|stem| finite(stem.samples()) && sounding(stem.samples()))
                && edited_stems
                    .stem(index, patch.id())
                    .is_some_and(|stem| finite(stem.samples()) && sounding(stem.samples()))
        });
    let target_track = MixerTrackId::new(1).unwrap();
    let parameter_isolation_exact =
        baseline_state
            .patches()
            .iter()
            .enumerate()
            .all(|(index, patch)| {
                baseline_stems.stem(index, patch.id()).unwrap().samples()
                    == edited_stems.stem(index, patch.id()).unwrap().samples()
            })
            && baseline_state.mixer().track(target_track).level_db() == 0.0
            && edited_state.mixer().track(target_track).level_db() == 1.0
            && MixerTrackId::ALL
                .into_iter()
                .filter(|track_id| *track_id != target_track)
                .all(|track_id| {
                    baseline_state.mixer().track(track_id) == edited_state.mixer().track(track_id)
                })
            && baseline_output != edited_output;
    (mixed_routing_exact, parameter_isolation_exact)
}

fn measure_worst_case_mixed_callback() -> (u64, usize, usize, u64, bool) {
    const MEASURED_BLOCKS: usize = 256;
    let registry = production_capability_registry().unwrap();
    let state = installed_state(registry.clone(), mixed_patches(MAX_ACTIVE_PATCHES));
    let parameters = projected_parameters(&state);
    let preparers = production_instrument_preparers().unwrap();
    let graph = PreparedGraphBuilder::new(&registry, &preparers)
        .build(
            GraphRevision::INITIAL,
            state.patches(),
            parameters.clone(),
            SAMPLE_RATE,
            BLOCK_FRAMES,
        )
        .unwrap();
    let boundary = LockFreeAudioBoundary::new(256, parameters);
    let (mut control, audio) = boundary.into_handles();
    for patch in state.patches() {
        let note_count =
            if patch.instrument_config().capability_id().as_str() == BRAIDS_CAPABILITY_ID {
                BRAIDS_VOICE_COUNT
            } else {
                4
            };
        for offset in 0..note_count {
            control
                .push_command(AudioCommand::patch_midi(
                    patch.id(),
                    note(
                        patch.channel(),
                        MidiMessageKind::NoteOn,
                        36 + offset as u8,
                        96,
                    ),
                ))
                .unwrap();
        }
    }
    let mut renderer = AudioRenderer::new(audio, NoStructuralGraphChanges::new(), graph);
    let mut output = [0.0_f32; BLOCK_FRAMES * 2];
    for _ in 0..8 {
        renderer.render(&mut output);
    }

    let native_before = braids_lifecycle_counts();
    let mut durations = Vec::with_capacity(MEASURED_BLOCKS);
    let mut allocations = 0;
    let mut deallocations = 0;
    let mut finite_audio = true;
    for _ in 0..MEASURED_BLOCKS {
        let started = Instant::now();
        begin_memory_count();
        renderer.render(&mut output);
        let memory = finish_memory_count();
        durations.push(started.elapsed().as_micros() as u64);
        allocations += memory.0;
        deallocations += memory.1;
        finite_audio &= finite(&output) && sounding(&output);
    }
    let native_after = braids_lifecycle_counts();
    durations.sort_unstable();
    let p99_index = (MEASURED_BLOCKS * 99).div_ceil(100).saturating_sub(1);
    (
        durations[p99_index],
        allocations,
        deallocations,
        native_after
            .destroyed
            .saturating_sub(native_before.destroyed),
        finite_audio,
    )
}
