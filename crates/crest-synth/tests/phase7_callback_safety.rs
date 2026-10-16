//! Phase 7 callback audit over the production Sample instrument and audition.
//!
//! The measured region is only `AudioRenderer::render`, exactly as the device
//! adapter invokes it. Preparation, command publication, graph publication,
//! and retired-graph collection remain on worker/control ownership.

use core::alloc::{GlobalAlloc, Layout};
use core::cell::Cell;
use crest_synth::adapter::lock_free_audio_boundary::LockFreeAudioBoundary;
use crest_synth::adapter::lock_free_structural_graph_boundary::LockFreeStructuralGraphBoundary;
use crest_synth::adapter::sample_capability::SampleCapability;
use crest_synth::adapter::sample_preparer::SamplePreparer;
use crest_synth::kernel::midi_message::{MidiMessage, MidiMessageKind};
use crest_synth::kernel::{MidiChannel, PatchId};
use crest_synth::mixer::global_parameters::GlobalParameters;
use crest_synth::mixer::mixer_state::MixerState;
use crest_synth::mixer::patch_output::PatchOutput;
use crest_synth::real_time::{
    AudioBoundary, AudioCommand, AudioRenderer, AuditionPreparationRequest, ControlAudioBoundary,
    ControlStructuralGraphBoundary, GraphHandoffStatus, GraphRevision, ParameterSnapshot,
    PreparedGraph, PreparedGraphBuilder, StructuralGraphBoundary,
};
use crest_synth::synth::{
    AssetFileId, CapabilityRegistry, DecodedSample, InstrumentCapabilityProvider,
    InstrumentPreparer, Patch, SampleEncoding, SampleMetadata,
};
use crest_synth::testing::{DeterministicSampleCatalog, DeterministicSampleDecoder};
use std::alloc::System;
use std::sync::Arc;

const SAMPLE_RATE: f32 = 48_000.0;
const MAX_FRAMES: usize = 64;
const AUDITION_ID: u64 = 77;

