use anyhow::{bail, Context, Result};
use core::alloc::{GlobalAlloc, Layout};
use crest_synth::adapter::braids_capability::BRAIDS_CAPABILITY_ID;
use crest_synth::adapter::hidef_soundfont_capability::HIDEF_CAPABILITY_ID;
use crest_synth::control::app_event::AppEvent;
use crest_synth::control::app_state::EventRejection;
use crest_synth::control::event_record::{EmittedEvent, EventInput, EventOutcome, EventSource};
use crest_synth::kernel::midi_message::MidiMessageKind;
use crest_synth::mixer::global_parameters::GlobalParameters;
use crest_synth::mixer::mixer_track_id::MixerTrackId;
use crest_synth::mixer::mixer_track_parameters::MixerTrackParameters;
use crest_synth::mixer::patch_output::PatchOutput;
use crest_synth::real_time::GraphRevision;
use crest_synth::shell::standalone_application::{
    production_application, ApplicationConfig, DegenerateMode, LiveSceneKind,
};
use crest_synth::shell::webview::TauriWebviewWindow;
use crest_synth::shell::window_input::WindowInput;
use crest_synth::synth::{CapabilityId, EffectCapabilityId};
use crest_synth::testing::demo_scene_report::{DemoCoverageGroup, DemoSceneReport};
use crest_synth::testing::full_instrument_effect_demo::FullDemoSelection;
use crest_synth::testing::{
    BehavioralMutationCase, BehavioralMutationHarness, BehavioralMutationObservation,
    LiveDemoReport, LIVE_DEMO_NO_PROGRESS_TIMEOUT, LIVE_DEMO_TOTAL_TIMEOUT,
};
use serde::Serialize;
use serde_json::Value;
use std::alloc::System;
use std::env;

struct CallbackMeasuringAllocator;

#[global_allocator]
static CALLBACK_MEASURING_ALLOCATOR: CallbackMeasuringAllocator = CallbackMeasuringAllocator;

unsafe impl GlobalAlloc for CallbackMeasuringAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        crest_synth::real_time::callback_safety::record_callback_allocation_from_allocator();
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        crest_synth::real_time::callback_safety::record_callback_allocation_from_allocator();
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        crest_synth::real_time::callback_safety::record_callback_deallocation_from_allocator();
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        crest_synth::real_time::callback_safety::record_callback_allocation_from_allocator();
        crest_synth::real_time::callback_safety::record_callback_deallocation_from_allocator();
        unsafe { System.realloc(pointer, layout, new_size) }
    }
}

fn main() -> Result<()> {
    let options = parse_options(env::args().skip(1))?;
    run(options)
}

fn full_demo_selection() -> Result<FullDemoSelection> {
    Ok(FullDemoSelection {
        effect_reference: CapabilityId::new(BRAIDS_CAPABILITY_ID)?,
        skipped_instruments: [
            HIDEF_CAPABILITY_ID,
            BRAIDS_CAPABILITY_ID,
            crest_synth::adapter::drum_rack_capability::DRUM_RACK_CAPABILITY_ID,
        ]
        .into_iter()
        .map(CapabilityId::new)
        .collect::<Result<_, _>>()?,
        skipped_effects: [
            crest_synth::adapter::chorus_capability::CHORUS_CAPABILITY_ID,
            crest_synth::adapter::reverb_capability::REVERB_CAPABILITY_ID,
            crest_synth::adapter::delay_capability::DELAY_CAPABILITY_ID,
        ]
        .into_iter()
        .map(EffectCapabilityId::new)
        .collect::<Result<_, _>>()?,
    })
}

fn run(options: Options) -> Result<()> {
    let _demo_sample_library = (options.demo_live_detail_and_assets || options.demo_live_drum_rack)
        .then(|| DemoSampleLibrary::create(options.demo_live_drum_rack))
        .transpose()?;
    let make_application = || -> Result<_> {
        let config = ApplicationConfig::default().with_test_midi_on_launch();
        // The webview shell is the product's one shell:
        // construction is direct, no launch flag chooses a renderer, and a
        // webview init failure is a typed startup error on the fatal path —
        // never an alternate window. Live modes do not use this interactive
        // window: `run_live_demo_scene` composes its own webview shell
        // window (WP03).
        let window = TauriWebviewWindow::default();
        production_application(window, config)
            .context("failed to validate the production composition")
    };

    if options.demo_live_drum_rack {
        make_application()?
            .with_drum_rack_demo()
            .run()
            .context("Drum Rack demo failed")?;
    } else if options.full_instrument_effect_demo {
        make_application()?
            .with_full_instrument_effect_demo(full_demo_selection()?)
            .run()
            .context("full instrument/effect demo failed")?;
    } else if options.demo_component_library {
        // Browsable rather than autonomous: no milestone timeout, no total
        // timeout, and no generation correlation. It finishes when the operator
        // closes the window.
        eprintln!(
            "crest-synth component gallery: browsable and operator-driven. Press 1-9 and 0 to select the first ten of fifteen pages. Then press [ until page 1 and ] fourteen times to step through all fifteen — the observation counts step reachability from what you actually visit, so a partial walk reports a partial count. Press any other key for one that binds no page, then close the window to finish. Page selection is scene-local and changes no application state."
        );
        let observation = crest_synth::testing::ComponentGalleryScene::new()
            .context("failed to construct the component gallery scene")?
            .run()
            .context("component gallery execution failed")?;
        let json = serde_json::to_string(&observation)
            .context("failed to serialize the component gallery observation")?;
        println!(
            "{}{json}",
            crest_synth::testing::COMPONENT_GALLERY_OBSERVATION_MARKER
        );
    } else if options.demo_live {
        let scene_kind = if options.demo_live_detail_and_assets {
            LiveSceneKind::DetailAndAssets {
                defeat_preview: options.defeat_detail_and_assets_preview,
            }
        } else if options.demo_live_mixer {
            LiveSceneKind::Mixer
        } else if options.demo_live_effects_and_buses {
            LiveSceneKind::EffectsAndBuses
        } else if options.demo_live_patch_editor {
            LiveSceneKind::FunctionalPatchEditor {
                defeat_patch_selection: options.defeat_patch_selection,
            }
        } else {
            LiveSceneKind::SixteenTrackMixerRouting
        };
        let total_timeout = if options.demo_live_detail_and_assets {
            std::time::Duration::from_secs(180)
        } else if options.demo_live_effects_and_buses {
            crest_synth::testing::live_effects_and_buses_scene::EFFECTS_AND_BUSES_TOTAL_TIMEOUT
        } else if options.demo_live_patch_editor {
            crest_synth::testing::live_patch_editor_scene::PATCH_EDITOR_TOTAL_TIMEOUT
        } else {
            LIVE_DEMO_TOTAL_TIMEOUT
        };
        if options.defeat_patch_selection {
            eprintln!(
                "crest-synth live demo: --defeat-patch-selection is the declared controlled negative. The patch-selection gesture is removed; the journey still runs, still succeeds step by step, and still visits three slots and makes an audible edit — on the FIRST instrument. The reach predicates are what must fail, and this process must exit 1."
            );
        }
        if options.defeat_detail_and_assets_preview {
            eprintln!(
                "crest-synth live demo: --defeat-detail-and-assets-preview removes the required semantic preview press/release; the report must exit non-zero on the named preview predicates"
            );
        }
        eprintln!(
            "crest-synth live demo: autonomous and input-isolated; expected duration about 90 seconds; no-progress timeout={}s; total timeout={}s; closing the window cancels the proof",
            LIVE_DEMO_NO_PROGRESS_TIMEOUT.as_secs(),
            total_timeout.as_secs(),
        );
        let observation = make_application()?
            .run_live_demo_scene(
                scene_kind,
                |checkpoint| {
                    let json = serde_json::to_string(checkpoint)
                        .expect("LiveDemoCheckpoint has a stable serializable schema");
                    println!("CREST_LIVE_CHECKPOINT {json}");
                },
                emit_live_report,
            )
            .context("live observable demo execution failed")?;
        if options.demo_live_detail_and_assets {
            let detail = observation
                .detail_and_assets()
                .context("the detail-and-assets scene did not retain its typed evidence")?;
            let detail = serde_json::to_string(detail)
                .context("failed to serialize detail-and-assets teardown evidence")?;
            println!("CREST_DETAIL_AND_ASSETS_LIVE_OBSERVATION {detail}");
        } else if options.demo_live_mixer {
            let mixer = observation
                .live_mixer()
                .context("the dedicated Mixer scene did not retain its typed evidence")?;
            if !mixer.is_complete() {
                bail!("dedicated Mixer live predicates were incomplete");
            }
            let json = serde_json::to_string(&mixer)
                .context("failed to serialize dedicated Mixer teardown evidence")?;
            println!("CREST_LIVE_MIXER_OBSERVATION {json}");
        } else if options.demo_live_patch_editor {
            let editor = observation
                .functional_patch_editor()
                .context("the functional Patch editor scene did not retain its measurement")?;
            let shortfalls = editor.shortfalls();
            let json = serde_json::to_string(editor)
                .context("failed to serialize the functional Patch editor observation")?;
            println!(
                "{}{json}",
                crest_synth::testing::FUNCTIONAL_PATCH_EDITOR_OBSERVATION_MARKER
            );
            if !shortfalls.is_empty() {
                // The failing predicate set is reported by name, so a negative
                // that failed for an unrelated reason is visible as such
                // rather than being credited as falsification.
                bail!(
                    "functional Patch editor predicates unmet: {}",
                    shortfalls.join(", ")
                );
            }
        } else if options.demo_live_effects_and_buses {
            let effects = observation
                .effects_and_buses()
                .context("the effects-and-buses scene did not retain its evidence")?;
            let effects = serde_json::to_string(&effects)
                .context("failed to serialize effects-and-buses teardown evidence")?;
            println!("CREST_EFFECTS_AND_BUSES_LIVE_OBSERVATION {effects}");
        } else if options.demo_live_sixteen_track {
            let routing = observation.sixteen_track_mixer_routing();
            let routing = serde_json::to_string(&routing)
                .context("failed to serialize sixteen-track routing teardown evidence")?;
            println!("CREST_SIXTEEN_TRACK_MIXER_ROUTING_LIVE_OBSERVATION {routing}");
        } else if options.demo_live_semantic {
            let observation = serde_json::to_string(&observation)
                .context("failed to serialize graphical-shell teardown evidence")?;
            println!("CREST_SEMANTIC_VIEW_MODEL_LIVE_OBSERVATION {observation}");
        } else {
            let observation = serde_json::to_string(&observation)
                .context("failed to serialize graphical-shell teardown evidence")?;
            println!("CREST_GRAPHICAL_SHELL_LIVE_OBSERVATION {observation}");
        }
    } else if options.smoke {
        if options.demo_scene {
            let report = make_application()?
                .run_demo_scene(options.degenerate)
                .context("exhaustive demo-scene execution failed")?;
            let second = make_application()?
                .run_demo_scene(options.degenerate)
                .context("second exhaustive demo-scene execution failed")?;
            let two_run_trace_equal = report
                .to_json()
                .context("failed to serialize the first demo trace")?
                == second
                    .to_json()
                    .context("failed to serialize the second demo trace")?;
            emit_demo_scene(&report, two_run_trace_equal)?;
            if !report.is_complete() {
                bail!(
                    "exhaustive demo scene is incomplete: {} grouped and {} event-log coverage identifiers missing, {} records dropped",
                    report.coverage().missing_count(),
                    report.event_log().coverage().missing().len(),
                    report.event_log().dropped_records(),
                );
            }
        } else {
            let observation = make_application()?
                .run_smoke(options.degenerate)
                .context("headless smoke execution failed")?;
            if options.observe {
                let json = serde_json::to_string(&observation)
                    .context("failed to serialize the smoke observation")?;
                println!("CREST_OBSERVATION {json}");
            }
        }
    } else {
        make_application()?
            .run()
            .context("standalone execution failed")?;
    }

    Ok(())
}

fn emit_demo_scene(report: &DemoSceneReport, two_run_trace_equal: bool) -> Result<()> {
    let event_log = report
        .event_log()
        .to_json()
        .context("failed to serialize the exhaustive event log")?;
    let state_tree = report.final_state_tree().json();
    let observation = DemoSceneObservation::from_report(report, two_run_trace_equal);
    let observation = serde_json::to_string(&observation)
        .context("failed to serialize the demo-scene observation")?;

    println!("CREST_EVENT_LOG {event_log}");
    println!("CREST_STATE_TREE {state_tree}");
    println!("CREST_OBSERVATION {observation}");
    Ok(())
}

fn emit_live_report(report: &LiveDemoReport) {
    let event_log_summary = serde_json::to_string(&report.event_log_summary())
        .expect("completed live EventLog summary has a stable serializable schema");
    let coverage = serde_json::to_string(report.coverage())
        .expect("completed live coverage has a stable serializable schema");
    println!("CREST_LIVE_EVENT_LOG_SUMMARY {event_log_summary}");
    println!("CREST_LIVE_STATE_TREE {}", report.state_tree().json());
    println!("CREST_LIVE_COVERAGE {coverage}");
    println!("CREST_LIVE_SUMMARY {}", report.summary());
}

/// Self-contained production-adapter library for Sample and Drum Rack scenes.
///
/// The files are created before application composition and consumed through
/// `FilesystemSampleCatalog` plus `WavSampleDecoder`; no fixture decoder or
/// callback-time I/O participates. Dropping the run restores any caller
/// configuration and removes only this process-unique temporary directory.
struct DemoSampleLibrary {
    root: std::path::PathBuf,
    previous_root: Option<std::ffi::OsString>,
    previous_asset: Option<std::ffi::OsString>,
}

impl DemoSampleLibrary {
    fn create(drum_rack: bool) -> Result<Self> {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .context("system clock is before the Unix epoch")?
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("crest-synth-demo-{}-{unique}", std::process::id()));
        std::fs::create_dir(&root).context("failed to create the demo sample library")?;
        let write_result = (|| -> Result<()> {
            if drum_rack {
                crest_synth::testing::drum_rack_demo::write_samples(&root)?;
                return Ok(());
            }
            std::fs::write(root.join("A-valid.wav"), demo_wave(196.0))
                .context("failed to write the active Phase 7 WAV fixture")?;
            std::fs::write(root.join("B-alternate.wav"), demo_wave(329.63))
                .context("failed to write the alternate Phase 7 WAV fixture")?;
            std::fs::write(root.join("Z-invalid.wav"), b"not a RIFF/WAVE asset")
                .context("failed to write the invalid Phase 7 WAV fixture")?;
            Ok(())
        })();
        if let Err(error) = write_result {
            let _ = std::fs::remove_dir_all(&root);
            return Err(error);
        }
        let previous_root =
            std::env::var_os(crest_synth::adapter::production_instruments::SAMPLE_LIBRARY_ROOT_ENV);
        let previous_asset = std::env::var_os(
            crest_synth::adapter::production_instruments::SAMPLE_DEFAULT_ASSET_ENV,
        );
        std::env::set_var(
            crest_synth::adapter::production_instruments::SAMPLE_LIBRARY_ROOT_ENV,
            &root,
        );
        std::env::set_var(
            crest_synth::adapter::production_instruments::SAMPLE_DEFAULT_ASSET_ENV,
            if drum_rack {
                crest_synth::testing::drum_rack_demo::sample_filename(0)
            } else {
                "A-valid.wav".into()
            },
        );
        Ok(Self {
            root,
            previous_root,
            previous_asset,
        })
    }
}