thread_local! {
    static COUNT_MEMORY: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    static DEALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

struct AuditAllocator;

#[global_allocator]
static AUDIT_ALLOCATOR: AuditAllocator = AuditAllocator;

unsafe impl GlobalAlloc for AuditAllocator {
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

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record_allocation();
        record_deallocation();
        unsafe { System.realloc(pointer, layout, size) }
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

fn counted_render<Boundary, Structural>(
    renderer: &mut AudioRenderer<Boundary, Structural>,
    output: &mut [f32],
) -> (usize, usize)
where
    Boundary: crest_synth::real_time::AudioThreadBoundary,
    Structural: crest_synth::real_time::AudioStructuralGraphBoundary,
{
    ALLOCATIONS.with(|count| count.set(0));
    DEALLOCATIONS.with(|count| count.set(0));
    COUNT_MEMORY.with(|enabled| enabled.set(true));
    renderer.render(output);
    COUNT_MEMORY.with(|enabled| enabled.set(false));
    (ALLOCATIONS.with(Cell::get), DEALLOCATIONS.with(Cell::get))
}

struct Fixture {
    patch_id: PatchId,
    patch: Patch,
    registry: CapabilityRegistry,
    preparers: Vec<Box<dyn InstrumentPreparer>>,
}

impl Fixture {
    fn new() -> Self {
        let asset = AssetFileId::new("audit.wav").unwrap();
        let capability = SampleCapability::new(asset.clone()).unwrap();
        let patch_id = PatchId::new(1).unwrap();
        let patch = Patch::new(
            patch_id,
            "Audited Sample".to_owned(),
            capability.default_config().unwrap(),
            MidiChannel::new(0).unwrap(),
            PatchOutput::default(),
        );
        let frames = 2_048_u64;
        let pcm = (0..frames)
            .flat_map(|frame| {
                let sample = ((frame as f32 / 19.0).sin() * 0.4).clamp(-1.0, 1.0);
                [sample, -sample]
            })
            .collect::<Vec<_>>();
        let decoded = DecodedSample::new(
            SampleMetadata::new(
                asset.clone(),
                64,
                SAMPLE_RATE as u32,
                2,
                32,
                SampleEncoding::Float,
                frames,
            )
            .unwrap(),
            pcm,
        )
        .unwrap();
        let catalog = Arc::new(DeterministicSampleCatalog::new(
            [],
            [(asset.clone(), Ok(vec![1]))],
        ));
        let decoder = Arc::new(DeterministicSampleDecoder::new([(asset, Ok(decoded))]));
        Self {
            patch_id,
            patch,
            registry: CapabilityRegistry::new(vec![capability.descriptor()]).unwrap(),
            preparers: vec![Box::new(SamplePreparer::new(catalog, decoder).unwrap())],
        }
    }

    fn parameters(&self, generation: u64, revision: GraphRevision) -> ParameterSnapshot {
        ParameterSnapshot::project_patches(
            generation,
            revision,
            GlobalParameters::new(0.0).unwrap(),
            MixerState::default(),
            core::slice::from_ref(&self.patch),
            &self.registry,
        )
        .unwrap()
    }

    fn graph(&self, generation: u64, revision: GraphRevision, audition: bool) -> PreparedGraph {
        let parameters = self.parameters(generation, revision);
        let request = audition.then(|| {
            AuditionPreparationRequest::new(
                AUDITION_ID,
                self.patch_id,
                self.patch.instrument_config().clone(),
            )
            .unwrap()
        });
        let builder = PreparedGraphBuilder::new(&self.registry, &self.preparers);
        let builder = match request.as_ref() {
            Some(request) => builder.with_audition(request),
            None => builder,
        };
        builder
            .build(
                revision,
                core::slice::from_ref(&self.patch),
                parameters,
                SAMPLE_RATE,
                MAX_FRAMES,
            )
            .unwrap()
    }
}

fn midi(kind: MidiMessageKind, note: u8, velocity: u8) -> MidiMessage {
    MidiMessage::try_new(MidiChannel::new(0).unwrap(), kind, note, velocity).unwrap()
}

#[test]
fn sample_dispatch_render_audition_swap_and_retirement_are_callback_allocation_and_destruction_free(
) {
    let fixture = Fixture::new();
    let initial_revision = GraphRevision::INITIAL;
    let audition_revision = initial_revision.checked_next().unwrap();
    let replacement_revision = audition_revision.checked_next().unwrap();
    let initial_graph = fixture.graph(0, initial_revision, false);
    let audition_graph = fixture.graph(1, audition_revision, true);
    let replacement_graph = fixture.graph(2, replacement_revision, false);

    let audio = LockFreeAudioBoundary::new(16, fixture.parameters(0, initial_revision));
    let (mut audio_control, audio_callback) = audio.into_handles();
    let structural = LockFreeStructuralGraphBoundary::new(
        2,
        2,
        GraphHandoffStatus::with_active(initial_revision),
    )
    .unwrap();
    let (mut structural_control, structural_callback) = structural.into_handles();
    let mut renderer = AudioRenderer::new(audio_callback, structural_callback, initial_graph);
    let mut output = [0.0_f32; MAX_FRAMES * 2];
    let mut memory = Vec::new();

    audio_control
        .push_command(AudioCommand::patch_midi(
            fixture.patch_id,
            midi(MidiMessageKind::NoteOn, 60, 110),
        ))
        .unwrap();
    memory.push(counted_render(&mut renderer, &mut output));
    assert!(output.iter().any(|sample| sample.abs() > 1.0e-5));

    audio_control
        .push_command(AudioCommand::all_notes_off())
        .unwrap();
    memory.push(counted_render(&mut renderer, &mut output));

    structural_control
        .publish_prepared_on_control(audition_graph)
        .unwrap();
    memory.push(counted_render(&mut renderer, &mut output));
    assert_eq!(renderer.active_revision(), audition_revision);
    assert_eq!(
        structural_control.collect_retired_on_control(),
        Some(initial_revision),
        "the superseded graph is destroyed only after returning to control ownership"
    );

    audio_control
        .push_command(AudioCommand::preview_start(fixture.patch_id, AUDITION_ID))
        .unwrap();
    memory.push(counted_render(&mut renderer, &mut output));
    assert!(output.iter().any(|sample| sample.abs() > 1.0e-5));

    audio_control
        .push_command(AudioCommand::preview_stop(fixture.patch_id, AUDITION_ID))
        .unwrap();
    for _ in 0..5 {
        memory.push(counted_render(&mut renderer, &mut output));
    }

    structural_control
        .publish_prepared_on_control(replacement_graph)
        .unwrap();
    memory.push(counted_render(&mut renderer, &mut output));
    assert_eq!(renderer.active_revision(), replacement_revision);
    assert_eq!(
        structural_control.collect_retired_on_control(),
        Some(audition_revision),
        "the graph that owns audition PCM also retires and drops off-callback"
    );

    assert!(
        memory.iter().all(|counts| *counts == (0, 0)),
        "every Sample callback region must allocate and destroy zero values: {memory:?}"
    );
}

#[test]
fn sample_callback_implementations_have_no_lock_io_log_or_panic_surface() {
    let source = include_str!("../src/adapter/sample_preparer.rs");
    let start = source
        .find("struct UpstreamSampleInstrument")
        .expect("the audited audition implementation exists");
    let end = start
        + source[start..]
            .find("#[cfg(test)]")
            .expect("unit tests delimit the callback-only implementations");
    let callback_source = &source[start..end];
    for forbidden in [
        ".lock(",
        "Mutex",
        "std::fs",
        "File::",
        "PathBuf",
        "println!",
        "eprintln!",
        "log::",
        "tracing::",
        "panic!",
        ".unwrap(",
        ".expect(",
        "Vec<",
        "String",
    ] {
        assert!(
            !callback_source.contains(forbidden),
            "callback implementation admitted forbidden surface {forbidden:?}"
        );
    }
}