impl Drop for DemoSampleLibrary {
    fn drop(&mut self) {
        let restore = |name: &str, value: &Option<std::ffi::OsString>| match value {
            Some(value) => std::env::set_var(name, value),
            None => std::env::remove_var(name),
        };
        restore(
            crest_synth::adapter::production_instruments::SAMPLE_LIBRARY_ROOT_ENV,
            &self.previous_root,
        );
        restore(
            crest_synth::adapter::production_instruments::SAMPLE_DEFAULT_ASSET_ENV,
            &self.previous_asset,
        );
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn demo_wave(frequency_hz: f32) -> Vec<u8> {
    const SAMPLE_RATE: u32 = 48_000;
    const FRAMES: u32 = 24_000;
    const CHANNELS: u16 = 1;
    const BITS: u16 = 16;
    let data_bytes = FRAMES * u32::from(CHANNELS) * u32::from(BITS / 8);
    let mut bytes = Vec::with_capacity(44 + data_bytes as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36_u32 + data_bytes).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&CHANNELS.to_le_bytes());
    bytes.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    bytes.extend_from_slice(&(SAMPLE_RATE * u32::from(CHANNELS) * 2).to_le_bytes());
    bytes.extend_from_slice(&(CHANNELS * 2).to_le_bytes());
    bytes.extend_from_slice(&BITS.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_bytes.to_le_bytes());
    for frame in 0..FRAMES {
        let phase = core::f32::consts::TAU * frequency_hz * frame as f32 / SAMPLE_RATE as f32;
        let sample = (phase.sin() * 0.35 * i16::MAX as f32).round() as i16;
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Options {
    smoke: bool,
    observe: bool,
    demo_scene: bool,
    full_instrument_effect_demo: bool,
    demo_live_drum_rack: bool,
    demo_live: bool,
    demo_live_mixer: bool,
    demo_live_semantic: bool,
    demo_live_sixteen_track: bool,
    demo_live_effects_and_buses: bool,
    demo_live_patch_editor: bool,
    demo_live_detail_and_assets: bool,
    /// Controlled Phase 7 negative: omit the required semantic preview hold.
    defeat_detail_and_assets_preview: bool,
    /// The declared controlled negative for the functional Patch editor
    /// scene. Accepted only alongside `--demo-live-patch-editor`.
    defeat_patch_selection: bool,
    /// The browsable component gallery. Deliberately not a `demo_live` scene:
    /// it accepts input, correlates with no generation, and is not part of the
    /// `--demo-live` alias group.
    demo_component_library: bool,
    degenerate: Option<DegenerateMode>,
}

fn parse_options<I, Argument>(arguments: I) -> Result<Options>
where
    I: IntoIterator<Item = Argument>,
    Argument: AsRef<str>,
{
    let mut options = Options::default();

    let arguments = arguments.into_iter();
    for argument in arguments {
        match argument.as_ref() {
            "--demo-live-drum-rack" if !options.demo_live_drum_rack => {
                options.demo_live_drum_rack = true;
            }
            "--full-instrument-effect-demo" if !options.full_instrument_effect_demo => {
                options.full_instrument_effect_demo = true;
            }
            "--smoke" if !options.smoke => options.smoke = true,
            "--observe" if !options.observe => options.observe = true,
            "--demo-scene" if !options.demo_scene => options.demo_scene = true,
            // `--demo-live` points at the newest cumulative retained scene.
            "--demo-live" | "--demo-live-detail-and-assets" if !options.demo_live => {
                options.demo_live = true;
                options.demo_live_detail_and_assets = true;
            }
            "--demo-live-effects-and-buses" if !options.demo_live => {
                options.demo_live = true;
                options.demo_live_effects_and_buses = true;
            }
            // Additive, and deliberately absent from the `--demo-live` alias
            // group above: `--demo-live` keeps pointing at the newest
            // cumulative retained scene.
            "--demo-live-patch-editor" if !options.demo_live => {
                options.demo_live = true;
                options.demo_live_patch_editor = true;
            }
            "--demo-live-mixer" if !options.demo_live => {
                options.demo_live = true;
                options.demo_live_mixer = true;
            }
            "--defeat-patch-selection" if !options.defeat_patch_selection => {
                options.defeat_patch_selection = true;
            }
            "--defeat-detail-and-assets-preview" if !options.defeat_detail_and_assets_preview => {
                options.defeat_detail_and_assets_preview = true;
            }
            "--demo-live-sixteen-track-mixer-routing" if !options.demo_live => {
                options.demo_live = true;
                options.demo_live_sixteen_track = true;
            }
            "--demo-live-semantic-view-model" if !options.demo_live => {
                options.demo_live = true;
                options.demo_live_semantic = true;
            }
            "--demo-live-graphical-shell" if !options.demo_live => {
                options.demo_live = true;
            }
            // Deliberately absent from the `--demo-live` alias group above: the
            // gallery is browsable rather than autonomous, and `--demo-live`
            // must keep pointing at the newest cumulative autonomous scene.
            "--demo-live-component-library" if !options.demo_component_library => {
                options.demo_component_library = true;
            }
            "--degenerate-audio" if options.degenerate.is_none() => {
                options.degenerate = Some(DegenerateMode::Audio);
            }
            "--degenerate-control" if options.degenerate.is_none() => {
                options.degenerate = Some(DegenerateMode::Control);
            }
            "--full-instrument-effect-demo"
            | "--demo-live-drum-rack"
            | "--smoke"
            | "--observe"
            | "--demo-scene"
            | "--demo-live"
            | "--demo-live-detail-and-assets"
            | "--demo-live-effects-and-buses"
            | "--demo-live-mixer"
            | "--demo-live-sixteen-track-mixer-routing"
            | "--demo-live-patch-editor"
            | "--defeat-patch-selection"
            | "--defeat-detail-and-assets-preview"
            | "--demo-live-semantic-view-model"
            | "--demo-live-graphical-shell"
            | "--demo-live-component-library" => {
                bail!("duplicate option {}", argument.as_ref());
            }
            "--degenerate-audio" | "--degenerate-control" => {
                bail!("only one degenerate smoke mode may be selected");
            }
            unsupported => bail!("unsupported option {unsupported}"),
        }
    }

    if (options.full_instrument_effect_demo || options.demo_live_drum_rack)
        && (options.smoke
            || options.observe
            || options.demo_scene
            || options.demo_live
            || options.demo_component_library
            || options.degenerate.is_some()
            || options.defeat_patch_selection
            || options.defeat_detail_and_assets_preview
            || (options.full_instrument_effect_demo && options.demo_live_drum_rack))
    {
        bail!("listening demo options must be used by themselves");
    }
    if options.observe && !options.smoke {
        bail!("--observe requires --smoke");
    }
    if options.demo_scene && !(options.smoke && options.observe) {
        bail!("--demo-scene requires both --smoke and --observe");
    }
    if options.degenerate.is_some() && !(options.smoke && options.observe) {
        bail!("degenerate modes require both --smoke and --observe");
    }
    if options.demo_live
        && (options.smoke || options.observe || options.demo_scene || options.degenerate.is_some())
    {
        bail!("the live demo option must be used by itself");
    }
    if options.defeat_patch_selection && !options.demo_live_patch_editor {
        bail!("--defeat-patch-selection requires --demo-live-patch-editor");
    }
    if options.defeat_detail_and_assets_preview && !options.demo_live_detail_and_assets {
        bail!("--defeat-detail-and-assets-preview requires --demo-live-detail-and-assets");
    }
    if options.demo_component_library
        && (options.smoke
            || options.observe
            || options.demo_scene
            || options.demo_live
            || options.degenerate.is_some())
    {
        bail!("the component gallery option must be used by itself");
    }

    Ok(options)
}

#[derive(Debug, Serialize)]
struct DemoSceneObservation {
    accepted_events: usize,
    active_graph_revision: u64,
    adjust_directions_exercised: usize,
    alternating_capabilities: bool,
    all_parameter_boundaries_exercised: bool,
    all_audio_parameter_effects_observed: bool,
    all_patch_parameter_cases_exercised: bool,
    all_serialized_properties_observed: bool,
    audio_command_variants_exercised: usize,
    app_event_variants_exercised: usize,
    top_level_contexts_exercised: usize,
    baseline_restored: bool,
    braids_patches: usize,
    braids_scalar_cases_exercised: usize,
    causal_audio_comparisons: bool,
    callback_destructions: usize,
    coverage_missing: usize,
    demo_scene_complete: bool,
    delay_input_nonzero: bool,
    descriptors_unique: bool,
    event_record_payloads_exact: bool,
    event_sources_exercised: usize,
    event_log_dropped: u64,
    event_log_records: usize,
    exact_projection_values: bool,
    exact_state_values: bool,
    faithful_effect_path: bool,
    final_state_tree_matches: bool,
    generation_chain_valid: bool,
    global_parameter_cases_exercised: usize,
    gui_projection_matches_state: bool,
    midi_message_kinds_exercised: usize,
    mixed_engine_parameter_isolation: bool,
    mixed_engine_stems_nonzero: bool,
    navigate_directions_exercised: usize,
    parameter_projection_matches_state: bool,
    parsed_soundfont_banks: usize,
    patch_adsr_control_cases_exercised: usize,
    patch_focus_projection_exact: bool,
    patch_adsr_scalar_only: bool,
    patch_adsr_structural_coexistence_exact: bool,
    post_rejection_event_accepted: bool,
    prepared_instruments: usize,
    rejection_variants_exercised: usize,
    rejected_events: usize,
    reverb_input_nonzero: bool,
    scene_checkpoints: usize,
    schema_surface_equal: bool,
    selection_clamps_exact: bool,
    envelope_parameter_cases_exercised: usize,
    state_hash_chain_valid: bool,
    state_tree_patch_count: usize,
    state_tree_schema_version: u32,
    tick_events_exact: bool,
    two_run_trace_equal: bool,
    unexpected_coverage: usize,
    window_input_cases_exercised: usize,
}

impl DemoSceneObservation {
    fn from_report(report: &DemoSceneReport, two_run_trace_equal: bool) -> Self {
        let records = report.event_log().records();
        let mut event_variants = [false; 22];
        let mut top_level_contexts = Vec::new();
        let mut navigate_directions = Vec::new();
        let mut adjust_directions = Vec::new();
        let mut midi_kinds = Vec::new();
        let mut event_sources = Vec::new();

        for record in records {
            push_unique(&mut event_sources, record.source());
            match record.input() {
                EventInput::SelectContext { context } => {
                    event_variants[0] = true;
                    push_unique(&mut top_level_contexts, *context);
                }
                EventInput::InstallPatches { .. } => event_variants[1] = true,
                EventInput::SelectPatch { direction } => {
                    event_variants[15] = true;
                    push_unique(&mut navigate_directions, *direction);
                }
                EventInput::NavigatePage { .. } => event_variants[21] = true,
                EventInput::Navigate { direction } => {
                    event_variants[2] = true;
                    push_unique(&mut navigate_directions, *direction);
                }
                EventInput::Adjust { direction } => {
                    event_variants[3] = true;
                    push_unique(&mut adjust_directions, *direction);
                }
                EventInput::SetInteractionMode { .. } => event_variants[4] = true,
                EventInput::OpenRelated => event_variants[16] = true,
                EventInput::OpenMidiSettings => event_variants[20] = true,
                EventInput::Activate => event_variants[17] = true,
                EventInput::PreviewStart => event_variants[18] = true,
                EventInput::PreviewStop => event_variants[19] = true,
                EventInput::EnterSurface { .. } => event_variants[5] = true,
                EventInput::Return => event_variants[6] = true,
                EventInput::Midi { message, .. } => {
                    event_variants[7] = true;
                    push_unique(&mut midi_kinds, message.kind());
                }
                EventInput::SetPatchOverviewOriginEnabled { .. } => {}
                EventInput::ReplacePersistedSession { .. } => {}
                EventInput::EngineSelectionLifecycleAdvanced { .. } => {}
                EventInput::EnginePrepared { .. } => event_variants[8] = true,
                EventInput::SampleAssetLifecycleAdvanced { .. } => {}
                EventInput::FileCatalogRefreshed { .. } => {}
                EventInput::EnginePreparationFailed { .. } => event_variants[9] = true,
                EventInput::EngineActivationAcknowledged { .. } => event_variants[10] = true,
                EventInput::SetSlotOccupancy { .. } => event_variants[11] = true,
                EventInput::SetReturnOccupancy { .. } => event_variants[12] = true,
                EventInput::TopologyPrepared { .. } => event_variants[13] = true,
                EventInput::TopologyPreparationFailed { .. } => event_variants[14] = true,
                EventInput::MidiInputPreferenceRestored { .. }
                | EventInput::MidiInputPreferenceStoreFailed { .. }
                | EventInput::MidiInputScanStarted
                | EventInput::MidiInputScanSucceeded { .. }
                | EventInput::MidiInputScanFailed { .. }
                | EventInput::MidiInputConnectRequested { .. }
                | EventInput::MidiInputConnectionPrepared { .. }
                | EventInput::MidiInputActivationAcknowledged { .. }
                | EventInput::MidiInputDisconnectRequested { .. }
                | EventInput::MidiInputConnectionLost { .. }
                | EventInput::MidiInputOperationFailed { .. }
                | EventInput::Controller { .. }
                | EventInput::MidiInputShutdownRequested
                | EventInput::AssetImported { .. }
                | EventInput::Send { .. }
                | EventInput::ToggleTestMidi => {}
            }
        }

        let accepted_events = records
            .iter()
            .filter(|record| record.outcome() == EventOutcome::Accepted)
            .count();
        let rejected_events = records
            .iter()
            .filter(|record| record.outcome() == EventOutcome::Rejected)
            .count();
        let post_rejection_event_accepted =
            records
                .iter()
                .zip(records.iter().skip(1))
                .any(|(previous, next)| {
                    previous.outcome() == EventOutcome::Rejected
                        && next.outcome() == EventOutcome::Accepted
                });
        let generation_chain_valid = generation_chain_valid(records);
        let state_hash_chain_valid = state_hash_chain_valid(records);
        let final_state_tree_matches = records.back().is_some_and(|record| {
            let tree = report.final_state_tree();
            record.generation_after() == tree.generation()
                && record.parameter_generation() == tree.generation()
                && record.state_hash_after() == tree.state_hash()
                && record.projection_state_hash() == tree.state_hash()
                && record.selected_line() == tree.selected_line()
        });

        let tree_json = serde_json::from_str::<Value>(report.final_state_tree().json()).ok();
        let gui_projection_matches_state = tree_json.as_ref().is_some_and(|tree| {
            tree.get("projection")
                .and_then(|projection| projection.get("stateHash"))
                .and_then(Value::as_str)
                == Some(report.final_state_tree().state_hash())
                && tree
                    .get("projection")
                    .and_then(|projection| projection.get("selectedLine"))
                    .and_then(Value::as_u64)
                    == u64::try_from(report.final_state_tree().selected_line()).ok()
        });
        let active_graph_revision = report.final_state_tree().graph_revision().value();
        let parameter_projection_matches_state = tree_json.as_ref().is_some_and(|tree| {
            parameter_projection_matches_state(tree)
                && tree
                    .pointer("/parameters/graphRevision")
                    .and_then(Value::as_u64)
                    == Some(active_graph_revision)
        });

        let editable = report
            .coverage()
            .group(DemoCoverageGroup::EditableParameters);
        let global_parameter_cases_exercised = editable
            .exercised()
            .iter()
            .filter(|identifier| identifier.starts_with("parameter.global."))
            .count();
        let envelope_parameter_cases_exercised = editable
            .exercised()
            .iter()
            .filter(|identifier| {
                [
                    "attackMilliseconds",
                    "decayMilliseconds",
                    "sustain",
                    "releaseMilliseconds",
                ]
                .iter()
                .any(|parameter| identifier.ends_with(&format!(".{parameter}")))
            })
            .count();
        let routed_patch_adsr = report
            .checkpoints()
            .iter()
            .filter_map(|checkpoint| checkpoint.patch_adsr())
            .filter(|checkpoint| checkpoint.lifecycle().is_none())
            .collect::<Vec<_>>();
        let patch_adsr_control_cases_exercised = routed_patch_adsr.len();
        let patch_focus_projection_exact = patch_adsr_control_cases_exercised
            == crest_synth::synth::VoiceEnvelope::surface_descriptor().len()
            && routed_patch_adsr
                .iter()
                .all(|checkpoint| checkpoint.focus_projection_exact());
        let patch_adsr_checkpoints = report
            .checkpoints()
            .iter()
            .filter_map(|checkpoint| checkpoint.patch_adsr())
            .collect::<Vec<_>>();
        let patch_adsr_scalar_only = !patch_adsr_checkpoints.is_empty()
            && patch_adsr_checkpoints
                .iter()
                .all(|checkpoint| checkpoint.scalar_only());
        let preparing_adsr = patch_adsr_checkpoints.iter().find(|checkpoint| {
            checkpoint.lifecycle()
                == Some(crest_synth::control::EngineSelectionStatusKind::Preparing)
        });
        let activating_adsr = patch_adsr_checkpoints.iter().find(|checkpoint| {
            checkpoint.lifecycle()
                == Some(crest_synth::control::EngineSelectionStatusKind::Activating)
        });
        let patch_adsr_structural_coexistence_exact = match (preparing_adsr, activating_adsr) {
            (Some(preparing), Some(activating)) => {
                [preparing, activating].into_iter().all(|checkpoint| {
                    checkpoint.scalar_only()
                        && checkpoint.focus_projection_exact()
                        && checkpoint.all_envelope_values_exact()
                        && checkpoint.untargeted_patches_exact()
                        && checkpoint.graph_revision_exact()
                }) && activating.renderer_graph_revision() > preparing.renderer_graph_revision()
            }
            _ => false,
        };
        let patch_capabilities: Vec<(u64, String)> = tree_json
            .as_ref()
            .and_then(|tree| tree.get("patches"))
            .and_then(Value::as_array)
            .map(|patches| {
                patches
                    .iter()
                    .filter_map(|patch| {
                        Some((
                            patch.get("id")?.as_u64()?,
                            patch
                                .pointer("/instrument/capabilityId")?
                                .as_str()?
                                .to_owned(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let soundfont_patches = patch_capabilities
            .iter()
            .filter(|(_, capability)| capability == HIDEF_CAPABILITY_ID)
            .count();
        let braids_patch_ids: Vec<u64> = patch_capabilities
            .iter()
            .filter_map(|(patch_id, capability)| {
                (capability == BRAIDS_CAPABILITY_ID).then_some(*patch_id)
            })
            .collect();
        let braids_patches = braids_patch_ids.len();
        let alternating_capabilities = patch_capabilities.len() > 1
            && soundfont_patches > 0
            && braids_patches > 0
            && patch_capabilities
                .windows(2)
                .all(|pair| pair[0].1 != pair[1].1);
        let braids_scalar_cases_exercised = editable
            .exercised()
            .iter()
            .filter(|identifier| {
                braids_patch_ids.iter().any(|patch_id| {
                    identifier.starts_with(&format!("parameter.patch.{patch_id}.braids."))
                })
            })
            .count();
        let all_patch_parameter_cases_exercised =
            prefixed_expected_are_exercised(editable, "parameter.patch.");
        let serialized = report
            .coverage()
            .group(DemoCoverageGroup::SerializedProperties);
        let projections = report.coverage().group(DemoCoverageGroup::Projections);
        let all_serialized_properties_observed =
            serialized.is_complete() && projections.is_complete();
        let audio_effects = report.coverage().group(DemoCoverageGroup::AudioEffects);
        let all_audio_parameter_effects_observed =
            prefixed_expected_are_exercised(audio_effects, "effect.parameterSnapshot.");
        let audio_command_variants_exercised = audio_effects
            .exercised()
            .iter()
            .filter(|identifier| identifier.starts_with("effect.emitted.audioCommand."))
            .count();
        let inputs = report.coverage().group(DemoCoverageGroup::Inputs);
        let rejections = report.coverage().group(DemoCoverageGroup::Rejections);
        let coverage_missing =
            report.coverage().missing_count() + report.event_log().coverage().missing().len();
        let unexpected_coverage =
            report.coverage().unexpected_count() + report.event_log().coverage().unexpected().len();
        let schema_surface_equal = coverage_missing == 0 && unexpected_coverage == 0;
        let boundary_rejections = records
            .iter()
            .filter(|record| record.rejection() == Some(EventRejection::ParameterAtBoundary))
            .count();
        let all_parameter_boundaries_exercised = boundary_rejections
            >= 2 * (PatchOutput::surface_descriptor().len()
                + MixerTrackId::COUNT * MixerTrackParameters::surface_descriptor().len()
                + GlobalParameters::surface_descriptor().len());

        let harness = BehavioralMutationHarness::new();
        let dry_observation = harness
            .run(BehavioralMutationCase::DryToWetBypass, false)
            .into_observation();
        let BehavioralMutationObservation::DryToWetBypass(dry_observation) = dry_observation else {
            unreachable!("the selected dry-to-wet case retains its typed schema")
        };
        let cross_observation = harness
            .run(BehavioralMutationCase::CrossTrackParameterLeak, false)
            .into_observation();
        let BehavioralMutationObservation::CrossTrackParameterLeak(cross_observation) =
            cross_observation
        else {
            unreachable!("the selected cross-track case retains its typed schema")
        };
        let reverb_input_nonzero = dry_observation.nonzero_send_reverb_input_energy > 0.0;
        let delay_input_nonzero = dry_observation.nonzero_send_delay_input_energy > 0.0;
        let faithful_effect_path = reverb_input_nonzero
            && delay_input_nonzero
            && dry_observation.identical_effect_state
            && dry_observation.dry_bypass_absent
            && dry_observation.finite_audio
            && dry_observation.baseline_restored;
        let causal_audio_comparisons = cross_observation.edited_track_audio_changed
            && cross_observation.unedited_track_audio_unchanged
            && cross_observation.all_track_parameters_isolated;
        let baseline_restored = tree_json
            .as_ref()
            .is_some_and(fixed_fixture_baseline_restored)
            && dry_observation.baseline_restored
            && cross_observation.baseline_restored;
        let selection_clamps_exact = tree_json.as_ref().is_some_and(|tree| {
            tree.pointer("/interaction/activeFocus/context")
                .and_then(Value::as_str)
                == Some("mixer")
                && tree
                    .pointer("/interaction/activeFocus/patchId")
                    .and_then(Value::as_u64)
                    == tree.pointer("/patches/0/id").and_then(Value::as_u64)
                && tree
                    .pointer("/interaction/activeFocus/controlId/id/kind")
                    .and_then(Value::as_str)
                    == Some("track")
                && tree
                    .pointer("/interaction/activeFocus/controlId/id/trackId")
                    .and_then(Value::as_u64)
                    == Some(0)
                && tree
                    .pointer("/interaction/activeFocus/controlId/id/parameter")
                    .and_then(Value::as_str)
                    == Some("level")
        });
        let exact_state_values = parameter_projection_matches_state
            && tree_json.as_ref().is_some_and(final_tree_values_are_exact);
        let exact_projection_values =
            gui_projection_matches_state && parameter_projection_matches_state;
        let event_record_payloads_exact = event_records_are_exact(records);
        let expected_midi = report.coverage().group(DemoCoverageGroup::MidiKinds);
        let tick_events_exact = event_sources.contains(&EventSource::AutomaticMidi)
            && expected_midi.is_complete()
            && midi_kinds.len() == expected_midi.expected().len();

        Self {
            accepted_events,
            active_graph_revision,
            adjust_directions_exercised: adjust_directions.len(),
            alternating_capabilities,
            all_parameter_boundaries_exercised,
            all_audio_parameter_effects_observed,
            all_patch_parameter_cases_exercised,
            all_serialized_properties_observed,
            audio_command_variants_exercised,
            app_event_variants_exercised: event_variants
                .iter()
                .filter(|exercised| **exercised)
                .count(),
            top_level_contexts_exercised: top_level_contexts.len(),
            baseline_restored,
            braids_patches,
            braids_scalar_cases_exercised,
            causal_audio_comparisons,
            callback_destructions: 0,
            coverage_missing,
            demo_scene_complete: report.is_complete(),
            delay_input_nonzero,
            descriptors_unique: descriptors_are_unique(),
            event_record_payloads_exact,
            event_sources_exercised: event_sources.len(),
            envelope_parameter_cases_exercised,
            event_log_dropped: report.event_log().dropped_records(),
            event_log_records: records.len(),
            exact_projection_values,
            exact_state_values,
            faithful_effect_path,
            final_state_tree_matches,
            generation_chain_valid,
            global_parameter_cases_exercised,
            gui_projection_matches_state,
            midi_message_kinds_exercised: midi_kinds.len(),
            mixed_engine_parameter_isolation: report
                .audio_evidence()
                .mixed_engine_parameter_isolation(),
            mixed_engine_stems_nonzero: report.audio_evidence().mixed_engine_stems_nonzero(),
            navigate_directions_exercised: navigate_directions.len(),
            parameter_projection_matches_state,
            parsed_soundfont_banks: 1,
            patch_adsr_control_cases_exercised,
            patch_focus_projection_exact,
            patch_adsr_scalar_only,
            patch_adsr_structural_coexistence_exact,
            post_rejection_event_accepted,
            prepared_instruments: report.final_state_tree().patch_count(),
            rejection_variants_exercised: rejections.exercised().len(),
            rejected_events,
            reverb_input_nonzero,
            scene_checkpoints: report.checkpoints().len(),
            schema_surface_equal,
            selection_clamps_exact,
            state_hash_chain_valid,
            state_tree_patch_count: report.final_state_tree().patch_count(),
            state_tree_schema_version: report.final_state_tree().schema_version(),
            tick_events_exact,
            two_run_trace_equal,
            unexpected_coverage,
            window_input_cases_exercised: inputs
                .exercised()
                .iter()
                .filter(|identifier| identifier.starts_with("input."))
                .count(),
        }
    }
}

fn push_unique<ValueType>(values: &mut Vec<ValueType>, value: ValueType)
where
    ValueType: Copy + PartialEq,
{
    if !values.contains(&value) {
        values.push(value);
    }
}

fn descriptors_are_unique() -> bool {
    unique(WindowInput::surface_descriptor())
        && unique(AppEvent::surface_descriptor())
        && unique(MidiMessageKind::surface_descriptor())
        && unique(PatchOutput::surface_descriptor())
        && unique(MixerTrackParameters::surface_descriptor())
        && unique(GlobalParameters::surface_descriptor())
        && unique(EventRejection::surface_descriptor())
}

fn unique<ValueType>(values: &[ValueType]) -> bool
where
    ValueType: PartialEq,
{
    values
        .iter()
        .enumerate()
        .all(|(index, value)| !values[..index].contains(value))
}

fn fixed_fixture_baseline_restored(tree: &Value) -> bool {
    let patches_restored = tree
        .get("patches")
        .and_then(Value::as_array)
        .is_some_and(|patches| {
            !patches.is_empty()
                && patches.iter().all(|patch| {
                    patch.pointer("/output/trimGainDb").and_then(Value::as_f64) == Some(0.0)
                        && patch
                            .pointer("/output/trackId")
                            .and_then(Value::as_u64)
                            .is_some_and(|track_id| track_id < MixerTrackId::COUNT as u64)
                })
        });
    let mixer_restored = tree
        .pointer("/mixer/tracks")
        .and_then(Value::as_array)
        .is_some_and(|tracks| {
            tracks.len() == MixerTrackId::COUNT
                && tracks.iter().all(|track| {
                    track.pointer("/levelDb").and_then(Value::as_f64) == Some(0.0)
                        && track.pointer("/pan").and_then(Value::as_f64) == Some(0.0)
                        && track.pointer("/mute").and_then(Value::as_bool) == Some(false)
                        && track.pointer("/solo").and_then(Value::as_bool) == Some(false)
                        // Sends travel as one indexed per-track array in
                        // ascending BusId order; the baseline is all-zero.
                        && track
                            .pointer("/sends")
                            .and_then(Value::as_array)
                            .is_some_and(|sends| {
                                sends.len() == 8
                                    && sends
                                        .iter()
                                        .all(|send| send.as_f64() == Some(0.0))
                            })
                })
        });
    let defaults = ApplicationConfig::default().global_parameters();
    let master_restored = tree
        .pointer("/global/masterGainDb")
        .and_then(Value::as_f64)
        .is_some_and(|value| value as f32 == defaults.master_gain_db());
    // The six retired global leaves are return-owned now: the baseline is the
    // production default occupancy — reverb defaults on bus 0, delay defaults
    // on bus 1, both at the default 0.5 level.
    let returns_restored = tree
        .pointer("/returns")
        .and_then(Value::as_array)
        .is_some_and(|returns| {
            returns.len() == 8
                && [(0_usize, 0.5, 0.5), (1, 250.0, 0.5)]
                    .iter()
                    .all(|(bus, first, second)| {
                        returns[*bus]
                            .pointer("/returnLevel")
                            .and_then(Value::as_f64)
                            == Some(0.5)
                            && returns[*bus]
                                .pointer("/effect/values/0/value/value")
                                .and_then(Value::as_f64)
                                == Some(*first)
                            && returns[*bus]
                                .pointer("/effect/values/1/value/value")
                                .and_then(Value::as_f64)
                                == Some(*second)
                    })
                && returns[2..]
                    .iter()
                    .all(|entry| entry.pointer("/effect").is_some_and(Value::is_null))
        });

    patches_restored && mixer_restored && master_restored && returns_restored
}

fn final_tree_values_are_exact(tree: &Value) -> bool {
    let patches_exact = tree
        .get("patches")
        .and_then(Value::as_array)
        .is_some_and(|patches| {
            patches.len() > 1
                && patches.iter().all(|patch| {
                    patch
                        .get("id")
                        .and_then(Value::as_u64)
                        .is_some_and(|id| id > 0)
                        && patch
                            .get("name")
                            .and_then(Value::as_str)
                            .is_some_and(|name| !name.is_empty())
                        && patch
                            .get("channel")
                            .and_then(Value::as_u64)
                            .is_some_and(|channel| channel < 16)
                        && patch
                            .pointer("/output/trackId")
                            .and_then(Value::as_u64)
                            .is_some_and(|track_id| track_id < MixerTrackId::COUNT as u64)
                        && patch
                            .pointer("/output/trimGainDb")
                            .and_then(Value::as_f64)
                            .is_some_and(f64::is_finite)
                })
        });
    let mixer_exact = tree
        .pointer("/mixer/tracks")
        .and_then(Value::as_array)
        .is_some_and(|tracks| {
            tracks.len() == MixerTrackId::COUNT
                && tracks.iter().all(|track| {
                    ["levelDb", "pan"].iter().all(|field| {
                        track
                            .pointer(&format!("/{field}"))
                            .and_then(Value::as_f64)
                            .is_some_and(f64::is_finite)
                    }) && track
                        .pointer("/sends")
                        .and_then(Value::as_array)
                        .is_some_and(|sends| {
                            sends.len() == 8
                                && sends
                                    .iter()
                                    .all(|send| send.as_f64().is_some_and(f64::is_finite))
                        })
                        && track.pointer("/mute").and_then(Value::as_bool).is_some()
                        && track.pointer("/solo").and_then(Value::as_bool).is_some()
                })
        });
    let globals_exact = tree
        .pointer("/global/masterGainDb")
        .and_then(Value::as_f64)
        .is_some_and(f64::is_finite);
    let returns_exact = tree
        .pointer("/returns")
        .and_then(Value::as_array)
        .is_some_and(|returns| {
            returns.len() == 8
                && returns.iter().all(|entry| {
                    entry
                        .pointer("/returnLevel")
                        .and_then(Value::as_f64)
                        .is_some_and(f64::is_finite)
                })
        });

    patches_exact && mixer_exact && globals_exact && returns_exact
}

fn event_records_are_exact(
    records: &std::collections::VecDeque<crest_synth::control::event_record::EventRecord>,
) -> bool {
    let mut expected_graph_revision = GraphRevision::INITIAL;
    for (index, record) in records.iter().enumerate() {
        if record.outcome() == EventOutcome::Accepted {
            if let EventInput::EngineActivationAcknowledged {
                target_graph_revision,
                ..
            } = record.input()
            {
                expected_graph_revision = *target_graph_revision;
            }
        }
        let sequence_exact = u64::try_from(index) == Ok(record.sequence());
        let projection_exact = record.parameter_generation() == record.generation_after()
            && record.projection_state_hash() == record.state_hash_after();
        let outcome_exact = match record.outcome() {
            EventOutcome::Accepted => {
                let state_accepted = record
                    .emitted_events()
                    .iter()
                    .filter(|effect| {
                        matches!(
                            effect,
                            EmittedEvent::StateAccepted { generation }
                                if *generation == record.generation_after()
                        )
                    })
                    .count()
                    == 1;
                let parameter_publications = record
                    .emitted_events()
                    .iter()
                    .filter(|effect| {
                        matches!(
                            effect,
                            EmittedEvent::ParameterSnapshotPublished {
                                generation,
                                graph_revision,
                            } if *generation == record.generation_after()
                                && *graph_revision == expected_graph_revision
                        )
                    })
                    .count();
                let parameters_published = if record.input().publishes_parameters_on_acceptance() {
                    parameter_publications == 1
                } else {
                    parameter_publications == 0
                };
                record.rejection().is_none()
                    && state_accepted
                    && parameters_published
                    && record.generation_before().checked_add(1) == Some(record.generation_after())
                    && record.state_hash_before() != record.state_hash_after()
            }
            EventOutcome::Rejected => {
                record.rejection().is_some()
                    && record.emitted_events().is_empty()
                    && record.generation_before() == record.generation_after()
                    && record.state_hash_before() == record.state_hash_after()
            }
        };
        if !sequence_exact || !projection_exact || !outcome_exact {
            return false;
        }
    }
    true
}

fn prefixed_expected_are_exercised(
    coverage: &crest_synth::testing::demo_scene_report::DemoCoverageSet,
    prefix: &str,
) -> bool {
    let mut found = false;
    for identifier in coverage
        .expected()
        .iter()
        .filter(|identifier| identifier.starts_with(prefix))
    {
        found = true;
        if !coverage.exercised().contains(identifier) {
            return false;
        }
    }
    found
}

fn generation_chain_valid(
    records: &std::collections::VecDeque<crest_synth::control::event_record::EventRecord>,
) -> bool {
    let individual = records.iter().all(|record| match record.outcome() {
        EventOutcome::Accepted => {
            record.generation_before().checked_add(1) == Some(record.generation_after())
        }
        EventOutcome::Rejected => record.generation_before() == record.generation_after(),
    });
    individual
        && records
            .iter()
            .zip(records.iter().skip(1))
            .all(|(previous, next)| previous.generation_after() == next.generation_before())
}

fn state_hash_chain_valid(
    records: &std::collections::VecDeque<crest_synth::control::event_record::EventRecord>,
) -> bool {
    let individual = records.iter().all(|record| match record.outcome() {
        EventOutcome::Accepted => record.state_hash_before() != record.state_hash_after(),
        EventOutcome::Rejected => record.state_hash_before() == record.state_hash_after(),
    });
    individual
        && records
            .iter()
            .zip(records.iter().skip(1))
            .all(|(previous, next)| previous.state_hash_after() == next.state_hash_before())
}

fn parameter_projection_matches_state(tree: &Value) -> bool {
    let Some(generation) = tree.get("generation").and_then(Value::as_u64) else {
        return false;
    };
    let Some(patches) = tree.get("patches").and_then(Value::as_array) else {
        return false;
    };
    let Some(global) = tree.get("global") else {
        return false;
    };
    let Some(parameters) = tree.get("parameters") else {
        return false;
    };
    // The snapshot mirror's global object and the tree's top-level global
    // both carry master gain alone — return-owned values travel as the
    // indexed return entries, never as global leaves — so the one shared
    // global leaf is compared exactly.
    if parameters.get("generation").and_then(Value::as_u64) != Some(generation)
        || parameters.get("patchCount").and_then(Value::as_u64) != u64::try_from(patches.len()).ok()
        || global.get("masterGainDb").is_none()
        || parameters.pointer("/global/masterGainDb") != global.get("masterGainDb")
    {
        return false;
    }

    let Some(parameter_patches) = parameters.get("patches").and_then(Value::as_array) else {
        return false;
    };
    patches.len() == parameter_patches.len()
        && patches
            .iter()
            .zip(parameter_patches)
            .all(|(patch, parameters)| {
                patch.get("id") == parameters.get("patchId")
                    && patch.get("parameters") == parameters.get("parameters")
            })
}

#[cfg(test)]
mod tests {
    use super::{parse_options, DegenerateMode, DemoSampleLibrary, Options};
    use crest_synth::control::event_record::{EventDirection, MidiKind};

    #[test]
    fn full_instrument_effect_demo_option_is_isolated_and_unique() {
        assert_eq!(
            parse_options(["--full-instrument-effect-demo"]).unwrap(),
            Options {
                full_instrument_effect_demo: true,
                ..Options::default()
            }
        );
        for other in [
            "--full-instrument-effect-demo",
            "--smoke",
            "--demo-live",
            "--demo-live-component-library",
            "--defeat-patch-selection",
        ] {
            assert!(parse_options(["--full-instrument-effect-demo", other]).is_err());
        }
    }

    #[test]
    fn full_demo_selection_skips_known_audio_and_keeps_the_changed_sampler() {
        let selection = super::full_demo_selection().unwrap();
        assert_eq!(
            selection.effect_reference.as_str(),
            super::BRAIDS_CAPABILITY_ID
        );
        assert_eq!(
            selection
                .skipped_instruments
                .iter()
                .map(|id| id.as_str())
                .collect::<Vec<_>>(),
            [
                super::HIDEF_CAPABILITY_ID,
                super::BRAIDS_CAPABILITY_ID,
                crest_synth::adapter::drum_rack_capability::DRUM_RACK_CAPABILITY_ID
            ]
        );
        assert_eq!(
            selection
                .skipped_effects
                .iter()
                .map(|id| id.as_str())
                .collect::<Vec<_>>(),
            ["effect.chorus", "effect.reverb", "effect.delay"]
        );
        assert!(
            !selection
                .skipped_instruments
                .iter()
                .any(|id| id.as_str()
                    == crest_synth::adapter::sample_capability::SAMPLE_CAPABILITY_ID)
        );
    }

    #[test]
    fn accepts_normal_smoke_observation_demo_scene_and_each_negative_mode() {
        assert_eq!(parse_options([] as [&str; 0]).unwrap(), Options::default());
        assert_eq!(
            parse_options(["--smoke"]).unwrap(),
            Options {
                smoke: true,
                observe: false,
                demo_scene: false,
                full_instrument_effect_demo: false,
                demo_live_drum_rack: false,
                demo_live: false,
                demo_live_mixer: false,
                demo_live_semantic: false,
                demo_live_sixteen_track: false,
                demo_live_effects_and_buses: false,
                demo_live_patch_editor: false,
                demo_live_detail_and_assets: false,
                defeat_detail_and_assets_preview: false,
                defeat_patch_selection: false,
                demo_component_library: false,
                degenerate: None,
            }
        );
        assert_eq!(
            parse_options(["--observe", "--smoke"]).unwrap(),
            Options {
                smoke: true,
                observe: true,
                demo_scene: false,
                full_instrument_effect_demo: false,
                demo_live_drum_rack: false,
                demo_live: false,
                demo_live_mixer: false,
                demo_live_semantic: false,
                demo_live_sixteen_track: false,
                demo_live_effects_and_buses: false,
                demo_live_patch_editor: false,
                demo_live_detail_and_assets: false,
                defeat_detail_and_assets_preview: false,
                defeat_patch_selection: false,
                demo_component_library: false,
                degenerate: None,
            }
        );
        assert_eq!(
            parse_options(["--demo-scene", "--observe", "--smoke"]).unwrap(),
            Options {
                smoke: true,
                observe: true,
                demo_scene: true,
                full_instrument_effect_demo: false,
                demo_live_drum_rack: false,
                demo_live: false,
                demo_live_mixer: false,
                demo_live_semantic: false,
                demo_live_sixteen_track: false,
                demo_live_effects_and_buses: false,
                demo_live_patch_editor: false,
                demo_live_detail_and_assets: false,
                defeat_detail_and_assets_preview: false,
                defeat_patch_selection: false,
                demo_component_library: false,
                degenerate: None,
            }
        );
        assert_eq!(
            parse_options(["--smoke", "--observe", "--degenerate-audio"]).unwrap(),
            Options {
                smoke: true,
                observe: true,
                demo_scene: false,
                full_instrument_effect_demo: false,
                demo_live_drum_rack: false,
                demo_live: false,
                demo_live_mixer: false,
                demo_live_semantic: false,
                demo_live_sixteen_track: false,
                demo_live_effects_and_buses: false,
                demo_live_patch_editor: false,
                demo_live_detail_and_assets: false,
                defeat_detail_and_assets_preview: false,
                defeat_patch_selection: false,
                demo_component_library: false,
                degenerate: Some(DegenerateMode::Audio),
            }
        );
        assert_eq!(
            parse_options([
                "--degenerate-control",
                "--demo-scene",
                "--observe",
                "--smoke",
            ])
            .unwrap(),
            Options {
                smoke: true,
                observe: true,
                demo_scene: true,
                full_instrument_effect_demo: false,
                demo_live_drum_rack: false,
                demo_live: false,
                demo_live_mixer: false,
                demo_live_semantic: false,
                demo_live_sixteen_track: false,
                demo_live_effects_and_buses: false,
                demo_live_patch_editor: false,
                demo_live_detail_and_assets: false,
                defeat_detail_and_assets_preview: false,
                defeat_patch_selection: false,
                demo_component_library: false,
                degenerate: Some(DegenerateMode::Control),
            }
        );
        assert_eq!(
            parse_options(["--demo-live"]).unwrap(),
            Options {
                demo_live: true,
                demo_live_detail_and_assets: true,
                demo_live_patch_editor: false,
                defeat_patch_selection: false,
                ..Options::default()
            }
        );
        assert_eq!(
            parse_options(["--demo-live-effects-and-buses"]).unwrap(),
            Options {
                demo_live: true,
                demo_live_effects_and_buses: true,
                demo_live_patch_editor: false,
                defeat_patch_selection: false,
                ..Options::default()
            }
        );
        assert_eq!(
            parse_options(["--demo-live-sixteen-track-mixer-routing"]).unwrap(),
            Options {
                demo_live: true,
                demo_live_sixteen_track: true,
                ..Options::default()
            }
        );
        assert_eq!(
            parse_options(["--demo-live-semantic-view-model"]).unwrap(),
            Options {
                demo_live: true,
                demo_live_semantic: true,
                ..Options::default()
            }
        );
        assert_eq!(
            parse_options(["--demo-live-graphical-shell"]).unwrap(),
            Options {
                demo_live: true,
                ..Options::default()
            }
        );
    }

    #[test]
    fn phase7_target_alias_negative_and_production_sample_fixtures_are_exact() {
        use crest_synth::synth::{
            AssetFileId, FileBrowserFolderId, FileBrowserRowKind, SampleAssetCatalogPort,
            SampleDecoderPort,
        };

        assert_eq!(
            parse_options(["--demo-live-detail-and-assets"]).unwrap(),
            Options {
                demo_live: true,
                demo_live_detail_and_assets: true,
                ..Options::default()
            }
        );
        assert_eq!(
            parse_options([
                "--demo-live-detail-and-assets",
                "--defeat-detail-and-assets-preview",
            ])
            .unwrap(),
            Options {
                demo_live: true,
                demo_live_detail_and_assets: true,
                defeat_detail_and_assets_preview: true,
                ..Options::default()
            }
        );
        assert!(parse_options(["--defeat-detail-and-assets-preview"]).is_err());

        let library = DemoSampleLibrary::create(false).unwrap();
        let catalog =
            crest_synth::adapter::filesystem_sample_catalog::FilesystemSampleCatalog::new(
                &library.root,
            )
            .unwrap();
        let listing = catalog.list(&FileBrowserFolderId::default()).unwrap();
        let files = listing
            .rows()
            .iter()
            .filter_map(|row| match row.kind() {
                FileBrowserRowKind::File(asset) => Some(asset.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(files, ["A-valid.wav", "B-alternate.wav", "Z-invalid.wav"]);
        let decoder = crest_synth::adapter::wav_sample_decoder::WavSampleDecoder;
        for name in ["A-valid.wav", "B-alternate.wav"] {
            let asset = AssetFileId::new(name).unwrap();
            let decoded = decoder
                .decode(&asset, &catalog.read(&asset).unwrap())
                .unwrap();
            assert_eq!(decoded.metadata().sample_rate(), 48_000);
            assert_eq!(decoded.metadata().channels(), 1);
        }
        let invalid = AssetFileId::new("Z-invalid.wav").unwrap();
        assert!(decoder
            .decode(&invalid, &catalog.read(&invalid).unwrap())
            .is_err());

        let makefile = include_str!("../../Makefile");
        assert!(makefile.contains("demo-live: demo-live-detail-and-assets"));
        assert!(makefile.contains("--demo-live-detail-and-assets"));
        assert!(makefile.contains("--defeat-detail-and-assets-preview"));
    }

    /// The functional Patch editor scene is additive: it stands alone, its
    /// negative is accepted only beside it, and `--demo-live` keeps pointing
    /// at the cumulative detail-and-assets scene.
    #[test]
    fn the_patch_editor_scene_is_additive_and_its_negative_is_bound_to_it() {
        assert_eq!(
            parse_options(["--demo-live-patch-editor"]).unwrap(),
            Options {
                demo_live: true,
                demo_live_patch_editor: true,
                ..Options::default()
            }
        );
        assert_eq!(
            parse_options(["--demo-live-patch-editor", "--defeat-patch-selection"]).unwrap(),
            Options {
                demo_live: true,
                demo_live_patch_editor: true,
                defeat_patch_selection: true,
                ..Options::default()
            }
        );

        // `demo-live` is unchanged: still the cumulative scene, never this one.
        let alias = parse_options(["--demo-live"]).unwrap();
        assert!(alias.demo_live_detail_and_assets);
        assert!(!alias.demo_live_patch_editor);
        assert!(!alias.demo_live_mixer);

        // The negative is rejected outside its scene, and outside every other.
        assert!(parse_options(["--defeat-patch-selection"]).is_err());
        assert!(parse_options(["--demo-live", "--defeat-patch-selection"]).is_err());
        assert!(
            parse_options(["--demo-live-effects-and-buses", "--defeat-patch-selection",]).is_err()
        );
        assert!(parse_options(["--smoke", "--defeat-patch-selection"]).is_err());

        // The mode itself refuses to combine with any incompatible flag.
        assert!(parse_options(["--demo-live-patch-editor", "--demo-live"]).is_err());
        assert!(parse_options(["--demo-live", "--demo-live-patch-editor"]).is_err());
        assert!(parse_options(["--demo-live-patch-editor", "--smoke"]).is_err());
        assert!(parse_options(["--demo-live-patch-editor", "--demo-live-patch-editor"]).is_err());
        assert!(parse_options([
            "--demo-live-patch-editor",
            "--defeat-patch-selection",
            "--defeat-patch-selection",
        ])
        .is_err());
    }

    #[test]
    fn the_dedicated_mixer_scene_is_additive_and_the_live_alias_is_unchanged() {
        assert_eq!(
            parse_options(["--demo-live-mixer"]).unwrap(),
            Options {
                demo_live: true,
                demo_live_mixer: true,
                ..Options::default()
            }
        );
        let alias = parse_options(["--demo-live"]).unwrap();
        assert!(alias.demo_live_detail_and_assets);
        assert!(!alias.demo_live_mixer);
        assert!(parse_options(["--demo-live-mixer", "--demo-live"]).is_err());
        assert!(parse_options(["--demo-live", "--demo-live-mixer"]).is_err());
        assert!(parse_options(["--demo-live-mixer", "--demo-live-mixer"]).is_err());
        assert!(parse_options(["--demo-live-mixer", "--smoke"]).is_err());

        let makefile = include_str!("../../Makefile");
        assert!(makefile.contains(
            "demo-live-mixer: cache-guard ## Run the dedicated sixteen-track Mixer demo"
        ));
        assert!(makefile.contains("cargo run --release --bin crest-synth -- --demo-live-mixer"));
        assert!(makefile.contains("demo-live: demo-live-detail-and-assets"));
        assert!(!makefile.contains("demo-live: demo-live-mixer"));
    }

    #[test]
    fn drum_rack_demo_is_an_isolated_additive_option() {
        assert_eq!(
            parse_options(["--demo-live-drum-rack"]).unwrap(),
            Options {
                demo_live_drum_rack: true,
                ..Options::default()
            }
        );
        for other in [
            "--demo-live-drum-rack",
            "--full-instrument-effect-demo",
            "--demo-live",
            "--smoke",
            "--demo-live-component-library",
            "--defeat-patch-selection",
        ] {
            assert!(parse_options(["--demo-live-drum-rack", other]).is_err());
            assert!(parse_options([other, "--demo-live-drum-rack"]).is_err());
        }
        assert!(
            parse_options(["--demo-live"])
                .unwrap()
                .demo_live_detail_and_assets
        );
    }

    /// The gallery is its own scene, not a live-demo alias.
    ///
    /// `--demo-live` must keep resolving to the newest cumulative autonomous
    /// witness, and the gallery must never set any `demo_live` flag: the
    /// autonomous scenes' input isolation protects a generation-correlated
    /// claim this scene deliberately does not make.
    #[test]
    fn the_component_gallery_is_its_own_option_and_never_a_live_demo_alias() {
        assert_eq!(
            parse_options(["--demo-live-component-library"]).unwrap(),
            Options {
                demo_component_library: true,
                ..Options::default()
            }
        );
        let gallery = parse_options(["--demo-live-component-library"]).unwrap();
        assert!(!gallery.demo_live);
        assert!(!gallery.demo_live_mixer);
        assert!(!gallery.demo_live_effects_and_buses);
        assert!(!gallery.demo_live_sixteen_track);
        assert!(!gallery.demo_live_semantic);

        let alias = parse_options(["--demo-live"]).unwrap();
        assert!(!alias.demo_component_library);
        assert!(alias.demo_live_detail_and_assets);

        assert!(parse_options([
            "--demo-live-component-library",
            "--demo-live-component-library",
        ])
        .is_err());
        assert!(parse_options(["--demo-live-component-library", "--demo-live"]).is_err());
        assert!(parse_options(["--demo-live", "--demo-live-component-library"]).is_err());
        assert!(parse_options(["--demo-live-component-library", "--smoke"]).is_err());
        assert!(parse_options(["--demo-live-component-library", "--smoke", "--observe"]).is_err());
    }

    #[test]
    fn rejects_unknown_duplicate_and_out_of_context_options() {
        assert!(parse_options(["--play"]).is_err());
        // A launch flag choosing a renderer no longer exists: the webview
        // shell is the only shell (mission webview-shell-cutover WP07).
        assert!(parse_options(["--shell"]).is_err());
        assert!(parse_options(["--shell", "webview"]).is_err());
        assert!(parse_options(["--smoke", "--smoke"]).is_err());
        assert!(parse_options(["--observe"]).is_err());
        assert!(parse_options(["--demo-scene"]).is_err());
        assert!(parse_options(["--smoke", "--demo-scene"]).is_err());
        assert!(parse_options(["--smoke", "--observe", "--demo-scene", "--demo-scene",]).is_err());
        assert!(parse_options(["--smoke", "--degenerate-audio"]).is_err());
        assert!(parse_options(["--demo-live", "--demo-live"]).is_err());
        assert!(parse_options(["--demo-live", "--demo-live-effects-and-buses"]).is_err());
        assert!(parse_options(["--demo-live", "--demo-live-mixer"]).is_err());
        assert!(parse_options(["--demo-live-effects-and-buses", "--smoke"]).is_err());
        assert!(
            parse_options(["--demo-live", "--demo-live-sixteen-track-mixer-routing",]).is_err()
        );
        assert!(parse_options(["--demo-live", "--demo-live-semantic-view-model"]).is_err());
        assert!(parse_options(["--demo-live", "--demo-live-graphical-shell"]).is_err());
        assert!(parse_options(["--demo-live-graphical-shell", "--smoke"]).is_err());
        assert!(parse_options(["--demo-live", "--smoke"]).is_err());
        assert!(parse_options(["--demo-live", "--observe", "--smoke"]).is_err());
        assert!(parse_options(["--demo-live", "--smoke", "--observe", "--demo-scene",]).is_err());
        assert!(
            parse_options(["--demo-live", "--smoke", "--observe", "--degenerate-audio",]).is_err()
        );
        assert!(parse_options([
            "--smoke",
            "--observe",
            "--degenerate-audio",
            "--degenerate-control",
        ])
        .is_err());
    }

    #[test]
    fn witness_enum_surfaces_remain_exhaustive() {
        let directions = [
            EventDirection::Up,
            EventDirection::Down,
            EventDirection::Left,
            EventDirection::Right,
        ];
        let midi_kinds = [
            MidiKind::NoteOn,
            MidiKind::NoteOff,
            MidiKind::ControlChange,
            MidiKind::ProgramChange,
            MidiKind::ChannelPressure,
            MidiKind::PitchBend,
            MidiKind::AllNotesOff,
        ];

        assert_eq!(directions.len(), 4);
        assert_eq!(midi_kinds.len(), 7);
    }
}
