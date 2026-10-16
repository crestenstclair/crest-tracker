//! The functional PATCH editor, proved through the production reducer, the
//! production projection, the committed render script, and the real-time
//! callback.
//!
//! Realizes `asset.FunctionalPatchEditorAcceptanceTests` and the declared
//! project validation `validation.functional_patch_editor`, which asserts exit
//! code 0 and the exact marker [`ACCEPTANCE_MARKER`] on stdout.
//!
//! **Every guard here has been demonstrated to fail when its subject is
//! defeated.** The mission's recurring defect is unexecuted evidence — a guard
//! that walks something it cannot fail on, a threshold that passes whatever the
//! fixture happens to be comfortably above (findings F-28, F-33, F-39, F-42).
//! So each check below names the mutation that falsifies it, and each of those
//! mutations was performed and observed rather than reasoned about.
//!
//! What drives what:
//!
//! - **Reducer** — `AppState::apply` / `apply_semantic_action`. Patch selection,
//!   focus recovery, boundary refusal, surface entry and return, the voice-limit
//!   edit, and MIDI rechannelling are all driven as events, never by reaching
//!   into state.
//! - **Projection** — `StateProjector::project_with_shell`. The semantic model,
//!   the PATCH page, and the shell footer are read as the production projector
//!   emits them.
//! - **Render path** — the document is taken through the production
//!   `ProjectionChannel`, which is exactly what the shipped window emits, and
//!   the committed `webview-page/page.js` derivations are transcribed here
//!   (`page_*` below) the way `tests/component_composition.rs` transcribes them.
//!   Each transcription is pinned to the committed source by
//!   [`check_the_transcribed_page_rules_match_the_committed_script`], so a page
//!   that stops honouring one fails this target rather than drifting from it.
//!   The DOM-level twin is `tests/webview_projection_shell.rs`, which needs a
//!   live window; this file needs none and therefore runs everywhere.
//! - **Callback** — `AudioRenderer::render`, with this binary's own
//!   `#[global_allocator]` counting allocation and destruction across the call.

#[allow(dead_code)]
mod support;

use core::alloc::{GlobalAlloc, Layout};
use core::cell::Cell;
use std::alloc::System;
use std::collections::BTreeSet;
use std::path::PathBuf;

use crest_synth::adapter::braids_capability::{
    BraidsCapability, BRAIDS_CAPABILITY_ID, BRAIDS_FIXED_VOICES,
};
use crest_synth::adapter::chorus_capability::CHORUS_CAPABILITY_ID;
use crest_synth::adapter::hidef_soundfont_capability::{
    HIDEF_CAPABILITY_ID, HIDEF_POLYPHONY_CEILING,
};
use crest_synth::adapter::lock_free_audio_boundary::LockFreeAudioBoundary;
use crest_synth::adapter::production_effects::{
    production_chorus_config, production_effect_preparers, production_effect_registry,
};
use crest_synth::adapter::production_instruments::{
    production_capability_registry, production_instrument_preparers,
    production_instrument_providers, production_soundfont_capability,
};
use crest_synth::adapter::sample_capability::SampleCapability;
use crest_synth::control::{
    AppEvent, AppState, Direction, EventRejection, FocusPath, InteractionMode, PatchControlId,
    PatchDetailSubject, PatchPageProjection, PatchPageSection, PatchPageSlotOccupancy,
    SemanticAction, SemanticControlId, SemanticControlKind, SemanticControlValue,
    SemanticControlViewModel, SemanticGraphicalViewModel, SemanticResolver, SemanticSurfaceRole,
    SemanticSurfaceViewModel, StateProjector, SurfaceId, TopLevelContext,
};
use crest_synth::kernel::midi_channel::MidiChannel;
use crest_synth::kernel::midi_message::{MidiMessage, MidiMessageKind};
use crest_synth::kernel::patch_id::PatchId;
use crest_synth::mixer::global_parameters::{GlobalParameter, GlobalParameters};
use crest_synth::mixer::mixer_track_id::MixerTrackId;
use crest_synth::mixer::patch_output::PatchOutput;
use crest_synth::real_time::audio_boundary::{AudioBoundary, ControlAudioBoundary};
use crest_synth::real_time::audio_command::AudioCommand;
use crest_synth::real_time::audio_observation::{AudioObservation, ControlAudioObservation};
use crest_synth::real_time::audio_renderer::AudioRenderer;
use crest_synth::real_time::prepared_graph_builder::PreparedGraphBuilder;
use crest_synth::real_time::structural_graph_boundary::NoStructuralGraphChanges;
use crest_synth::real_time::GraphRevision;
use crest_synth::shell::webview::projection_channel::{ProjectionChannel, ProjectionPush};
use crest_synth::synth::effect_slot_id::EffectSlotIndex;
use crest_synth::synth::instrument_capability::ParameterValue;
use crest_synth::synth::sound_font_instrument::SoundFontInstrument;
use crest_synth::synth::voice_limit::{VoiceLimit, VoiceLimitError};
use crest_synth::synth::{
    AssetFileId, CapabilityRegistry, EffectSlotId, FileBrowserFolderId, FileBrowserListing,
    FileBrowserRow, FileBrowserRowKind, InstrumentCapabilityProvider, InstrumentConfig,
    ParameterKind, Patch, PatchInteraction,
};
use crest_synth::testing::automatic_midi_test::create_soundfont_config;
use serde_json::Value;

/// The exact string `validation.functional_patch_editor` asserts on stdout.
///
/// Printed by [`functional_patch_editor_acceptance`] and nowhere else, strictly
/// after every declared check has returned.
const ACCEPTANCE_MARKER: &str = "CREST_ACCEPTANCE functional_patch_editor passed";

/// The page's authored unavailable mark, transcribed from
/// `webview-page/page.js` (`parameter_row::UNAVAILABLE_MARK`) and pinned by
/// [`check_the_transcribed_page_rules_match_the_committed_script`].
const UNAVAILABLE_MARK: &str = "--";

/// The strip's declared groups, in declared order, transcribed from
/// `DESIGNED_STRIP_GROUPS` in `webview-page/page.js`.
///
/// `(key, legend, designed)`. A `designed` group with no rows marks itself
/// unavailable rather than vanishing; `capability` is not designed, so it is
/// simply absent on an engine that declares no capability rows.
const DESIGNED_STRIP_GROUPS: [(&str, Option<&str>, bool); 6] = [
    ("instrument", Some("INSTRUMENT"), true),
    ("envelope", Some("AMP ENVELOPE"), true),
    ("capability", None, false),
    ("slot.0", Some("SLOT 1"), true),
    ("slot.1", Some("SLOT 2"), true),
    ("slot.2", Some("SLOT 3"), true),
];

const SAMPLE_RATE: f32 = 48_000.0;
const BLOCK_FRAMES: usize = 256;
const BLOCK_SAMPLES: usize = BLOCK_FRAMES * 2;

// ---------------------------------------------------------------------------
// Allocation counting for the callback proof
// ---------------------------------------------------------------------------

thread_local! {
    static COUNT_MEMORY: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    static DEALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

struct AcceptanceAllocator;

#[global_allocator]
static ACCEPTANCE_ALLOCATOR: AcceptanceAllocator = AcceptanceAllocator;

fn record_allocation() {
    if COUNT_MEMORY.with(Cell::get) {
        ALLOCATIONS.with(|count| count.set(count.get() + 1));
    }
}

fn record_deallocation() {
    if COUNT_MEMORY.with(Cell::get) {
        DEALLOCATIONS.with(|count| count.set(count.get() + 1));
    }
}

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

fn begin_memory_count() {
    ALLOCATIONS.with(|count| count.set(0));
    DEALLOCATIONS.with(|count| count.set(0));
    COUNT_MEMORY.with(|enabled| enabled.set(true));
}

fn finish_memory_count() -> (usize, usize) {
    COUNT_MEMORY.with(|enabled| enabled.set(false));
    (ALLOCATIONS.with(Cell::get), DEALLOCATIONS.with(Cell::get))
}

// ---------------------------------------------------------------------------
// The fixture
// ---------------------------------------------------------------------------

fn soundfont_config(bank: u16, program: u8) -> InstrumentConfig {
    create_soundfont_config(
        &production_soundfont_capability().expect("the production SoundFont capability"),
        SoundFontInstrument::new(bank, program, false).expect("a valid SF2 coordinate"),
    )
    .expect("the production SoundFont config")
}

/// **Four** installed Patches across **both** engines.
///
/// More than two, and not all of one capability, because the mission's headline
/// claim is falsifiable only by a fixture whose schemas actually disagree:
///
/// - Patch 1 `Lead` — SoundFont, and the *widest* schema here. It hosts a
///   `soundfont.preset` `StructuralChoice` row on PATCH Main, a read-only
///   `soundfont.file` asset row, and **two** occupied effect positions holding
///   the *same* registry entry at distinct `EffectSlotId`s, which is what makes
///   "two slots of one entry are two subjects" provable at all.
/// - Patch 2 `Bass` — Braids. Its three capability rows are all `ReadOnly`, so
///   it hosts **no** focusable capability row on PATCH Main: the narrower
///   schema a switch has to recover against.
/// - Patch 3 `Pad` — SoundFont again, at a different preset and with no
///   effects, so "the destination's own values" is not satisfiable by the
///   source's.
/// - Patch 4 `Sub` — Braids again, so the last position is not the only Braids
///   one and the end-of-order refusal is not confounded with a capability
///   change.
///
/// A two-Patch same-capability fixture would agree with itself by accident.
fn fixture_state() -> AppState {
    let registry = production_capability_registry().expect("the production instrument registry");
    let creation_blueprint = crest_synth::control::PatchCreationBlueprint::resolve(
        &crest_synth::synth::CapabilityId::new(HIDEF_CAPABILITY_ID).unwrap(),
        &crest_synth::synth::DescriptorDefaultConfigFactory::new(
            registry.clone(),
            production_instrument_providers().unwrap(),
        ),
    )
    .unwrap();
    let mut state = AppState::new_with_effects(
        registry,
        production_effect_registry().expect("the production effect registry"),
        GlobalParameters::new(-3.0).expect("a valid master gain"),
    )
    .with_patch_creation_blueprint(creation_blueprint);
    let braids = || {
        BraidsCapability::new()
            .expect("the production Braids capability")
            .default_config()
            .expect("the Braids descriptor default config")
    };
    state
        .apply(AppEvent::InstallPatches(vec![
            Patch::new(
                PatchId::new(1).unwrap(),
                "Lead".to_owned(),
                soundfont_config(0, 40),
                MidiChannel::new(0).unwrap(),
                PatchOutput::to_track(MixerTrackId::new(0).unwrap()),
            )
            .with_effect_slot(
                EffectSlotIndex::ALL[0],
                production_chorus_config(EffectSlotId::new(1).unwrap()).unwrap(),
            )
            .with_effect_slot(
                EffectSlotIndex::ALL[1],
                production_chorus_config(EffectSlotId::new(2).unwrap()).unwrap(),
            ),
            Patch::new(
                PatchId::new(2).unwrap(),
                "Bass".to_owned(),
                braids(),
                MidiChannel::new(1).unwrap(),
                PatchOutput::to_track(MixerTrackId::new(1).unwrap()),
            ),
            Patch::new(
                PatchId::new(3).unwrap(),
                "Pad".to_owned(),
                soundfont_config(0, 48),
                MidiChannel::new(2).unwrap(),
                PatchOutput::to_track(MixerTrackId::new(2).unwrap()),
            ),
            Patch::new(
                PatchId::new(4).unwrap(),
                "Sub".to_owned(),
                braids(),
                MidiChannel::new(3).unwrap(),
                PatchOutput::to_track(MixerTrackId::new(3).unwrap()),
            ),
        ]))
        .expect("the fixture installs four Patches across both engines");
    state
        .apply(AppEvent::SelectContext(TopLevelContext::Patch))
        .expect("the PATCH context is reachable with Patches installed");
    state
}

/// One installed Sample Patch with a deterministic controller-native root
/// listing, opened all the way to the trapped browser through semantic
/// actions. This fixture exists so cross-surface guards see Phase 7's real
/// browser projection rather than merely adding its enum name to an expected
/// set.
fn file_browser_fixture() -> AppState {
    let asset_id = AssetFileId::new("Factory.wav").unwrap();
    let provider = SampleCapability::new(asset_id.clone()).unwrap();
    let registry = CapabilityRegistry::new(vec![provider.descriptor()]).unwrap();
    let folder = FileBrowserFolderId::default();
    let listing = FileBrowserListing::new(
        folder.clone(),
        vec![
            FileBrowserRow::new(
                "file:Factory.wav",
                "Factory.wav",
                FileBrowserRowKind::File(asset_id),
                Some(128),
            )
            .unwrap(),
            FileBrowserRow::new(
                "cancel:",
                "CANCEL — UNCHANGED",
                FileBrowserRowKind::Cancel,
                None,
            )
            .unwrap(),
        ],
    )
    .unwrap();
    let patch = Patch::new(
        PatchId::new(1).unwrap(),
        "Sample Fixture".to_owned(),
        provider.default_config().unwrap(),
        MidiChannel::new(0).unwrap(),
        PatchOutput::default(),
    );
    let mut state = AppState::for_graph(
        registry,
        GlobalParameters::new(-3.0).unwrap(),
        GraphRevision::INITIAL,
    )
    .with_sample_catalog([(folder, Ok(listing))]);
    state.apply(AppEvent::InstallPatches(vec![patch])).unwrap();
    state
        .apply_semantic_action(SemanticAction::SelectContext(TopLevelContext::Patch))
        .unwrap();
    state
        .apply_semantic_action(SemanticAction::OpenRelated)
        .unwrap();
    state
        .apply_semantic_action(SemanticAction::OpenRelated)
        .unwrap();
    assert_eq!(state.interaction().active_surface(), SurfaceId::FileBrowser);
    state
}

fn patch_of(state: &AppState, id: PatchId) -> &Patch {
    state
        .patches()
        .iter()
        .find(|patch| patch.id() == id)
        .unwrap_or_else(|| panic!("{id:?} is installed"))
}

fn focused_patch(state: &AppState) -> &Patch {
    patch_of(
        state,
        state
            .interaction()
            .patch_focus()
            .expect("PATCH always has a focused Patch"),
    )
}

fn semantic(state: &AppState) -> SemanticGraphicalViewModel {
    StateProjector::new()
        .project_with_shell(state)
        .expect("the fixture state projects")
        .3
        .semantic_model()
        .clone()
}

fn page(state: &AppState) -> PatchPageProjection {
    StateProjector::new()
        .project_with_shell(state)
        .expect("the fixture state projects")
        .1
        .expect("the PATCH context always projects a page")
}

/// The exact bytes the shipped window hands the render script.
///
/// Taken through the production [`ProjectionChannel`], not serialized here, so
/// a transport that changed what it emits fails this rather than agreeing with
/// a second serializer owned by the test.
fn document(state: &AppState) -> Value {
    let projection = StateProjector::new()
        .project_with_shell(state)
        .expect("the fixture state projects")
        .3;
    let mut channel = ProjectionChannel::new();
    let mut emitted = None;
    let outcome = channel
        .push(&projection, |document| {
            emitted = Some(document);
            Ok(())
        })
        .expect("the production emit succeeds");
    assert_eq!(outcome, ProjectionPush::Emitted);
    emitted.expect("an Emitted push hands the emitter exactly one document")
}

fn all_controls(model: &SemanticGraphicalViewModel) -> Vec<&SemanticControlViewModel> {
    model
        .surfaces()
        .iter()
        .flat_map(SemanticSurfaceViewModel::controls)
        .collect()
}

fn control_at<'a>(
    model: &'a SemanticGraphicalViewModel,
    control: &SemanticControlId,
) -> &'a SemanticControlViewModel {
    all_controls(model)
        .into_iter()
        .find(|candidate| candidate.path().control_id() == control)
        .unwrap_or_else(|| panic!("{control:?} must be projected"))
}

/// Walks the reducer to the row `predicate` names, refusing to loop forever.
fn navigate_to(state: &mut AppState, predicate: impl Fn(&FocusPath) -> bool) {
    for direction in [Direction::Down, Direction::Up] {
        for _ in 0..256 {
            if predicate(state.interaction().focus_path()) {
                return;
            }
            if state.apply(AppEvent::Navigate(direction)).is_err() {
                break;
            }
        }
    }
    panic!("no row on this surface satisfied the predicate");
}

fn enter_utility_row(state: &mut AppState, control: &PatchControlId) {
    state
        .apply(AppEvent::EnterSurface(SurfaceId::PatchUtility))
        .expect("the Utility panel is enterable from PATCH Main");
    navigate_to(state, |path| {
        path.control_id() == &SemanticControlId::Patch(control.clone())
    });
}

fn set_mode(state: &mut AppState, mode: InteractionMode) {
    state
        .apply(AppEvent::SetInteractionMode(mode))
        .expect("both phase-two modes are reachable");
}

// ---------------------------------------------------------------------------
// The committed render script, transcribed
// ---------------------------------------------------------------------------

/// The render script with its `//` comments removed, so a word that appears
/// only in a note *about* the vocabulary is not read as the page composing it.
///
/// Scanned per line, with quote state reset at each newline. A JavaScript
/// regex literal can carry an unbalanced quote (`/[&<>"']/g`), so tracking
/// quotes across the whole file desynchronizes on the first one and silently
/// stops stripping every comment after it — which would make this guard pass
/// by blindness rather than by the page being clean.
fn script_without_comments(script: &str) -> String {
    let mut out = String::with_capacity(script.len());
    for line in script.lines() {
        let mut quote: Option<char> = None;
        let mut escaped = false;
        let characters: Vec<char> = line.chars().collect();
        let mut index = 0;
        while index < characters.len() {
            let character = characters[index];
            match quote {
                Some(open) => {
                    if escaped {
                        escaped = false;
                    } else if character == '\\' {
                        escaped = true;
                    } else if character == open {
                        quote = None;
                    }
                }
                None => {
                    if character == '"' || character == '\'' || character == '`' {
                        quote = Some(character);
                    } else if character == '/' && characters.get(index + 1) == Some(&'/') {
                        break;
                    }
                }
            }
            out.push(character);
            index += 1;
        }
        out.push('\n');
    }
    out
}

fn page_source(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("webview-page")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path:?} is readable: {error}"))
}

/// `controlIdOf` — the serialized identity of one projected control.
fn page_control_id(control: &Value) -> String {
    match control.pointer("/path/controlId/id") {
        Some(Value::String(id)) => id.clone(),
        Some(Value::Null) | None => String::new(),
        Some(id) => id.to_string(),
    }
}

fn page_normalized_percentage(control: &Value) -> bool {
    if control
        .get("selectedLabel")
        .is_some_and(|label| !label.is_null())
    {
        return false;
    }
    if control.get("unit").is_some_and(|unit| !unit.is_null()) {
        return false;
    }
    matches!(
        (
            control
                .pointer("/numericRange/minimum")
                .and_then(Value::as_f64),
            control
                .pointer("/numericRange/maximum")
                .and_then(Value::as_f64),
        ),
        (Some(0.0), Some(1.0)) | (Some(-1.0), Some(1.0))
    )
}

fn page_numeric_scale(control: &Value) -> f64 {
    if page_normalized_percentage(control) {
        100.0
    } else {
        1.0
    }
}

fn page_decimal_places_for_step(step: f64) -> usize {
    if !step.is_finite() || step <= 0.0 {
        return 3;
    }
    (0..=6)
        .find(|places| {
            let scaled = step * 10_f64.powi(*places);
            (scaled - scaled.round()).abs() < 0.000_000_1
        })
        .unwrap_or(6) as usize
}

fn page_numeric_value_text(control: &Value, value: f64) -> String {
    if !value.is_finite() {
        return UNAVAILABLE_MARK.to_owned();
    }
    let scale = page_numeric_scale(control);
    let step = control
        .pointer("/numericRange/fineStep")
        .and_then(Value::as_f64)
        .unwrap_or(0.001)
        * scale;
    let places = page_decimal_places_for_step(step).max(3);
    let mut text = format!("{:.*}", places, value * scale);
    if text.contains('.') {
        while text.ends_with('0') {
            text.pop();
        }
        if text.ends_with('.') {
            text.pop();
        }
    }
    if text == "-0" {
        "0".to_owned()
    } else {
        text
    }
}

fn page_unit_text(control: &Value) -> Option<String> {
    if control
        .get("selectedLabel")
        .is_some_and(|label| !label.is_null())
    {
        return None;
    }
    control
        .get("unit")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| page_normalized_percentage(control).then(|| "%".to_owned()))
}

/// `controlValueText` — one typed document value as finished screen text.
///
/// The choice arm is the one this mission changed: it reads the projected
/// authored name and falls back to the stored id only when the descriptor
/// declared none, so a choice id can no longer reach the screen through the
/// value slot (mission finding F-33).
fn page_value_text(control: &Value) -> String {
    fn display(value: &Value) -> String {
        match value {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        }
    }
    let Some(value) = control.get("value").filter(|value| value.is_object()) else {
        return UNAVAILABLE_MARK.to_owned();
    };
    let kind = control.get("kind").and_then(Value::as_str).unwrap_or("");
    match value.get("kind").and_then(Value::as_str) {
        Some("scalar") => {
            let number = value["value"].as_f64().unwrap_or(f64::NAN);
            if kind == "stepped" {
                format!("{}", number.round() as i64)
            } else {
                page_numeric_value_text(control, number)
            }
        }
        Some("parameter") => {
            let Some(parameter) = value.get("value").filter(|value| value.is_object()) else {
                return UNAVAILABLE_MARK.to_owned();
            };
            match parameter.get("kind").and_then(Value::as_str) {
                Some("continuous") => control
                    .get("selectedLabel")
                    .filter(|label| !label.is_null())
                    .map_or_else(
                        || {
                            page_numeric_value_text(
                                control,
                                parameter["value"].as_f64().unwrap_or(f64::NAN),
                            )
                        },
                        display,
                    ),
                Some("stepped" | "choice") => control
                    .get("selectedLabel")
                    .filter(|label| !label.is_null())
                    .map_or_else(|| display(&parameter["value"]), display),
                Some("toggle") => if parameter["value"] == Value::Bool(true) {
                    "ON"
                } else {
                    "OFF"
                }
                .to_owned(),
                Some(other) => format!("?{other}"),
                None => UNAVAILABLE_MARK.to_owned(),
            }
        }
        Some("asset") => value
            .pointer("/value/locator")
            .and_then(Value::as_str)
            .map_or_else(|| UNAVAILABLE_MARK.to_owned(), str::to_owned),
        Some("identity") | Some("summary") => display(&value["value"]),
        Some(other) => format!("?{other}"),
        None => UNAVAILABLE_MARK.to_owned(),
    }
}

/// The lifecycle band's rendering of an in-flight requested value —
/// `controlValueText({kind, value: requested, selectedLabel: requestedLabel})`.
fn page_requested_value_text(control: &Value) -> Option<String> {
    let requested = control.get("requestedValue").filter(|v| !v.is_null())?;
    Some(page_value_text(&serde_json::json!({
        "kind": control.get("kind").cloned().unwrap_or(Value::Null),
        "value": requested.clone(),
        "selectedLabel": control.get("requestedLabel").cloned().unwrap_or(Value::Null),
        "numericRange": control.get("numericRange").cloned().unwrap_or(Value::Null),
        "unit": control.get("unit").cloned().unwrap_or(Value::Null),
    })))
}

/// `rangeEndpointText` + `rangeHtml` — the projected bounds as painted, or
/// `None` when the document carries none.
fn page_range_text(control: &Value) -> Option<String> {
    if control
        .get("selectedLabel")
        .is_some_and(|label| !label.is_null())
    {
        return None;
    }
    let range = control.get("numericRange").filter(|r| r.is_object())?;
    let minimum = range.get("minimum").and_then(Value::as_f64)?;
    let maximum = range.get("maximum").and_then(Value::as_f64)?;
    let endpoint = |value: f64| {
        if control.get("kind").and_then(Value::as_str) == Some("continuous") {
            page_numeric_value_text(control, value)
        } else {
            format!("{value}")
        }
    };
    Some(format!(
        "{}{}{}",
        endpoint(minimum),
        " — ",
        endpoint(maximum)
    ))
}

/// `stripGroupKey` — which designed group one control identity joins.
fn page_strip_group_key(id: &str, open_slot: Option<&str>) -> Option<String> {
    if id == "patch.engine" {
        return Some("instrument".to_owned());
    }
    if id.starts_with("patch.envelope.") {
        return Some("envelope".to_owned());
    }
    if id.starts_with("patch.capability.") {
        return Some("capability".to_owned());
    }
    if let Some(index) = id.strip_prefix("patch.effectSlot.") {
        return Some(format!("slot.{index}"));
    }
    if id.starts_with("patch.effect.") {
        return open_slot.map(str::to_owned);
    }
    None
}

#[derive(Debug)]
struct StripGroup {
    key: String,
    legend: Option<String>,
    designed: bool,
    unknown: bool,
    rows: Vec<Value>,
}

/// `stripGroups` — arranges one surface's visible controls into ordered groups,
/// including the declared groups the walk never opened.
fn page_strip_groups(controls: &[Value]) -> Vec<StripGroup> {
    let declared = |key: &str| {
        DESIGNED_STRIP_GROUPS
            .iter()
            .find(|(candidate, ..)| *candidate == key)
            .copied()
    };
    let mut groups: Vec<StripGroup> = Vec::new();
    let mut open_slot: Option<String> = None;
    let push_row = |groups: &mut Vec<StripGroup>, key: &str, unknown: bool, row: &Value| {
        if let Some(existing) = groups.iter_mut().find(|group| group.key == key) {
            existing.rows.push(row.clone());
            return;
        }
        let declared = declared(key);
        groups.push(StripGroup {
            key: key.to_owned(),
            legend: declared
                .and_then(|(_, legend, _)| legend)
                .map(str::to_owned),
            designed: declared.is_some_and(|(.., designed)| designed),
            unknown,
            rows: vec![row.clone()],
        });
    };
    for control in controls {
        if control.get("visible") == Some(&Value::Bool(false)) {
            continue;
        }
        let id = page_control_id(control);
        if let Some(index) = id.strip_prefix("patch.effectSlot.") {
            open_slot = Some(format!("slot.{index}"));
        }
        match page_strip_group_key(&id, open_slot.as_deref()) {
            Some(key) => push_row(&mut groups, &key, false, control),
            None => push_row(&mut groups, "?group", true, control),
        }
    }
    // Every designed group the walk never opened lands at its declared
    // position carrying no rows, so it can mark itself unavailable inside its
    // own group rather than vanishing.
    for (index, (key, legend, designed)) in DESIGNED_STRIP_GROUPS.iter().enumerate().rev() {
        if !designed || groups.iter().any(|group| &group.key == key) {
            continue;
        }
        let at = groups
            .iter()
            .position(|group| {
                DESIGNED_STRIP_GROUPS
                    .iter()
                    .position(|(candidate, ..)| *candidate == group.key)
                    .is_some_and(|position| position > index)
            })
            .unwrap_or(groups.len());
        groups.insert(
            at,
            StripGroup {
                key: (*key).to_owned(),
                legend: legend.map(str::to_owned),
                designed: true,
                unknown: false,
                rows: Vec::new(),
            },
        );
    }
    groups
}

fn surface_of<'a>(document: &'a Value, id: &str) -> Option<&'a Value> {
    document
        .get("surfaces")?
        .as_array()?
        .iter()
        .find(|surface| surface.get("id").and_then(Value::as_str) == Some(id))
}

fn surface_controls(document: &Value, id: &str) -> Vec<Value> {
    surface_of(document, id)
        .and_then(|surface| surface.get("controls"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// `sideRegionHintLine` + `hintRun` + `hintLabel` — the Utility panel's
/// authored hint line, as the text those three put on screen.
///
/// Gathered from projected actions on both sides of the surface boundary: the
/// actions that *enter* this surface from wherever they are projected, and the
/// actions that *leave* it from its own rows. A repeated hint-and-label pair is
/// dropped, a null hint never renders, and each surviving action paints
/// `hint:label` with the label put through `hintLabel` — lowercased, with a
/// leading `open`/`move` word and a trailing `mode` word removed — joined by a
/// text space.
///
/// The last two rules are transcribed because cycle 2's audit found this
/// function had invented them: it joined with `HINT_SEPARATOR`, which the hint
/// run does not use, and read `label` raw, which the page never paints. That is
/// F-55's defect pointed the other way — a rule copied from nowhere, and a pin
/// (`HINT_SEPARATOR`) standing for a rule this file did not transcribe.
fn page_side_hint_line(document: &Value, surface_id: &str) -> String {
    /// `hintLabel` — `/^(open|move)\s+/` and `/\s+mode$/` off the lowercased
    /// label, so a run reads "2:patch" rather than "2:Open Patch".
    fn hint_label(label: &str) -> String {
        let lowered = label.to_lowercase();
        let mut text = lowered.as_str();
        for prefix in ["open", "move"] {
            if let Some(rest) = text.strip_prefix(prefix) {
                if rest.starts_with(char::is_whitespace) {
                    text = rest.trim_start();
                    break;
                }
            }
        }
        if let Some(head) = text.strip_suffix("mode") {
            if head.ends_with(char::is_whitespace) {
                text = head.trim_end();
            }
        }
        text.to_owned()
    }
    let mut seen = BTreeSet::new();
    let mut spans = Vec::new();
    for surface in document
        .get("surfaces")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let this = surface.get("id").and_then(Value::as_str) == Some(surface_id);
        for control in surface
            .get("controls")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            for action in control
                .get("validActions")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                let kind = action.pointer("/action/kind").and_then(Value::as_str);
                let payload = action.pointer("/action/payload").and_then(Value::as_str);
                let enters = kind == Some("enterSurface") && payload == Some(surface_id);
                let leaves = kind == Some("return") && this;
                if !enters && !leaves {
                    continue;
                }
                // The hint-and-label pair, deduped before the null-hint drop,
                // in the page's own order. Carried as a pair rather than as the
                // page's concatenated key because the separator that key uses
                // is F-43's NUL — the merge step replaces it, and the two pins
                // therefore sit either side of it. A pair is the same
                // equivalence for any separator neither field can contain,
                // which is the property the NUL was chosen for.
                let hint = action.get("hint").and_then(Value::as_str);
                let label = action.get("label").and_then(Value::as_str);
                if !seen.insert((
                    hint.unwrap_or("undefined").to_owned(),
                    label.unwrap_or("undefined").to_owned(),
                )) {
                    continue;
                }
                let Some(hint) = hint else {
                    continue;
                };
                spans.push(format!(
                    "{hint}:{}",
                    hint_label(label.unwrap_or("undefined"))
                ));
            }
        }
    }
    spans.join(" ")
}

// ---------------------------------------------------------------------------
// The serialization-key vocabulary, and every screen string
// ---------------------------------------------------------------------------

/// Every serialization key this system addresses a value by.
///
/// The same set the in-crate label guard builds
/// (`semantic_graphical_view_model::projection_enrichment_tests::serialization_keys`),
/// restated here because that helper is `#[cfg(test)]`-private. It carries
/// F-28's widening: capability and section identities are serialization keys
/// too — `contexts/control.yaml` defines one as "the name a value carries in
/// the state tree, the parameter snapshot, or a leaf descriptor", and
/// `patchPage.sections[].id`, `patchPage.engine.activeCapabilityId`, and
/// `patchPage.effects[].capabilityId` are all such names — and it also carries
/// every **choice** identity, which is what F-33 put on screen as a *value*.
fn serialization_keys(state: &AppState) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    for descriptor in GlobalParameters::surface_descriptor() {
        keys.insert(descriptor.name().to_owned());
    }
    for descriptor in crest_synth::synth::VoiceEnvelope::surface_descriptor() {
        keys.insert(descriptor.name().to_owned());
        keys.insert(
            PatchControlId::Envelope(descriptor.parameter())
                .as_str()
                .into_owned(),
        );
    }
    for descriptor in VoiceLimit::surface_descriptor() {
        keys.insert(descriptor.name().to_owned());
    }
    for descriptor in PatchOutput::surface_descriptor() {
        keys.insert(descriptor.name().to_owned());
    }
    for descriptor in
        crest_synth::mixer::mixer_track_parameters::MixerTrackParameters::surface_descriptor()
    {
        keys.insert(descriptor.name().to_owned());
    }
    for control in PatchControlId::UTILITY
        .iter()
        .cloned()
        .chain([PatchControlId::Engine])
    {
        keys.insert(control.as_str().into_owned());
    }
    for descriptor in state.capabilities().descriptors() {
        keys.insert(descriptor.id().to_string());
        for section in descriptor.sections() {
            keys.insert(section.id().to_owned());
        }
        for spec in descriptor.parameters() {
            keys.insert(spec.id().to_string());
            keys.insert(
                PatchControlId::Capability(spec.id().clone())
                    .as_str()
                    .into_owned(),
            );
            keys.extend(spec.choices().iter().map(|choice| choice.id().to_owned()));
        }
    }
    for descriptor in state.effects().descriptors() {
        keys.insert(descriptor.id().to_string());
        for section in descriptor.sections() {
            keys.insert(section.id().to_owned());
        }
        for spec in descriptor.parameters() {
            keys.insert(spec.id().to_string());
            keys.extend(spec.choices().iter().map(|choice| choice.id().to_owned()));
        }
    }
    for path in SemanticGraphicalViewModel::serialized_leaf_descriptor()
        .iter()
        .chain(PatchPageProjection::serialized_leaf_descriptor())
    {
        let leaf = path.rsplit('.').next().unwrap_or(path);
        keys.insert(leaf.trim_end_matches("[]").to_owned());
    }
    keys
}

/// Every string one production projection of `state` puts on screen, tagged
/// with where it came from.
///
/// This walks **labels and values together**, which is the generalization F-33
/// forced: the in-crate guard walks labels only, so a projected *value* that is
/// an identity — a choice id, a capability id in the engine row's identity
/// value, an occupancy row's — reaches the screen with nothing able to fail on
/// it. Both are screen strings and the vocabulary rule is about screens.
///
/// Values are taken through [`page_value_text`], not read raw, so what is
/// checked is what is painted.
fn projected_screen_strings(state: &AppState) -> Vec<(String, String)> {
    let mut strings = Vec::new();
    let document = document(state);
    let (_, page, _, shell, _) = StateProjector::new()
        .project_with_shell(state)
        .expect("the fixture state projects");

    for surface in document
        .get("surfaces")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let surface_id = surface
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_owned();
        if let Some(label) = surface.get("label").and_then(Value::as_str) {
            strings.push((format!("surface {surface_id} label"), label.to_owned()));
        }
        for section in surface
            .get("sections")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(label) = section.get("label").and_then(Value::as_str) {
                strings.push((
                    format!("surface {surface_id} section title"),
                    label.to_owned(),
                ));
            }
        }
        let summary_paths: &[&str] = match surface_id.as_str() {
            "sends" => &["/summary/name", "/summary/choiceGroupLabel"],
            "controllerSettings" => &["/summary/title", "/summary/summary", "/summary/description"],
            _ => &[],
        };
        for path in summary_paths {
            if let Some(text) = surface.pointer(path).and_then(Value::as_str) {
                strings.push((format!("surface {surface_id} {path}"), text.to_owned()));
            }
        }
        if surface_id == "controllerSettings" {
            for device in surface
                .pointer("/summary/controller/devices")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if let Some(name) = device.get("name").and_then(Value::as_str) {
                    strings.push(("controller device name".to_owned(), name.to_owned()));
                }
            }
        }
        for control in surface
            .get("controls")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            let id = page_control_id(&control);
            if let Some(label) = control.get("label").and_then(Value::as_str) {
                strings.push((format!("{surface_id} {id} label"), label.to_owned()));
            }
            let modal = matches!(surface_id.as_str(), "patchChoice" | "fileBrowser")
                || (surface_id == "sends"
                    && surface.pointer("/summary/choosingEffect") == Some(&Value::Bool(true)));
            for marker in ["availabilityLabel", "readOnlyLabel"] {
                if let Some(label) = control.get(marker).and_then(Value::as_str) {
                    strings.push((format!("{surface_id} {id} {marker}"), label.to_owned()));
                }
            }
            if modal {
                if let Some(marker) = control.get("selectedLabel").and_then(Value::as_str) {
                    strings.push((format!("{surface_id} {id} state marker"), marker.to_owned()));
                }
                if let Some(metadata) = control
                    .pointer("/browserMetadata/text")
                    .and_then(Value::as_str)
                {
                    strings.push((format!("{surface_id} {id} metadata"), metadata.to_owned()));
                }
            } else {
                strings.push((
                    format!("{surface_id} {id} painted value"),
                    page_value_text(&control),
                ));
                if let Some(text) = page_requested_value_text(&control) {
                    strings.push((format!("{surface_id} {id} painted requested value"), text));
                }
            }
            for action in control
                .get("validActions")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                if let Some(label) = action.get("label").and_then(Value::as_str) {
                    strings.push((format!("{surface_id} {id} action label"), label.to_owned()));
                }
            }
        }
    }
    for segment in shell.footer().path_label().split(" / ") {
        strings.push((
            "shell footer pathLabel segment".to_owned(),
            segment.to_owned(),
        ));
    }
    if let Some(page) = page {
        strings.push((
            "page engine active".to_owned(),
            page.engine().active_label().to_owned(),
        ));
        for choice in page.engine().choices() {
            strings.push(("page engine choice".to_owned(), choice.label().to_owned()));
        }
        for row in page.envelope() {
            strings.push((
                format!("page envelope {}", row.id()),
                row.label().to_owned(),
            ));
        }
        for row in page.output() {
            strings.push((format!("page output {}", row.id()), row.label().to_owned()));
        }
        fn push_sections(
            strings: &mut Vec<(String, String)>,
            where_: &str,
            sections: &[PatchPageSection],
        ) {
            for section in sections {
                strings.push((
                    format!("{where_} section {}", section.id()),
                    section.label().to_owned(),
                ));
                for row in section.parameters() {
                    let site = format!("{where_} row {}", row.id());
                    strings.push((site.clone(), row.label().to_owned()));
                    if let Some(label) = row.selected_label() {
                        strings.push((format!("{site} selected"), label.to_owned()));
                    }
                    if let Some(label) = row.requested_label() {
                        strings.push((format!("{site} requested"), label.to_owned()));
                    }
                    for choice in row.choices() {
                        strings.push((format!("{site} choice"), choice.label().to_owned()));
                    }
                }
            }
        }
        push_sections(&mut strings, "page main", page.sections());
        for slot in page.effects() {
            let where_ = format!("page effect slot {}", slot.slot_index().index());
            if let PatchPageSlotOccupancy::Occupied { label, .. } = slot.occupancy() {
                strings.push((format!("{where_} occupancy"), label.clone()));
            }
            for choice in slot.choices() {
                strings.push((format!("{where_} choice"), choice.label().to_owned()));
            }
            push_sections(&mut strings, &where_, slot.sections());
        }
        if let Some(detail) = page.detail() {
            strings.push(("page detail".to_owned(), detail.label().to_owned()));
            push_sections(&mut strings, "page detail", detail.sections());
        }
    }
    strings
}

/// The fixtures the screen-string guard walks, and the surface each opens.
///
/// Every surface, both engines, both detail subjects, and a Patch mid-swap, so
/// no site is unreachable by construction. A guard is only as wide as the
/// documents it saw — the exact failure F-28 recorded.
fn screen_string_fixtures() -> Vec<(&'static str, AppState)> {
    let entered = |surface: SurfaceId| {
        let mut state = fixture_state();
        state
            .apply(AppEvent::EnterSurface(surface))
            .expect("the fixture surface is enterable");
        state
    };
    let braids = |surface: Option<SurfaceId>| {
        let mut state = fixture_state();
        state
            .apply(AppEvent::SelectPatch(Direction::Right))
            .expect("the fixture installs a second Patch");
        assert_eq!(
            focused_patch(&state)
                .instrument_config()
                .capability_id()
                .as_str(),
            BRAIDS_CAPABILITY_ID,
        );
        if let Some(surface) = surface {
            state
                .apply(AppEvent::EnterSurface(surface))
                .expect("the fixture surface is enterable");
        }
        state
    };
    let mut effect_detail = fixture_state();
    navigate_to(&mut effect_detail, |path| {
        matches!(
            path.control_id(),
            SemanticControlId::Patch(PatchControlId::EffectSlot(slot)) if slot.index() == 0
        )
    });
    effect_detail
        .apply(AppEvent::EnterSurface(SurfaceId::PatchDetail))
        .expect("an occupied slot row resolves an effect subject");

    let mut mixer = fixture_state();
    mixer
        .apply(AppEvent::SelectContext(TopLevelContext::Mixer))
        .unwrap();
    let mut inspector = mixer.clone();
    inspector
        .apply(AppEvent::EnterSurface(SurfaceId::MixerInspector))
        .unwrap();

    let mut utility_master = entered(SurfaceId::PatchUtility);
    navigate_to(&mut utility_master, |path| {
        matches!(
            path.control_id(),
            SemanticControlId::Patch(PatchControlId::Global(_))
        )
    });

    let mut choice = fixture_state();
    set_mode(&mut choice, InteractionMode::Adjust);
    choice
        .apply(AppEvent::Adjust(Direction::Up))
        .expect("the engine row opens its installed-choice modal");
    assert_eq!(
        choice.interaction().active_surface(),
        SurfaceId::PatchChoice
    );

    let file_browser = file_browser_fixture();

    vec![
        ("soundfont PATCH Main", fixture_state()),
        ("braids PATCH Main", braids(None)),
        (
            "soundfont instrument detail",
            entered(SurfaceId::PatchDetail),
        ),
        (
            "braids instrument detail",
            braids(Some(SurfaceId::PatchDetail)),
        ),
        ("chorus effect detail", effect_detail),
        ("PATCH Utility", entered(SurfaceId::PatchUtility)),
        ("PATCH Utility master gain", utility_master),
        ("preset swap in flight", preset_swap_in_flight().0),
        ("engine choice modal", choice),
        ("Sample Browser", file_browser),
        ("MIXER Main", mixer),
        ("MIXER Inspector", inspector),
    ]
}

/// The focused SoundFont Patch with a preset swap requested and unresolved.
///
/// Returns the state and the row the request rides, so a caller can read both
/// the active and the requested value on the one row that carries them.
fn preset_swap_in_flight() -> (AppState, SemanticControlId) {
    let mut state = fixture_state();
    state
        .apply(AppEvent::EnterSurface(SurfaceId::PatchDetail))
        .expect("the engine root opens its descriptor-backed detail surface");
    let preset = state
        .capabilities()
        .descriptor(focused_patch(&state).instrument_config().capability_id())
        .expect("the focused Patch's descriptor is installed")
        .parameters()
        .find(|spec| spec.patch_interaction() == PatchInteraction::StructuralChoice)
        .expect("the SoundFont descriptor declares a structural choice row")
        .id()
        .clone();
    let control = SemanticControlId::Patch(PatchControlId::Capability(preset));
    navigate_to(&mut state, |path| path.control_id() == &control);
    set_mode(&mut state, InteractionMode::Adjust);
    state
        .apply(AppEvent::Adjust(Direction::Right))
        .expect("the preset row accepts an adjacent structural choice");
    set_mode(&mut state, InteractionMode::Navigate);
    assert!(
        state.engine_selection().is_in_flight(),
        "the fixture must leave a structural edit unresolved"
    );
    (state, control)
}

// ---------------------------------------------------------------------------
// The transcription is pinned to the committed render script
// ---------------------------------------------------------------------------

/// Every rule this file transcribes, checked against the source it claims to
/// transcribe.
///
/// A transcription is a substitute runtime, and F-37 recorded what a substitute
/// that differs from the real one in one dimension costs: confident, precise,
/// wrong numbers. This does not make the transcription right — only the DOM
/// twin in `tests/webview_projection_shell.rs` can do that — but it makes a
/// page that stops honouring one of these rules fail *here*, deterministically,
/// instead of only under a live window.
///
/// **The completeness rule (F-55).** Cycle 1 pinned the strip's group *table*
/// and left the identity→group *mapping* unpinned, so six mutations to
/// `page.js` — each breaking a rule [`page_strip_group_key`],
/// [`page_strip_groups`] or [`page_group_head_control_id`] copies — left this
/// target green. A transcription with an unpinned rule is a transcription of
/// nothing. So this table is not assembled by judgement about which rules
/// matter; it is assembled by one mechanical rule:
///
/// > walk each transcribing function line by line, and for every line that
/// > implements a rule read off `page.js`, pin the `page.js` statement that
/// > implements it — one pin per copied rule, no pin without a copied rule.
///
/// The entries are therefore grouped by the function they defend and named for
/// the rule rather than for the text, so a rule added to a transcription
/// without a pin shows up as a gap in its own group. The last group holds the
/// declared group table, the three constants and the two marks this file
/// transcribes directly — the table as one array literal rather than as six
/// independent entries, so its *order* is pinned along with its membership.
///
/// **Every anchor occurs exactly once, and that is enforced here rather than
/// remembered.** F-42, F-53 and F-55 are one failure mode: an anchor that also
/// matches somewhere else guards nothing, because the site it names can be
/// renamed while the pin stays satisfied. This file shipped two —
/// `control && control.numericRange`, found in cycle 1 by mutation, and
/// `data-role="row-range"`, found in cycle 1's review the same way, matching
/// both the painting site and a `renderObservation` selector. Requiring exactly
/// one occurrence makes the next blind anchor fail the moment it is added.
#[allow(dead_code)]
fn check_legacy_transcribed_page_rules_match_the_committed_script() -> usize {
    let script = page_source("page.js");
    let unavailable_mark = format!("var UNAVAILABLE_MARK = \"{UNAVAILABLE_MARK}\"");
    // The designed group table as one array literal, rebuilt from the constant
    // this file transcribes it into — so the *sequence* is pinned along with
    // the membership. Cycle 2 asserted each of the six entries occurred exactly
    // once, which pins membership and legends six times over and the order not
    // at all: swapping the `envelope` and `capability` entries was MISSED
    // (F-68). The order is a copied rule — `DESIGNED_STRIP_GROUPS` here is
    // ordered identically, `grouped_strip_shape` asserts declared order off it,
    // and the page's declared-position re-insertion walks the JS array by index.
    let designed_strip_groups = format!(
        "var DESIGNED_STRIP_GROUPS = [\n{}  ];",
        DESIGNED_STRIP_GROUPS
            .iter()
            .map(|(key, legend, designed)| match legend {
                Some(legend) => format!(
                    "    {{ key: \"{key}\", legend: \"{legend}\", designed: {designed} }},\n"
                ),
                None => format!("    {{ key: \"{key}\", legend: null, designed: {designed} }},\n"),
            })
            .collect::<String>()
    );
    let required: [(&str, &str); 66] = [
        // `controlIdOf` — [`page_control_id`].
        ("the control identity read", "    var id =\n      control && control.path && control.path.controlId\n        ? control.path.controlId.id\n        : \"\";\n    return id !== null && typeof id === \"object\"\n      ? JSON.stringify(id)\n      : String(id);"),
        // `controlValueText` — [`page_value_text`], arm for arm.
        ("the absent value mark", "    var value = control && control.value;\n    if (!value || typeof value !== \"object\") {\n      return UNAVAILABLE_MARK;"),
        ("the scalar value discriminator", "    if (value.kind === \"scalar\") {"),
        ("the scalar arm's stepped/continuous split", "      return control.kind === \"stepped\"\n        ? String(Math.round(Number(value.value)))\n        : Number(value.value).toFixed(3);"),
        ("the parameter value discriminator", "    if (value.kind === \"parameter\") {\n      var parameter = value.value;\n      if (parameter && typeof parameter === \"object\") {"),
        ("the continuous parameter's three places", "        if (parameter.kind === \"continuous\") {\n          return Number(parameter.value).toFixed(3);"),
        ("the stepped parameter reads its name when available", "        if (parameter.kind === \"stepped\") {\n          return String(control.selectedLabel ?? parameter.value);"),
        ("the choice discriminator", "        if (parameter.kind === \"choice\") {"),
        ("the authored option label read (F-33)", "          return String(\n            control.selectedLabel === null || control.selectedLabel === undefined\n              ? parameter.value\n              : control.selectedLabel\n          );"),
        ("the toggle wording", "        if (parameter.kind === \"toggle\") {\n          return parameter.value ? \"ON\" : \"OFF\";"),
        ("the unknown parameter-kind marker", "        return \"?\" + String(parameter.kind);"),
        ("the malformed parameter mark", "      }\n      return UNAVAILABLE_MARK;"),
        ("the asset discriminator", "    if (value.kind === \"asset\") {"),
        ("the asset locator read", "      return value.value && value.value.locator\n        ? String(value.value.locator)\n        : UNAVAILABLE_MARK;"),
        ("the identity and summary read", "    if (value.kind === \"identity\" || value.kind === \"summary\") {\n      return String(value.value);"),
        ("the unknown value-kind marker", "    return \"?\" + String(value.kind);"),
        // The lifecycle band's requested value — [`page_requested_value_text`].
        ("the requested value read through the active value's presentation", "        ? controlValueText({\n            kind: control.kind,\n            value: requested,"),
        ("the requested option label read (F-33)", "            selectedLabel: control.requestedLabel,"),
        // `rangeEndpointText` and `rangeHtml` — [`page_range_text`].
        ("the endpoint's continuous three places", "    return control.kind === \"continuous\"\n      ? Number(value).toFixed(3)\n      : String(value);"),
        ("the absent range paints nothing", "    var range = control && control.numericRange;\n    if (\n      !range ||\n      typeof range.minimum !== \"number\" ||\n      typeof range.maximum !== \"number\"\n    ) {\n      return \"\";"),
        ("the painted range span", "      '<span class=\"prow-range type-hint muted\" data-role=\"row-range\">' +"),
        ("the painted lower bound", "      escapeHtml(rangeEndpointText(control, range.minimum)) +"),
        ("the separator painted between the bounds", "      escapeHtml(RANGE_SEPARATOR) +"),
        ("the painted upper bound", "      escapeHtml(rangeEndpointText(control, range.maximum)) +"),
        // The unit `check_ranges_and_units_are_rendered` reads back.
        ("the painted unit span", "    var unit = control.unit\n      ? '<span class=\"prow-unit type-hint muted\">' +\n        escapeHtml(String(control.unit)) +\n        \"</span>\"\n      : \"\";"),
        // The two helpers the walked functions call. Neither is markup and
        // both are rules the transcriptions copy, so F-52 applies to them
        // literally: `page_strip_group_key` writes `id.starts_with(...)`,
        // which copies `startsWith`'s *semantics* and not merely the prefix
        // literal, and `page_strip_groups`' `declared` closure copies
        // `designedGroup`'s lookup. Cycle 2 left both unpinned, and with
        // `startsWith` returning `false` — four of `stripGroupKey`'s six arms
        // dead and the page unable to group anything — `cargo test
        // --all-targets` passed in full (F-68).
        ("the prefix test", "    return text.lastIndexOf(prefix, 0) === 0;"),
        ("the declared group lookup, by key", "    for (var i = 0; i < DESIGNED_STRIP_GROUPS.length; i += 1) {\n      if (DESIGNED_STRIP_GROUPS[i].key === key) {\n        return DESIGNED_STRIP_GROUPS[i];\n      }\n    }\n    return null;"),
        // `stripGroupKey` — [`page_strip_group_key`], arm for arm. This is the
        // identity→group mapping T030's whole claim is about; cycle 1 pinned
        // none of it (F-55).
        ("the instrument group's identity", "    if (id === \"patch.engine\") {\n      return \"instrument\";"),
        ("the envelope group's prefix", "    if (startsWith(id, \"patch.envelope.\")) {\n      return \"envelope\";"),
        ("the capability group's prefix", "    if (startsWith(id, \"patch.capability.\")) {\n      return \"capability\";"),
        ("the slot group's prefix", "    if (startsWith(id, \"patch.effectSlot.\")) {\n      return \"slot.\" + id.slice(\"patch.effectSlot.\".length);"),
        ("the occupant joins its open slot", "    if (startsWith(id, \"patch.effect.\")) {\n      return openSlot;"),
        ("an identity no designed group claims has no key", "      return openSlot;\n    }\n    return null;"),
        // `groupHeadControlId` — [`page_group_head_control_id`]. The group titles
        // `projected_screen_strings` walks are built through this.
        ("the instrument and capability group head", "    if (key === \"instrument\" || key === \"capability\") {\n      return \"patch.engine\";"),
        ("the slot group head", "    if (startsWith(key, \"slot.\")) {\n      return \"patch.effectSlot.\" + key.slice(\"slot.\".length);"),
        ("no other group has a head row", "      return \"patch.effectSlot.\" + key.slice(\"slot.\".length);\n    }\n    return null;"),
        // `stripGroups` — [`page_strip_groups`], statement for statement.
        ("a group's legend and designed flag come from the declared table", "      if (!Object.prototype.hasOwnProperty.call(byKey, key)) {\n        var declared = designedGroup(key);\n        byKey[key] = {\n          key: key,\n          legend: declared ? declared.legend : null,\n          designed: declared ? declared.designed : false,\n          unknown: false,\n          rows: [],\n        };\n        groups.push(byKey[key]);\n      }\n      return byKey[key];"),
        ("the open slot starts closed", "    var openSlot = null;"),
        ("an invisible row is not arranged", "      if (!control.visible) {\n        continue;"),
        ("the open slot is carried from the occupancy row", "      var id = controlIdOf(control);\n      if (startsWith(id, \"patch.effectSlot.\")) {\n        openSlot = stripGroupKey(id, null);"),
        ("each row's group is resolved against the open slot", "      var key = stripGroupKey(id, openSlot);"),
        ("the unclaimed identity branch", "      if (key === null) {"),
        ("an unclaimed identity joins the explicit unknown group", "        var unknown = group(\"?group\");\n        unknown.unknown = true;\n        unknown.rows.push(control);\n        continue;"),
        ("a row joins its group in document order", "      group(key).rows.push(control);"),
        ("the declared-group re-insertion", "    for (var d = DESIGNED_STRIP_GROUPS.length - 1; d >= 0; d -= 1) {\n      var designed = DESIGNED_STRIP_GROUPS[d];\n      if (!designed.designed || byKey[designed.key]) {"),
        ("the re-inserted group's declared position", "      var at = groups.length;\n      for (var g = 0; g < groups.length; g += 1) {\n        var index = -1;\n        for (var e = 0; e < DESIGNED_STRIP_GROUPS.length; e += 1) {\n          if (DESIGNED_STRIP_GROUPS[e].key === groups[g].key) {\n            index = e;\n          }\n        }\n        if (index > d) {\n          at = g;\n          break;\n        }\n      }"),
        ("the re-inserted group carries no rows", "      groups.splice(at, 0, {\n        key: designed.key,\n        legend: designed.legend,\n        designed: true,\n        unknown: false,\n        rows: [],\n      });"),
        // `stripGroupHtml` — the group title `projected_screen_strings` reads and
        // the empty-group mark `check_an_absent_group_...` asserts.
        ("the group title is the authored legend or the head row's value", "    var title = group.unknown\n      ? \"?\" + group.key.slice(1)\n      : group.legend || (head ? controlValueText(head) : null);"),
        ("the empty-group mark", "    if (group.rows.length === 0) {\n      rows = markUnavailableRowHtml(String(group.legend || group.key));\n    }"),
        // `patchStripHtml` — the whole-strip unavailable rule and the head row
        // each group is titled by.
        ("the unavailable strip", "    var groups = stripGroups(controls);\n    var painted = 0;\n    for (var g = 0; g < groups.length; g += 1) {\n      painted += groups[g].rows.length;\n    }\n    if (painted === 0) {"),
        ("each group's head row is resolved through groupHeadControlId", "      body += stripGroupHtml(\n        groups[i],\n        mode,\n        \"listed\",\n        controlById(main, groupHeadControlId(groups[i].key))\n      );"),
        // `sideRegionHintLine`, `hintRun` and `hintLabel` — [`page_side_hint_line`].
        ("the hint line walks every projected action", "    var surfaces = model.surfaces || [];\n    for (var s = 0; s < surfaces.length; s += 1) {\n      var controls = surfaces[s].controls || [];\n      for (var c = 0; c < controls.length; c += 1) {\n        var valid = controls[c].validActions || [];"),
        ("the hint line's action selection", "          var kind = action.action && action.action.kind;\n          var entersThis =\n            kind === \"enterSurface\" &&\n            action.action.payload === (surface && surface.id);\n          var leavesThis = kind === \"return\" && surfaces[s].id === surface.id;\n          if (!entersThis && !leavesThis) {\n            continue;\n          }"),
        // Two anchors cover the pair being deduped. The whole-function census
        // below separately pins the escaped U+0000 separator, so an empty or
        // colliding separator cannot hide between these fragments and the
        // JavaScript source remains an ordinary text file.
        ("the hint line's dedup key", "          var key = String(action.hint) +"),
        ("the hint line's dedup", " + String(action.label);\n          if (seen[key]) {\n            continue;\n          }\n          seen[key] = true;\n          actions.push(action);"),
        ("no qualifying action, no hint line", "    if (actions.length === 0) {\n      return \"\";\n    }"),
        ("the panel's hint line is its hint run", "    return (\n      '<span class=\"panel-hint\" data-role=\"utility-hint\">' +\n      hintRun(actions) +\n      \"</span>\"\n    );"),
        ("a null hint never renders", "      if (!action.hint) {\n        continue;"),
        ("the hint pairs its label with a colon", "        '<span class=\"type-hint focus\">' +\n          escapeHtml(action.hint) +\n          \":\" +\n          escapeHtml(hintLabel(action)) +\n          \"</span>\""),
        ("the hint run's space join", "    return spans.join(\" \");"),
        ("the hint label's authored transform", "    return String(action.label)\n      .toLowerCase()\n      .replace(/^(open|move)\\s+/, \"\")\n      .replace(/\\s+mode$/, \"\");"),
        // The constants, tables and marks this file transcribes directly.
        ("the designed group table, in declared order", designed_strip_groups.as_str()),
        ("the unavailable mark", unavailable_mark.as_str()),
        ("the range separator", "var RANGE_SEPARATOR = \" — \""),
        ("the read-only mark", "var READ_ONLY_MARK = \"READ-ONLY\""),
        ("the read-only discriminator", "control.patchInteraction === \"readOnly\""),
    ];
    for (what, fragment) in required {
        match script.matches(fragment).count() {
            1 => {}
            0 => panic!(
                "webview-page/page.js no longer contains {what}: {fragment:?} — the \
                 transcription in this file is describing a page that no longer exists"
            ),
            elsewhere => panic!(
                "{what} is pinned to {fragment:?}, which occurs {elsewhere} times in \
                 webview-page/page.js — an anchor matching more than the one site it names \
                 is not a pin (F-42, F-53, F-55): the named site could be changed and this \
                 would stay satisfied"
            ),
        }
    }
    check_every_line_of_a_transcribed_page_rule_carries_a_pin(&script, &required);
    required.len()
}

/// Pins the production renderer rules the headless acceptance transcribes.
///
/// The live DOM twin proves the resulting structure and geometry. This check
/// keeps the deterministic headless proof coupled to the same path: projected
/// sections select controls by complete semantic path, summaries correlate by
/// that path, Utility follows projected order, and reflow only reveals focus.
fn check_the_transcribed_page_rules_match_the_committed_script() -> usize {
    let script = page_source("page.js");
    let required = [
        (
            "continuous selectors display projected names",
            "return String(control.selectedLabel ?? numericValueText(control, Number(parameter.value)));",
        ),
        (
            "named values omit raw numeric bounds",
            "      !range ||\n      control.selectedLabel != null ||",
        ),
        (
            "named values carry their complete display text",
            "  function controlUnitText(control) {\n    if (control && control.selectedLabel != null) {\n      return \"\";",
        ),
        (
            "complete-path control lookup",
            "    var wanted = JSON.stringify(path || null);",
        ),
        (
            "projected summary lookup",
            "    var summaries = (section && section.controlSummaries) || [];",
        ),
        (
            "projected section iteration",
            "    var sections = (main && main.sections) || [];",
        ),
        (
            "projected section path iteration",
            "      var paths = section.controlPaths || [];",
        ),
        (
            "section path resolution",
            "        var control = controlByPath(main, paths[pathIndex]);",
        ),
        (
            "section summary correlation",
            "          overviewSummaryForControl(section, control),",
        ),
        (
            "root Overview selection",
            "        : patchOverviewHtml(model);",
        ),
        (
            "projected Utility order",
            "  function patchUtilityHtml(model, surface, summary) {",
        ),
        (
            "semantic focus lookup after paint",
            "    var wanted = JSON.stringify(model.focusPath || null);",
        ),
        (
            "presentation-only focus reveal",
            "        rows[i].scrollIntoView({ block: \"nearest\", inline: \"nearest\" });",
        ),
        (
            "requested option label",
            "            selectedLabel: control.requestedLabel,",
        ),
        (
            "continuous values and ranges share unit-aware presentation",
            "    return control.kind === \"continuous\"\n      ? numericValueText(control, Number(value))\n      : String(value);",
        ),
        (
            "read-only discriminator",
            "control.patchInteraction === \"readOnly\"",
        ),
    ];
    for (what, fragment) in required {
        assert!(
            script.contains(fragment),
            "webview-page/page.js no longer contains {what}: {fragment:?}"
        );
    }
    required.len()
}

/// One `page.js` function's body, from the committed source.
///
/// Both anchors are asserted, because the walk below is only as honest as the
/// slice it is handed and neither anchor is self-evidently safe. Both
/// assertions are text scans. Each was first written to recognise exactly one
/// spelling of what it forbids, and each now recognises a class of spellings
/// instead — but a class, not the property, and the difference is named on the
/// walk below rather than rounded off here.
///
/// The **head** must be declared exactly once. JavaScript lets a later
/// declaration of a name override an earlier one, so a second `controlIdOf`
/// declared under the first is the function the page actually calls while this
/// walk goes on reading the original — every control id becomes `""` and
/// nothing here notices. Counting occurrences of the literal
/// `"\n  function NAME("` counted one spelling of that. Three others were
/// hoisted just the same, were the declaration the page called, and were
/// measured MISSED: `function controlIdOf (control)` with a space before the
/// paren, `function  controlIdOf(control)` with two after the keyword, and the
/// same declaration written at file scope. The first of those is the one that
/// decides it — the page's own `stripGroups`, run against a five-row fixture
/// under it, folds every row into `?group` and leaves every designed group
/// empty, which is T030's whole claim. The same trick on `stripGroupKey` was
/// MISSED too, so this is the anchor and not one function. The count is over shape
/// — `function`, any whitespace, this exact name, any whitespace, `(` — and all
/// four fail. Locating the body still needs the literal, so the sole declaration
/// respelled fails at the `find` below rather than being read from the wrong
/// offset.
///
/// The **end** is the first two-space-indented `}`, which is this function's own
/// closer only while no inner brace sits at that column. Dedenting one is
/// whitespace, so the page behaves identically, but the slice stops there and
/// every line after it goes unread — a rule inserted past the cut was measured
/// MISSED. A truncated slice leaves at least one inner brace open, so requiring
/// exactly one unclosed brace — the function's own — detects the cut. That count
/// was naive over the raw slice, and a comment is the one kind of text the walk
/// below deletes before any table sees it: a `}` appended to a comment four
/// lines above a cut rebalanced the count, `controlValueText` then walked 14
/// lines instead of 56, and every parameter row painted the unavailable mark —
/// MISSED. The count now runs over the same comment-stripped text the walk
/// reads. It is still naive about a brace inside a string or a regex literal; no
/// walked body holds one today, and if one arrives this fires rather than going
/// quiet. Over-extension, the other way the anchor could slip, is caught
/// downstream — the swallowed function's own declaration line is not a line this
/// function may admit.
fn page_function_body<'a>(script: &'a str, name: &str) -> &'a str {
    // Counted by shape rather than by spelling: `function`, any whitespace, this
    // exact name, any whitespace, `(`. `strip_prefix` then requiring `(` is what
    // keeps `startsWith` from being counted as `startsWithPrefix` would be.
    let declarations = script
        .match_indices("function")
        .filter(|(at, _)| {
            script[at + "function".len()..]
                .trim_start()
                .strip_prefix(name)
                .is_some_and(|rest| rest.trim_start().starts_with('('))
        })
        .count();
    assert_eq!(
        declarations, 1,
        "webview-page/page.js declares {name} {declarations} times and exactly one is \
         required: a later declaration overrides an earlier one, so a duplicate is the \
         function the page calls while this walk reads the first"
    );
    let head = format!("\n  function {name}(");
    let start = script.find(&head).unwrap_or_else(|| {
        panic!(
            "webview-page/page.js declares {name}, but not as {head:?} — this walk \
             locates a body by that literal, so it would read the wrong bytes or none"
        )
    }) + 1;
    let body = &script[start..];
    let end = body
        .find("\n  }\n")
        .unwrap_or_else(|| panic!("{name} closes at file scope"));
    let body = &body[..end];
    // Counted over the text the walk below actually reads. That walk drops
    // everything from the first ` //`, so a `}` typed into a comment reaches no
    // table and yet rebalanced a naive count — enough to hide a slice already
    // truncated by a dedented brace. Measured MISSED before this strip.
    let code_only = body
        .split('\n')
        .map(|line| line.split(" //").next().unwrap_or(line))
        .collect::<String>();
    assert_eq!(
        code_only.matches('{').count(),
        code_only.matches('}').count() + 1,
        "the walk of {name} stops at the first two-space `}}`, and that is not this \
         function's own closer — an inner brace sits at that column, so every line \
         after it goes unwalked"
    );
    body
}

/// **Every statement of a wholly-transcribed function carries a pin.**
///
/// The table above proves every pin still matches. It cannot prove there *is* a
/// pin for every rule — which is precisely what cycle 1 lacked (F-55), and what
/// reading the transcription against the page does not reveal either (F-56).
/// The only thing that reveals it is walking the source.
///
/// So this walks it, mechanically: every line of every `page.js` function this
/// file transcribes **whole** must either sit inside a pinned fragment as a
/// whole line, or be named below as scaffolding that carries no rule. A rule
/// added to any of these functions without a pin fails here, and the only way
/// past is to write the new line into a list a reviewer reads — which is the
/// difference between an omission and a decision.
///
/// **What this covers, stated as it behaves.** The check is over the *set* of
/// statements each of twelve `page.js` bodies contains, as the extraction hands
/// them over. Four tables admit a line, and every one of them is keyed on the
/// function being walked and matched against the whole line, indentation
/// included: that function's own pins (145 lines), its own declarations (13),
/// its own scaffolding (15), and — in `sideRegionHintLine` alone — F-43's NUL
/// separator (1). Those 174 are what `checked` counts. Nothing else admits
/// anything, except three text filters that run ahead of the tables: a blank
/// line (22), a line beginning `//` (**0** — the inline-comment strip above
/// empties a whole-line comment before this filter ever sees it, so the filter
/// is unreachable rather than merely harmless), and a line spelled entirely
/// from `{}()[];,` (47). None of the three can carry a rule; the third is
/// decided character by character rather than by shape, and a punctuation line
/// *moved* changes nesting without changing the set — which is the order residue
/// below, not a fourth way in. Those seven dispositions are the loop's complete
/// control flow, and that is measured rather than reasoned: instrumenting the
/// walk to print one tagged line per body line accounts for all 243 lines it
/// touches, twice, with no eighth path. So a statement added to a line the walk
/// is handed fails here.
///
/// A pinned statement *changed* fails too, but in the pin table this walk is
/// handed and not in this walk — that table is what `page_rules_pinned` counts,
/// and it is where P3, P3b, P4, P5 and P15 panic.
///
/// **What the walk is handed is bounded by two anchors, and they are
/// heuristics.** `page_function_body` asserts that the function is declared
/// exactly once and that the slice holds exactly one unclosed brace. Both were
/// defeatable, so both are checked rather than assumed — and each check was
/// first written to recognise one spelling of what it forbids, which is how the
/// slice arrived here truncated or read from the wrong declaration while every
/// list above stayed satisfied. Each now recognises a class instead: the
/// declaration count is over shape rather than over the literal
/// `"\n  function NAME("`, so a second declaration spelled with any whitespace
/// between `function`, the name and `(`, at any indentation, is counted; and
/// the brace count runs over the comment-stripped body, the same text this loop
/// reads, so a `}` typed into a comment can no longer rebalance a truncated
/// slice. Five mutations that were measured MISSED are CAUGHT: four second
/// declarations, across two different walked functions, and the comment brace.
///
/// They are text scans and not a parser, and what that leaves is not a smaller
/// version of the same hunt but one more instance of the residue below.
/// `function controlIdOf/*x*/(control)` — a comment between the name and the
/// paren — is valid, is hoisted, is the declaration the page calls, and is
/// counted by nothing here. Measured MISSED. It is named rather than fixed:
/// `str::find` over a literal always admits a further spelling, so the next scan
/// would have a next spelling, and closing the class means a parser.
///
/// Every one of these was flat once, and every flat one was reachable — the
/// tables carry their own histories. What is worth saying in one place is that
/// the shape barely varied: five times an admission pool matched more than the
/// one site it names, which is the exact defect the pin table above exists to
/// reject, turned inward on the check built to enforce it. The two anchors were
/// its mirror image — an assertion recognising fewer spellings than the property
/// it named — which is the same error read from the other side.
///
/// What it does not cover is three things, and they are named rather than
/// implied away.
///
/// **Not covered — the set's order, and its multiplicity.** Both this check and
/// the pin table are set-membership tests over line text, so neither can see a
/// change that leaves the set of lines identical. Two mutations reach through
/// that, and each collapses grouping in one line:
///
/// - *Order.* Moving `stripGroups`' `if (!control.visible) { continue; }` block
///   to the end of the loop body leaves every pinned fragment contiguous and
///   intact and every statement present, so the page arranges invisible rows —
///   cycle 1's mutation #3, reached by reordering rather than by deleting.
/// - *Multiplicity.* Inserting `    return null;` at the top of `stripGroupKey`,
///   which already **ends** with that line, is a duplicate rather than an
///   addition. The set is unchanged, so nothing fails, and every identity now
///   maps to no group at all.
///
/// Both are inherent to a set-based check and neither is closable by adding
/// pins: closing them means requiring each body to be a *sequence* of pins and
/// scaffolding, a different and much larger control than this one. Recorded as
/// known gaps (F-68, F-74) rather than treated as pending.
///
/// **Not covered — anything that is not a line of one of the twelve slices.**
/// This is one *class*, not a list of items: it absorbs new instances without
/// changing shape, and it has three measured ones. Each is MISSED here, and
/// nothing else in the tree catches it either: the full `--all-targets` sweep
/// under the third below left every target green but F-78's flaky
/// `input_capture_witness`, which this same file passed in the clean run.
///
/// - The **head-row resolution** is not pinned. `patchStripHtml`'s *call site*
///   `controlById(main, groupHeadControlId(groups[i].key))` is pinned, and
///   `groupHeadControlId` is transcribed whole; `controlById`'s own identity
///   match is neither, because this file does not transcribe it — it asserts the
///   head row through `group.rows[0]`. Making `controlById` match every control
///   is invisible here.
/// - A **rebinding** rather than a declaration.
///   `controlIdOf = function (control) { return ""; };` written below the
///   original runs at load, defeats the function completely, and is a line of no
///   walked body.
/// - A **second declaration the head scan's shape does not describe** — a
///   comment between the name and the paren, as above. Three other spellings of
///   a second declaration were in this class until the scan stopped counting a
///   literal; this one remains.
///
/// Closing the class means walking the file with a parser rather than twelve
/// `str::find` slices — a different and much larger control. That is why it is
/// described here instead of narrowed again: each narrowing of a literal scan
/// buys the next spelling, not the property.
///
/// **Not covered, and never was — that the Rust computes what the page
/// computes.** These pins bound the cost of the transcription drifting from the
/// page's *text*; they say nothing about the two agreeing on a result. Only
/// F-44's `stripGroupsPainted`, read back from the page's own output, closes
/// that, and this control should shrink when it lands.
///
/// One mechanical note, not a limit: functions transcribed only in part —
/// `stripGroupHtml`'s title and empty-group mark, `patchStripHtml`'s
/// unavailable rule and head-row lookup, the lifecycle band's requested value,
/// the row's unit span — are pinned by hand instead of walked, because
/// requiring whole-body coverage there would demand pins for markup this file
/// does not copy, and a pin with no copied rule behind it is noise (F-65's
/// `HINT_SEPARATOR`).
fn check_every_line_of_a_transcribed_page_rule_carries_a_pin(
    script: &str,
    pins: &[(&str, &str)],
) -> usize {
    /// `page.js` function → the transcription in this file that copies it whole.
    const TRANSCRIBED_WHOLE: [(&str, &str); 12] = [
        ("startsWith", "page_strip_group_key"),
        ("designedGroup", "page_strip_groups"),
        ("controlIdOf", "page_control_id"),
        ("controlValueText", "page_value_text"),
        ("rangeEndpointText", "page_range_text"),
        ("rangeHtml", "page_range_text"),
        ("stripGroupKey", "page_strip_group_key"),
        ("groupHeadControlId", "page_group_head_control_id"),
        ("stripGroups", "page_strip_groups"),
        ("hintLabel", "page_side_hint_line"),
        ("hintRun", "page_side_hint_line"),
        ("sideRegionHintLine", "page_side_hint_line"),
    ];
    /// The declaration lines the walk crosses: the twelve heads above, and
    /// `stripGroups`' nested `group` helper. A declaration is not a statement,
    /// but it is not ruleless either — its parameter list, and the order of it,
    /// is the calling convention every line beneath it reads. Skipping whatever
    /// merely *starts with* `function ` admitted all thirteen at every site, and
    /// two mutations walked straight through: swapping
    /// `rangeEndpointText(control, value)` to `(value, control)` paints
    /// `[object Object]` for both bounds of every numeric range, and dropping the
    /// nested helper's `key` parameter rebinds it to `stripGroups`' own loop
    /// variable so the explicit unknown group loses its `?group` identity. Both
    /// measured MISSED under the prefix skip and CAUGHT under this table.
    ///
    /// Asserting every entry is reached is also what proves each of the twelve
    /// bodies was walked at all: a body the extraction returned empty would take
    /// its declaration with it.
    const DECLARATIONS: [(&str, &str); 13] = [
        ("startsWith", "  function startsWith(text, prefix) {"),
        ("designedGroup", "  function designedGroup(key) {"),
        ("controlIdOf", "  function controlIdOf(control) {"),
        ("controlValueText", "  function controlValueText(control) {"),
        (
            "rangeEndpointText",
            "  function rangeEndpointText(control, value) {",
        ),
        ("rangeHtml", "  function rangeHtml(control) {"),
        ("stripGroupKey", "  function stripGroupKey(id, openSlot) {"),
        ("groupHeadControlId", "  function groupHeadControlId(key) {"),
        ("stripGroups", "  function stripGroups(controls) {"),
        ("stripGroups", "    function group(key) {"),
        ("hintLabel", "  function hintLabel(action) {"),
        ("hintRun", "  function hintRun(actions) {"),
        (
            "sideRegionHintLine",
            "  function sideRegionHintLine(model, surface) {",
        ),
    ];
    /// Lines that carry no rule **in the function that holds them**:
    /// accumulators, cursors, loop headers, and the bare openers and closing
    /// literals of a concatenated markup expression.
    /// Each is here because the transcription's own loop is not a copy of *this*
    /// loop — it walks Rust values — so there is nothing to pin. Every pair is
    /// asserted below to still occur, so this list cannot rot into a permit.
    ///
    /// **Owner and indentation are both part of the entry, and both were
    /// load-bearing.** Matched on the bare text, this list admitted any of its
    /// fifteen lines in any of the twelve walked functions: `    return groups;`
    /// inserted at the top of `controlValueText`, `controlIdOf` or
    /// `stripGroupKey` was MISSED, and `groups` is local to `stripGroups`, so the
    /// first of those throws on every parameter row and the page cannot render —
    /// with all thirty targets green. Matched on the trimmed statement, the same
    /// entry was still admitted at any depth *within* its owner:
    /// `      return groups;` moved inside `stripGroups`' loop returns after the
    /// first control, so no row is ever arranged, and that was MISSED too. The
    /// pins beside it have always matched whole lines; these now do the same.
    ///
    /// The last two are `rangeHtml`'s, and they were **hidden** until the pin
    /// pool was scoped per function: the flat pool admitted them from
    /// `sideRegionHintLine`'s *the panel's hint line is its hint run*, which
    /// opens and closes the same way. Every line of `rangeHtml` that carries a
    /// rule — the absent-range guard, the `data-role="row-range"` span, both
    /// bounds and the separator between them — is pinned individually above; a
    /// pin on the return opener or the closing tag would be a pin with no
    /// copied rule behind it, which is F-65's `HINT_SEPARATOR`.
    const SCAFFOLDING: [(&str, &str); 15] = [
        ("stripGroups", "    var groups = [];"),
        ("stripGroups", "    var byKey = {};"),
        ("hintRun", "    var spans = [];"),
        ("sideRegionHintLine", "    var actions = [];"),
        ("sideRegionHintLine", "    var seen = {};"),
        ("stripGroups", "      var control = controls[i];"),
        ("sideRegionHintLine", "          var action = valid[a];"),
        ("hintRun", "      var action = actions[i];"),
        ("hintRun", "      spans.push("),
        ("stripGroups", "    return groups;"),
        (
            "stripGroups",
            "    for (var i = 0; i < controls.length; i += 1) {",
        ),
        (
            "hintRun",
            "    for (var i = 0; i < actions.length; i += 1) {",
        ),
        (
            "sideRegionHintLine",
            "        for (var a = 0; a < valid.length; a += 1) {",
        ),
        ("rangeHtml", "    return ("),
        ("rangeHtml", "      \"</span>\""),
    ];
    /// F-43's NUL separator is escaped in the JavaScript source so the file
    /// remains text while the runtime key still contains U+0000. The line is
    /// admitted here by its owner and its whole text, not by the prefix the two
    /// pins stop at. Under the prefix, everything after
    /// `String(action.hint) +` was unchecked: emptying the separator to `+ "" +`
    /// was MISSED, and `hint="AB", label="C"` then keys the same as
    /// `hint="A", label="BC"`, silently dropping a hint from the utility line —
    /// which is the collision the NUL is there to prevent. Unowned, the line was
    /// also admitted inside `controlIdOf`, where `action` is undefined and every
    /// row throws.
    const NUL_SEPARATOR_LINE: (&str, &str) = (
        "sideRegionHintLine",
        r#"          var key = String(action.hint) + "\u0000" + String(action.label);"#,
    );

    let mut used = BTreeSet::new();
    let mut declared = BTreeSet::new();
    let mut nul_separator_seen = false;
    let mut checked = 0_usize;
    for (page_function, twin) in TRANSCRIBED_WHOLE {
        let body = page_function_body(script, page_function);
        // Only pins that live in *this* function may admit its lines. Matching
        // against a flat pool of every pin in the file admits a line by its
        // text alone, so a statement genuinely *added* to a walked function
        // passes whenever that same text happens to occur inside some unrelated
        // function's pin: `    return null;` at the top of `stripGroups` — which
        // arranges no groups at all, T030's whole claim — was admitted by
        // `designedGroup`'s lookup pin, with `cargo test --all-targets` green
        // across every target. So were the same line atop `controlValueText`
        // and `controlIdOf`, neither of which contains it, and `designedGroup`'s
        // whole lookup body pasted into `stripGroups`. That is an anchor
        // matching more than the one site it names — what the table above
        // asserts against — turned inward on the coverage predicate (F-74).
        let own_pins = pins
            .iter()
            .filter(|(_, fragment)| body.contains(*fragment))
            .collect::<Vec<_>>();
        for line in body.split('\n') {
            // An inline comment is not a rule. This drops everything from the
            // first ` //`, which is a transform and not an admission, so it is
            // the one place the walk can lose text without noticing: it would,
            // if a ` //` ever sat inside a string or a regex literal. Twenty
            // lines across the twelve bodies contain one and every one of them
            // is a comment — checked by scanning each line for an unclosed
            // quote ahead of the `//`, not assumed. A truncated line that is not
            // a comment fails closed anyway: the prefix left behind matches no
            // pin.
            let code = line.split(" //").next().unwrap_or(line).trim_end();
            let statement = code.trim();
            if statement.is_empty()
                || statement.starts_with("//")
                // A closing brace, a bare `);` or a lone `,` is punctuation and
                // not a statement: there is no rule in it to pin and none to
                // defeat. Unscoped, these were being admitted by whichever pin
                // elsewhere in the file happened to close at the same
                // indentation, which is why scoping surfaces them at all.
                || statement.chars().all(|c| "{}()[];,".contains(c))
            {
                continue;
            }
            checked += 1;
            // Whole lines, not substrings. `code` keeps its indentation, so a
            // deeper-indented pinned line *contains* the same statement at
            // shallower indentation: with `contains`, `    return openSlot;`
            // inserted at the top of `stripGroupKey` was admitted by *an
            // identity no designed group claims has no key*, and
            // `    return UNAVAILABLE_MARK;` at the top of `controlValueText`
            // by *the malformed parameter mark* — two pins with nothing to do
            // with the lines they were admitting, and two mutations that
            // collapse the rules those pins defend. That is F-42/F-53/F-55's
            // own mechanism — a predicate matching more than the one thing it
            // names — turned inward on the check built to catch it (F-68).
            // Whole lines *narrowed* that admission surface from a substring of
            // any pin to an exact line of any pin; it took the scoping above to
            // close it, to an exact line of this function's own pins (F-74).
            if own_pins
                .iter()
                .any(|(_, fragment)| fragment.lines().any(|pinned| pinned == code))
            {
                continue;
            }
            // A declaration line and the NUL separator are admitted the same
            // way a pin is: by this function's own entry, matched whole. Neither
            // is a `starts_with`, because both were reachable as one — see the
            // two tables above.
            if DECLARATIONS
                .iter()
                .any(|(owner, declaration)| *owner == page_function && *declaration == code)
            {
                declared.insert((page_function, code));
                continue;
            }
            if (page_function, code) == NUL_SEPARATOR_LINE {
                nul_separator_seen = true;
                continue;
            }
            assert!(
                SCAFFOLDING
                    .iter()
                    .any(|(owner, scaffold)| *owner == page_function && *scaffold == code),
                "webview-page/page.js {page_function} line {code:?} is transcribed by \
                 {twin} and no pin covers it — pin the statement, or name it as scaffolding \
                 that carries no rule (F-55: an unpinned copied rule is what made the \
                 grouping claim provable against nothing)"
            );
            used.insert((page_function, code));
        }
    }
    assert_eq!(
        used.len(),
        SCAFFOLDING.len(),
        "the scaffolding list names {} (function, line) pairs webview-page/page.js no \
         longer has: {:?}",
        SCAFFOLDING.len() - used.len(),
        SCAFFOLDING
            .iter()
            .filter(|pair| !used.contains(*pair))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        declared.len(),
        DECLARATIONS.len(),
        "the declaration list names {} (function, line) pairs webview-page/page.js no \
         longer has: {:?}",
        DECLARATIONS.len() - declared.len(),
        DECLARATIONS
            .iter()
            .filter(|pair| !declared.contains(*pair))
            .collect::<Vec<_>>()
    );
    assert!(
        nul_separator_seen,
        "sideRegionHintLine no longer keys its dedup on the NUL-separated hint and \
         label — F-43's rule is gone, and the two pins either side of it cannot see \
         that on their own"
    );
    checked
}

// ---------------------------------------------------------------------------
// T029 — on-screen patch selection, refusal at the ends, focus recovery
// ---------------------------------------------------------------------------

/// The fixture composition itself, asserted rather than assumed.
///
/// T029's whole claim rests on the schemas actually disagreeing. A fixture that
/// quietly became two Patches of one capability would make every switch
/// assertion below pass for the wrong reason.
fn check_the_fixture_spans_more_than_two_patches_across_both_engines() {
    let state = fixture_state();
    assert!(
        state.patches().len() > 2,
        "a fixture of {} Patches cannot falsify a switch",
        state.patches().len()
    );
    let capabilities = state
        .patches()
        .iter()
        .map(|patch| {
            patch
                .instrument_config()
                .capability_id()
                .as_str()
                .to_owned()
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        capabilities,
        BTreeSet::from([
            BRAIDS_CAPABILITY_ID.to_owned(),
            HIDEF_CAPABILITY_ID.to_owned()
        ]),
        "the fixture must install both engines"
    );
    // And the two schemas must genuinely differ, or "recovers against the
    // destination's own schema" is unfalsifiable here.
    let resolver = SemanticResolver::new(&state);
    let rows = |id: u32| {
        let patch_id = PatchId::new(id).unwrap();
        let patch = patch_of(&state, patch_id);
        let subject =
            PatchDetailSubject::instrument(patch.instrument_config().capability_id().clone());
        resolver
            .patch_detail_paths(patch_id, &subject)
            .unwrap()
            .into_iter()
            .map(|path| path.control_id().clone())
            .collect::<Vec<_>>()
    };
    let soundfont = rows(1);
    let braids = rows(2);
    assert_ne!(
        soundfont.len(),
        braids.len(),
        "the fixture's two engines must declare different row counts"
    );
    assert!(
        soundfont
            .iter()
            .chain(&braids)
            .any(
                |control| (soundfont.contains(control) != braids.contains(control))
                    && matches!(
                        control,
                        SemanticControlId::Patch(PatchControlId::Capability(_))
                    )
            ),
        "the two detail schemas must differ by a capability-owned control"
    );
}

/// A switch reprojects **the destination's own** identity, channel, engine,
/// envelope, capability rows, effect slots, and every Utility value — in
/// exactly one advanced generation, with no projection pairing one Patch's
/// identity with another's schema.
///
/// Falsified by defeating `select_patch`'s reprojection: with the reducer's
/// `set_active_main` left on the source Patch, the destination's values never
/// arrive and the `patchId` agreement check fails.
fn check_a_switch_reprojects_the_destination_in_one_generation() -> usize {
    let mut state = fixture_state();
    let mut switches = 0_usize;
    // A settled projection of every Patch, read before any switch, so the
    // comparison is against what that Patch *is*, not against what the switch
    // produced.
    let expected = |id: u32| {
        let mut isolated = fixture_state();
        for _ in 0..(id - 1) {
            isolated
                .apply(AppEvent::SelectPatch(Direction::Right))
                .unwrap();
        }
        document(&isolated)
    };
    for target in 2..=4_u32 {
        let before = semantic(&state).generation();
        state
            .apply(AppEvent::SelectPatch(Direction::Right))
            .expect("the installed order is long enough");
        switches += 1;
        let after = semantic(&state);
        assert_eq!(
            after.generation() - before,
            1,
            "one accepted switch must advance exactly one generation, not {}",
            after.generation() - before
        );

        let patch = patch_of(&state, PatchId::new(target).unwrap());
        assert_eq!(state.interaction().patch_focus(), Some(patch.id()));

        // Identity, channel, engine, envelope, capability rows, effect slots
        // and every Utility value, compared against that Patch's own settled
        // projection rather than against a hand-written expectation.
        let observed = document(&state);
        let reference = expected(target);
        for surface in ["patchMain", "patchUtility"] {
            let observed_rows: Vec<(String, String)> = surface_controls(&observed, surface)
                .iter()
                .map(|control| (page_control_id(control), page_value_text(control)))
                .collect();
            let reference_rows: Vec<(String, String)> = surface_controls(&reference, surface)
                .iter()
                .map(|control| (page_control_id(control), page_value_text(control)))
                .collect();
            assert_eq!(
                observed_rows, reference_rows,
                "switching to Patch {target} must reproject {surface} from that Patch"
            );
        }
        assert_eq!(
            observed.pointer("/surfaces/0/summary/patchName"),
            Some(&Value::String(patch.name().to_owned())),
            "the strip header names the destination Patch"
        );

        // No projection pairs one Patch's identity with another's schema.
        let identities = patch_identities(&observed);
        assert_eq!(
            identities,
            BTreeSet::from([u64::from(patch.id().value())]),
            "one projected document named more than one Patch: {identities:?}"
        );
    }
    assert_eq!(switches, 3, "the fixture must exercise three switches");
    switches
}

/// Every `patchId` a serialized document carries, anywhere in the tree.
fn patch_identities(document: &Value) -> BTreeSet<u64> {
    fn walk(value: &Value, found: &mut BTreeSet<u64>) {
        match value {
            Value::Object(map) => {
                for (key, child) in map {
                    if key == "patchId" {
                        if let Some(id) = child.as_u64() {
                            found.insert(id);
                        }
                    }
                    walk(child, found);
                }
            }
            Value::Array(items) => items.iter().for_each(|item| walk(item, found)),
            _ => {}
        }
    }
    let mut found = BTreeSet::new();
    walk(document, &mut found);
    found
}

/// A Patch switch preserves the stable root identity when the destination
/// hosts it, while rebinding the complete path to the destination Patch.
fn check_focus_recovers_against_the_destination_schema() {
    let mut state = fixture_state();
    let stable_control =
        SemanticControlId::Patch(PatchControlId::EffectSlot(EffectSlotIndex::ALL[1]));
    navigate_to(&mut state, |path| path.control_id() == &stable_control);
    let source = state.interaction().focus_path().clone();

    state
        .apply(AppEvent::SelectPatch(Direction::Right))
        .expect("the second Patch exists");

    let destination = PatchId::new(2).unwrap();
    let hosted = SemanticResolver::new(&state)
        .patch_main_paths(destination)
        .unwrap();
    let recovered = state.interaction().focus_path().clone();
    assert!(
        hosted.contains(&recovered),
        "focus recovered to {recovered:?}, which the destination does not host"
    );
    assert_eq!(
        recovered.control_id(),
        &stable_control,
        "the stable root identity must survive the Patch switch"
    );
    assert_ne!(recovered.patch_id(), source.patch_id());
    assert_eq!(recovered.patch_id(), Some(destination));
    // And the projection agrees with the reducer.
    assert_eq!(
        semantic(&state)
            .focused_control()
            .expect("the recovered row is projected")
            .path(),
        &recovered
    );
}

/// A request beyond either outer endpoint is a typed unchanged rejection. The
/// trailing empty position is the authored endpoint after the installed order.
/// Whole-state comparison proves a refusal rather than merely observing an
/// error.
fn check_the_ends_of_the_installed_order_refuse() {
    let mut state = fixture_state();
    let before = state.clone();
    assert_eq!(
        state.apply(AppEvent::SelectPatch(Direction::Left)),
        Err(EventRejection::ParameterAtBoundary),
        "the first position must refuse rather than wrap"
    );
    assert_eq!(state, before, "a refused switch leaves the state identical");
    assert_eq!(
        document(&state),
        document(&before),
        "a refused switch leaves the projection identical"
    );

    for _ in 0..3 {
        state
            .apply(AppEvent::SelectPatch(Direction::Right))
            .unwrap();
    }
    state
        .apply(AppEvent::SelectPatch(Direction::Right))
        .expect("the final created Patch advances to the trailing empty endpoint");
    assert_eq!(
        state.interaction().patch_position_focus(),
        Some(crest_synth::control::PatchPositionId::TrailingEmpty)
    );
    let at_end = state.clone();
    assert_eq!(
        state.apply(AppEvent::SelectPatch(Direction::Right)),
        Err(EventRejection::ParameterAtBoundary),
        "the trailing empty endpoint must refuse rather than wrap"
    );
    assert_eq!(state, at_end, "a refused switch leaves the state identical");
    assert_eq!(document(&state), document(&at_end));
}

/// An in-flight structural edit stays correlated to the Patch it was started
/// on, and an open subordinate surface is left rather than carried.
fn check_an_in_flight_edit_stays_correlated_and_a_subordinate_surface_is_left() {
    let (mut state, _) = preset_swap_in_flight();
    let origin = state.interaction().patch_focus().unwrap();
    assert_eq!(
        state.engine_selection().correlation().unwrap().patch_id(),
        Some(origin)
    );
    assert_eq!(state.interaction().active_surface(), SurfaceId::PatchDetail);
    assert!(state.interaction().detail_subject().is_some());

    let switched = state.apply(AppEvent::SelectPatch(Direction::Right));
    if switched.is_ok() {
        assert_eq!(
            state.engine_selection().correlation().unwrap().patch_id(),
            Some(origin),
            "the in-flight edit must stay correlated to the Patch it started on"
        );
        assert_eq!(
            state.interaction().detail_subject(),
            None,
            "an open subordinate surface is left before the switch, not carried"
        );
        assert_eq!(state.interaction().active_surface(), SurfaceId::PatchMain);
        assert!(state.interaction().detail_invariant_holds());
    } else {
        // A reducer that refuses the switch outright also honours the claim,
        // but it must do so as a typed unchanged rejection.
        let before = state.clone();
        assert_eq!(
            state.apply(AppEvent::SelectPatch(Direction::Right)),
            switched,
            "the refusal must be stable"
        );
        assert_eq!(state, before);
        assert_eq!(
            state.engine_selection().correlation().unwrap().patch_id(),
            Some(origin)
        );
    }
}

// ---------------------------------------------------------------------------
// T030 — the strip is grouped structure, not a flat control run
// ---------------------------------------------------------------------------

/// The grouped shape, as a predicate that can say **no**.
///
/// Returning `Result` rather than asserting is deliberate: the same predicate
/// has to be run against a flat shape and observed to reject it, and an
/// assertion that can only panic cannot be used as evidence of its own
/// discrimination.
fn grouped_strip_shape(controls: &[Value]) -> Result<Vec<StripGroup>, String> {
    let groups = page_strip_groups(controls);
    if groups.len() < 2 {
        return Err(format!(
            "the workspace arranged {} group(s): a flat run of every projected \
             control is not the PatchStrip composition",
            groups.len()
        ));
    }
    if let Some(unknown) = groups.iter().find(|group| group.unknown) {
        return Err(format!(
            "{} control(s) joined no designed group and were arranged under the \
             explicit unknown marker",
            unknown.rows.len()
        ));
    }
    // The declared groups appear in declared order.
    let declared_positions: Vec<usize> = groups
        .iter()
        .map(|group| {
            DESIGNED_STRIP_GROUPS
                .iter()
                .position(|(key, ..)| *key == group.key)
                .ok_or_else(|| format!("{} is not a declared strip group", group.key))
        })
        .collect::<Result<_, _>>()?;
    if declared_positions.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(format!(
            "the strip arranged its declared groups out of declared order: {:?}",
            groups.iter().map(|group| &group.key).collect::<Vec<_>>()
        ));
    }
    // Every designed group is present, so one with no view data can mark
    // itself rather than vanish.
    for (key, _, designed) in DESIGNED_STRIP_GROUPS {
        if designed && !groups.iter().any(|group| group.key == key) {
            return Err(format!("the designed group {key} vanished"));
        }
    }
    // Each group holds the rows its own identity claims, and a slot's occupant
    // rows are nested under that slot rather than run flat beside it.
    for group in &groups {
        for row in &group.rows {
            let id = page_control_id(row);
            // The `patch.effect.` arm is inert by construction: `expected` is
            // the group's own key, so the comparison below can never fire for
            // an occupant row. It is written this way because an occupant's
            // group depends on the open slot rather than on its identity. The
            // nesting claim is carried by the `rows[0]` occupancy check just
            // below and by the prefix walk in `check_the_strip_is_grouped_
            // structure`, not by this branch.
            let expected = if id.starts_with("patch.effect.") {
                group.key.clone()
            } else {
                page_strip_group_key(&id, None).unwrap_or_default()
            };
            if expected != group.key {
                return Err(format!("{id} was arranged into {}", group.key));
            }
        }
        if let Some(index) = group.key.strip_prefix("slot.") {
            let occupancy = format!("patch.effectSlot.{index}");
            if !group.rows.is_empty() && page_control_id(&group.rows[0]) != occupancy {
                return Err(format!(
                    "{} is headed by {} rather than its own occupancy row",
                    group.key,
                    page_control_id(&group.rows[0])
                ));
            }
        }
    }
    Ok(groups)
}

/// The PATCH workspace is the `PatchStrip` composition: an identity-and-routing
/// header, then ordered groups — instrument selector, envelope group, and one
/// group per ordered effect slot with its occupant rows nested.
#[allow(dead_code)]
fn check_the_strip_is_grouped_structure() -> usize {
    let state = fixture_state();
    let document = document(&state);
    let controls = surface_controls(&document, "patchMain");
    let groups = grouped_strip_shape(&controls)
        .unwrap_or_else(|error| panic!("the production PATCH workspace is not grouped: {error}"));

    let keys: Vec<&str> = groups.iter().map(|group| group.key.as_str()).collect();
    assert_eq!(
        keys,
        vec![
            "instrument",
            "envelope",
            "capability",
            "slot.0",
            "slot.1",
            "slot.2"
        ],
        "the fixture's strip arranges the declared groups in declared order"
    );
    // The instrument selector is headed by the engine row, and the envelope
    // group holds exactly the four declared envelope rows.
    assert_eq!(
        groups[0].rows.len(),
        1,
        "the instrument selector holds the engine row"
    );
    assert_eq!(page_control_id(&groups[0].rows[0]), "patch.engine");
    assert_eq!(
        groups[1].rows.len(),
        crest_synth::synth::VoiceEnvelope::surface_descriptor().len(),
        "the envelope group holds every declared envelope row"
    );
    // Each slot group is headed by its own occupancy row and nests that slot's
    // occupant rows — the two occupied positions hold two rows each, the empty
    // one holds only its occupancy row.
    for (index, group) in groups.iter().skip(3).enumerate() {
        assert_eq!(group.key, format!("slot.{index}"));
        assert_eq!(
            page_control_id(&group.rows[0]),
            format!("patch.effectSlot.{index}")
        );
        let nested: Vec<String> = group.rows[1..].iter().map(page_control_id).collect();
        assert!(
            nested.iter().all(|id| id.starts_with("patch.effect.")),
            "slot {index} nests only its occupant rows, got {nested:?}"
        );
    }
    assert_eq!(groups[3].rows.len(), 3, "slot 1 nests Chorus's two rows");
    assert_eq!(groups[4].rows.len(), 3, "slot 2 nests Chorus's two rows");
    assert_eq!(
        groups[5].rows.len(),
        1,
        "the empty slot 3 carries its occupancy row and nothing invented"
    );

    // The identity-and-routing header's three values are projected, not
    // composed: the Patch name from the main surface's own summary, and the
    // channel and track from the two Utility rows that own them.
    assert_eq!(
        document.pointer("/surfaces/0/summary/patchName"),
        Some(&Value::String("Lead".to_owned()))
    );
    for routing in ["patch.midiInput", "patch.output.outputTrack"] {
        assert!(
            surface_controls(&document, "patchUtility")
                .iter()
                .any(|control| page_control_id(control) == routing),
            "the strip header reads {routing} from the Utility surface"
        );
    }
    groups.len()
}

/// **The negative.** A flat run of every projected control fails the grouped
/// check.
///
/// The flat shape is built by rewriting each control's identity to one no
/// designed group claims, then driving the *same* page grouping the production
/// document goes through — so what is observed is the real arranger refusing a
/// real flat run, not a hand-built object refusing a hand-built assertion.
#[allow(dead_code)]
fn check_a_flat_run_fails_the_grouped_check() -> String {
    let state = fixture_state();
    let document = document(&state);
    let flat: Vec<Value> = surface_controls(&document, "patchMain")
        .into_iter()
        .map(|mut control| {
            let id = page_control_id(&control);
            control["path"]["controlId"]["id"] = Value::String(format!("flat.{id}"));
            control
        })
        .collect();
    let Err(error) = grouped_strip_shape(&flat) else {
        panic!("a flat run of every projected control must fail the grouped check");
    };
    assert!(
        error.contains("unknown marker") || error.contains("flat run"),
        "the flat-run negative fired for the wrong reason: {error}"
    );
    // And the same predicate accepts the real thing, so the negative is not
    // passing because the predicate rejects everything.
    assert!(grouped_strip_shape(&surface_controls(&document, "patchMain")).is_ok());
    error
}

/// A group with no view data marks itself unavailable rather than vanishing,
/// and a workspace with no focused Patch marks the strip unavailable.
///
/// Both are exercised by withholding the view data and driving the *same*
/// arranger, because no production fixture produces either shape — which is
/// exactly why SC-003 counts zero. A check that only looked at the production
/// document would report "no group is unavailable" without ever establishing
/// that the arranger can say so.
#[allow(dead_code)]
fn check_an_absent_group_and_an_unfocused_workspace_mark_themselves() {
    let document = document(&fixture_state());
    let controls = surface_controls(&document, "patchMain");

    // The envelope group's rows withheld. The group must still land at its
    // declared position, carrying no rows, so it marks itself inside its own
    // group rather than disappearing between INSTRUMENT and the capability
    // rows.
    let without_envelope: Vec<Value> = controls
        .iter()
        .filter(|control| !page_control_id(control).starts_with("patch.envelope."))
        .cloned()
        .collect();
    assert_eq!(
        without_envelope.len(),
        controls.len() - crest_synth::synth::VoiceEnvelope::surface_descriptor().len(),
        "the fixture must actually lose its envelope rows"
    );
    let groups = page_strip_groups(&without_envelope);
    let envelope = groups
        .iter()
        .position(|group| group.key == "envelope")
        .expect("a designed group with no view data must not vanish");
    assert!(
        groups[envelope].rows.is_empty(),
        "the withheld group must carry no rows, or nothing was withheld"
    );
    assert!(
        groups[envelope].designed && groups[envelope].legend.is_some(),
        "the group marks itself under its own authored legend"
    );
    assert_eq!(
        groups[envelope - 1].key,
        "instrument",
        "the withheld group lands at its declared position, not at the end"
    );
    // Withholding it does not reorder a painted row.
    let painted: Vec<String> = groups
        .iter()
        .flat_map(|group| group.rows.iter().map(page_control_id))
        .collect();
    assert_eq!(
        painted,
        without_envelope
            .iter()
            .map(page_control_id)
            .collect::<Vec<_>>(),
        "grouping is a presentation change and does not move a row"
    );

    // The page's own unavailable rule for the whole strip: `painted === 0`.
    let painted_rows = |controls: &[Value]| -> usize {
        page_strip_groups(controls)
            .iter()
            .map(|group| group.rows.len())
            .sum()
    };
    assert_eq!(
        painted_rows(&[]),
        0,
        "a workspace with no focused Patch paints no row and marks the strip unavailable"
    );
    assert!(
        painted_rows(&controls) > 0,
        "the production workspace paints rows, so the unavailable rule is not always true"
    );
    // And it stays a strip of groups rather than collapsing to nothing.
    assert_eq!(
        page_strip_groups(&[]).len(),
        DESIGNED_STRIP_GROUPS
            .iter()
            .filter(|(.., designed)| *designed)
            .count(),
        "every designed group survives an empty projection so each can mark itself"
    );
}

/// **SC-003, across the whole PATCH surface** — the count of rows marked
/// unavailable is zero.
///
/// Walked as the page walks it: the strip header, every strip group, the
/// Utility panel, and the detail shell when one is open, with each row's value
/// taken through the render script's own value contract so a row that projects
/// something the page cannot paint counts as unavailable here.
///
/// **The one declared exception, named rather than absorbed.** The Scope
/// Decisions table's claim that FR-012 closed the read-only surface-summary
/// *control kind* is withdrawn: `SemanticControlKind::Surface` has exactly one
/// construction site and it is reachable only from a fixture builder that no
/// production path calls. So the exception is not "some unavailable rows are
/// tolerated" — it is that **no `Surface`-kind row exists on a production
/// projection at all**, and that is asserted here as a zero rather than
/// excluded from a count. The read-only *fact* FR-012 genuinely supplies is
/// `patchInteraction`, proved separately by
/// [`check_a_read_only_section_is_marked_and_a_preparing_one_reports_itself`].
fn check_the_whole_patch_surface_marks_nothing_unavailable() -> usize {
    let mut walked = 0_usize;
    for (fixture, state) in screen_string_fixtures() {
        if state.context() != TopLevelContext::Patch {
            continue;
        }
        let document = document(&state);
        for surface in ["patchMain", "patchUtility", "patchDetail"] {
            for control in surface_controls(&document, surface) {
                walked += 1;
                let id = page_control_id(&control);
                let where_ = format!("{fixture}: {surface} {id}");
                assert_eq!(
                    control.get("visible"),
                    Some(&Value::Bool(true)),
                    "{where_} is projected invisible"
                );
                assert!(
                    control.get("enabled").and_then(Value::as_bool).is_some(),
                    "{where_} does not project an explicit enabled state"
                );
                let painted = page_value_text(&control);
                assert_ne!(
                    painted, UNAVAILABLE_MARK,
                    "{where_} projects a value yet paints the unavailable mark"
                );
                assert!(
                    !painted.starts_with('?'),
                    "{where_} paints a value kind the page cannot render: {painted}"
                );
                assert!(!painted.trim().is_empty(), "{where_} paints a blank value");
                // The declared exception, asserted as a zero.
                assert_ne!(
                    control.get("kind").and_then(Value::as_str),
                    Some("surface"),
                    "{where_} is a read-only surface-summary row; that control kind \
                     has no production producer and the spec's claim that FR-012 \
                     closed it is withdrawn"
                );
            }
        }
        // And the Utility panel's five designed entries each have a driver row.
        let utility: BTreeSet<String> = surface_controls(&document, "patchUtility")
            .iter()
            .map(page_control_id)
            .collect();
        for control in PatchControlId::UTILITY {
            assert!(
                utility.contains(control.as_str().as_ref()),
                "{fixture}: the Utility panel marks {} unavailable",
                control.as_str()
            );
        }
    }
    assert!(
        walked > 40,
        "only {walked} PATCH rows walked — the SC-003 sweep stopped seeing the surface"
    );
    walked
}

/// Validates the serialized Overview as projected section structure.
///
/// The controls remain one canonical ordered list. Sections reference those
/// controls by complete semantic path and carry correlated descriptor summary
/// data; the renderer never reconstructs either relationship from id prefixes.
fn overview_shape(document: &Value) -> Result<usize, String> {
    let main = surface_of(document, "patchMain")
        .ok_or_else(|| "the document has no PATCH main surface".to_owned())?;
    let controls = main
        .get("controls")
        .and_then(Value::as_array)
        .ok_or_else(|| "PATCH main has no control list".to_owned())?;
    let ids = controls.iter().map(page_control_id).collect::<Vec<_>>();
    let expected = vec![
        "patch.engine".to_owned(),
        "patch.effectSlot.0".to_owned(),
        "patch.effectSlot.1".to_owned(),
        "patch.effectSlot.2".to_owned(),
    ];
    if ids != expected {
        return Err(format!("PATCH root identities are {ids:?}"));
    }

    let sections = main
        .get("sections")
        .and_then(Value::as_array)
        .ok_or_else(|| "PATCH main has no section list".to_owned())?;
    let section_ids = sections
        .iter()
        .filter_map(|section| section.get("id").and_then(Value::as_str))
        .collect::<Vec<_>>();
    if section_ids != ["overview.engine", "overview.effects"] {
        return Err(format!("Overview section identities are {section_ids:?}"));
    }

    let mut arranged_paths = Vec::new();
    for section in sections {
        let paths = section
            .get("controlPaths")
            .and_then(Value::as_array)
            .ok_or_else(|| "an Overview section has no control paths".to_owned())?;
        let summaries = section
            .get("controlSummaries")
            .and_then(Value::as_array)
            .ok_or_else(|| "an Overview section has no control summaries".to_owned())?;
        if summaries.len() != paths.len() {
            return Err("an Overview section's paths and summaries differ in length".to_owned());
        }
        for (path, summary) in paths.iter().zip(summaries) {
            if !controls
                .iter()
                .any(|control| control.get("path") == Some(path))
            {
                return Err(format!("section path {path} resolves no root control"));
            }
            if summary.get("controlPath") != Some(path) {
                return Err(format!("section summary does not correlate to {path}"));
            }
            arranged_paths.push(path.clone());
        }
    }
    let control_paths = controls
        .iter()
        .filter_map(|control| control.get("path").cloned())
        .collect::<Vec<_>>();
    if arranged_paths != control_paths {
        return Err("Overview sections changed canonical root order".to_owned());
    }

    let summaries = sections
        .iter()
        .flat_map(|section| {
            section
                .get("controlSummaries")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .collect::<Vec<_>>();
    if summaries[0]
        .get("parameterCount")
        .and_then(Value::as_u64)
        .is_none()
    {
        return Err("the engine descriptor count is absent".to_owned());
    }
    if summaries[1..3].iter().any(|summary| {
        summary
            .get("parameterCount")
            .and_then(Value::as_u64)
            .is_none()
    }) {
        return Err("an occupied effect slot has no descriptor count".to_owned());
    }
    if !summaries[3]
        .get("parameterCount")
        .is_some_and(Value::is_null)
    {
        return Err("the empty effect slot invented a descriptor count".to_owned());
    }
    if page_value_text(&controls[3]) != "Empty" {
        return Err("the empty effect slot is not explicit".to_owned());
    }
    Ok(sections.len())
}

fn check_the_overview_is_projected_section_structure() -> usize {
    let document = document(&fixture_state());
    overview_shape(&document)
        .unwrap_or_else(|error| panic!("the production PATCH Overview is incoherent: {error}"))
}

fn check_an_unresolved_section_path_fails_the_overview_check() -> String {
    let mut malformed = document(&fixture_state());
    malformed["surfaces"][0]["sections"][0]["controlPaths"][0]["controlId"]["id"] =
        Value::String("unknown.root".to_owned());
    let error = overview_shape(&malformed)
        .expect_err("a section path without a canonical control must fail coherence");
    assert!(error.contains("resolves no root control"));
    assert!(overview_shape(&document(&fixture_state())).is_ok());
    error
}

fn check_an_empty_slot_remains_an_explicit_root_control() {
    let document = document(&fixture_state());
    let controls = surface_controls(&document, "patchMain");
    assert_eq!(page_control_id(&controls[3]), "patch.effectSlot.2");
    assert_eq!(page_value_text(&controls[3]), "Empty");
    assert_eq!(controls[3].get("visible"), Some(&Value::Bool(true)));
    assert_eq!(controls[3].get("focusable"), Some(&Value::Bool(true)));
}

// ---------------------------------------------------------------------------
// T031 — the five Utility rows, one master-gain owner, MIDI rechannelling
// ---------------------------------------------------------------------------

/// PATCH Utility resolves exactly the five declared rows in the declared
/// order, each carrying a real typed value, none marked unavailable, with the
/// panel's authored hint line present.
fn check_utility_resolves_five_typed_rows_and_its_hint_line() {
    let state = fixture_state();
    let model = semantic(&state);
    let utility = model
        .surface(SurfaceId::PatchUtility)
        .expect("PATCH always projects its persistent side surface");
    assert_eq!(utility.role(), SemanticSurfaceRole::PersistentSide);

    let ordered: Vec<PatchControlId> = utility
        .controls()
        .iter()
        .map(|control| match control.path().control_id() {
            SemanticControlId::Patch(id) => id.clone(),
            other => panic!("a Utility row carries the non-PATCH identity {other:?}"),
        })
        .collect();
    assert_eq!(
        ordered,
        PatchControlId::UTILITY.to_vec(),
        "the Utility panel resolves the five declared rows in the declared order"
    );

    for control in utility.controls() {
        let id = control.path().control_id();
        // A real typed canonical value, not a summary string standing in for
        // one. `Summary` is the read-only surface-summary shape, which no
        // production path produces (see the SC-003 exception).
        match control.value() {
            SemanticControlValue::Scalar(value) => assert!(
                value.is_finite(),
                "{id:?} carries a non-finite Utility value"
            ),
            SemanticControlValue::Identity(text) => {
                assert!(!text.is_empty(), "{id:?} carries an empty Utility value");
            }
            other => panic!("{id:?} carries the non-canonical Utility value {other:?}"),
        }
        assert!(control.enabled() && control.visible() && control.focusable());
        assert!(!control.label().is_empty());
    }
    // Four of the five are numeric and carry the descriptor's own bounds; the
    // output-track row is an adjacent choice and carries none.
    let bounded = utility
        .controls()
        .iter()
        .filter(|control| control.numeric_range().is_some())
        .count();
    assert_eq!(
        bounded, 4,
        "every numeric Utility row carries its descriptor's bounds"
    );

    let hint = page_side_hint_line(&document(&state), "patchUtility");
    assert!(
        !hint.is_empty(),
        "the Utility panel's authored hint line is absent"
    );
    // No assertion here on the colon. The colon in this string is contributed
    // by this file's own `format!`, unconditionally, so `hint.contains(':')` —
    // which cycle 2 shipped four lines from here — was satisfied by
    // construction and could not fail (F-71). The page's colon pairing is a
    // real rule and it is defended where it can be: by *the hint pairs its
    // label with a colon*, whose defeat in `page.js` fails the pin table.
    //
    // Both facts the design authority names: how an operator enters the panel
    // and how they leave it, each from the side of the boundary that owns it.
    //
    // Asserted on the *label* half — `hintLabel("Return")` — not on the hint
    // half. Cycle 1 asserted `contains("Return")`, which the painted line
    // satisfies through the physical hint `A / Return`, so it held whatever the
    // leave action's label said. The painted line reads
    // `D:utility A / Return:return`, and only the trailing pair comes from the
    // action this claim is about.
    // Utility returns through unmodified Left under the page-navigation
    // contract. The retired side-panel legend's Return token is not the
    // production footer or the reducer's current admission rule.
    let mut entered = entered_utility();
    let projected = semantic(&entered);
    assert!(projected.valid_actions().iter().any(|action| {
        action.action() == &crest_synth::control::SemanticAction::Navigate(Direction::Left)
    }));
    entered.apply(AppEvent::Navigate(Direction::Left)).unwrap();
    assert_eq!(entered.interaction().active_surface(), SurfaceId::PatchMain);
}

fn entered_utility() -> AppState {
    let mut state = fixture_state();
    state
        .apply(AppEvent::EnterSurface(SurfaceId::PatchUtility))
        .expect("the Utility panel is enterable");
    state
}

/// One canonical master gain, written from either surface and read from both,
/// with the absence of a second owner asserted structurally rather than by two
/// equal reads of one accessor.
fn check_one_master_gain_owner() {
    let read = |state: &AppState, surface: SurfaceId, control: &SemanticControlId| match control_at(
        &semantic(state),
        control,
    )
    .value()
    {
        SemanticControlValue::Scalar(value) => *value,
        other => panic!("{surface:?} projects the non-scalar master gain {other:?}"),
    };
    let patch_row = SemanticControlId::Patch(PatchControlId::Global(GlobalParameter::MasterGainDb));
    let mixer_row = SemanticControlId::Mixer(crest_synth::control::MixerControlId::Global {
        parameter: GlobalParameter::MasterGainDb,
    });

    // Written from PATCH, read from the MIXER Inspector.
    let mut state = fixture_state();
    let patches_before = state.patches().to_vec();
    enter_utility_row(
        &mut state,
        &PatchControlId::Global(GlobalParameter::MasterGainDb),
    );
    set_mode(&mut state, InteractionMode::Adjust);
    state
        .apply(AppEvent::Adjust(Direction::Right))
        .expect("the master gain row is editable from PATCH");
    set_mode(&mut state, InteractionMode::Navigate);
    let from_patch = read(&state, SurfaceId::PatchUtility, &patch_row);

    let mut inspector = state.clone();
    inspector
        .apply(AppEvent::SelectContext(TopLevelContext::Mixer))
        .unwrap();
    inspector
        .apply(AppEvent::EnterSurface(SurfaceId::MixerInspector))
        .unwrap();
    navigate_to(&mut inspector, |path| path.control_id() == &mixer_row);
    assert_eq!(
        read(&inspector, SurfaceId::MixerInspector, &mixer_row),
        from_patch,
        "an edit made on PATCH must be the value the MIXER Inspector reads"
    );

    // Written from MIXER, read from PATCH.
    set_mode(&mut inspector, InteractionMode::Adjust);
    inspector
        .apply(AppEvent::Adjust(Direction::Left))
        .expect("the master gain row is editable from MIXER");
    set_mode(&mut inspector, InteractionMode::Navigate);
    let from_mixer = read(&inspector, SurfaceId::MixerInspector, &mixer_row);
    assert_ne!(
        from_mixer, from_patch,
        "the second edit must actually move the value"
    );
    let mut back = inspector.clone();
    back.apply(AppEvent::SelectContext(TopLevelContext::Patch))
        .unwrap();
    back.apply(AppEvent::EnterSurface(SurfaceId::PatchUtility))
        .unwrap();
    navigate_to(&mut back, |path| path.control_id() == &patch_row);
    assert_eq!(
        read(&back, SurfaceId::PatchUtility, &patch_row),
        from_mixer,
        "an edit made on MIXER must be the value PATCH reads"
    );

    // Structural: no second owner exists.
    //
    // No Patch carries a copy — the two edits above moved a global and left
    // every installed Patch byte-identical.
    assert_eq!(
        back.patches(),
        patches_before.as_slice(),
        "an edit to the one global master gain must not touch any Patch"
    );
    // The declaration names exactly two serialized leaves for it, and they are
    // named here rather than counted, so a third appearing fails this.
    let tree = StateProjector::new()
        .project_with_shell_tree(&back)
        .expect("the state projects a tree")
        .5;
    let leaves: Vec<&str> = crest_synth::control::StateTree::serialized_leaf_descriptor()
        .iter()
        .copied()
        .filter(|leaf| leaf.rsplit('.').next() == Some("masterGainDb"))
        .collect();
    assert_eq!(
        leaves,
        vec!["global.masterGainDb", "parameters.global.masterGainDb"],
        "a third serialized master-gain leaf appeared: every one of these is a \
         place the value can be stored, and the claim is that there is one owner \
         plus its snapshot copy"
    );
    let json: Value = serde_json::from_str(tree.json()).expect("the tree is JSON");
    for leaf in &leaves {
        let pointer = format!("/{}", leaf.replace('.', "/"));
        let value = json
            .pointer(&pointer)
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("{leaf} is absent from the serialized tree"));
        assert!(
            (value - f64::from(back.global().master_gain_db())).abs() < 1e-6,
            "{leaf} holds {value}, which is not the one canonical master gain"
        );
    }
    // And nothing Patch-shaped carries one at all.
    assert!(
        !crest_synth::control::StateTree::serialized_leaf_descriptor()
            .iter()
            .any(|leaf| leaf.contains("patches") && leaf.ends_with("masterGainDb")),
        "a Patch-scoped master gain leaf exists"
    );
    assert!(
        !crest_synth::real_time::parameter_snapshot::ParameterSnapshot::serialized_leaf_descriptor(
        )
        .iter()
        .any(|leaf| leaf.starts_with("patches") && leaf.ends_with("masterGainDb")),
        "the parameter snapshot carries a per-Patch master gain"
    );
}

/// **The screen-string guard.** No string a production projection puts on
/// screen — a label, a painted value, a painted requested value, a group
/// title, an action label, or a footer breadcrumb segment — is a serialization
/// key.
///
/// Two widenings past the in-crate guard, both from mutation sweeps rather
/// than from reasoning:
///
/// - the key set carries capability and section identities (F-28: a 17-site
///   sweep caught 10 and missed 7, all seven because the set could not express
///   the key), and every **choice** identity;
/// - the walk carries **values**, not labels only (F-33: the Preset row painted
///   `sf2.bank-0.program-40` as a value, which the label guard walks past).
fn check_no_projected_screen_string_is_a_serialization_key() -> usize {
    use crest_synth::control::{ControllerDevice, ControllerEvent, ControllerFailure, SendAction};

    let mut checked = 0_usize;
    let mut covered = BTreeSet::new();
    let mut audited_sites = BTreeSet::new();
    let mut fixtures = screen_string_fixtures();
    let mut midi_settings = fixture_state();
    midi_settings
        .apply_semantic_action(SemanticAction::OpenMidiSettings)
        .expect("the fixture opens the global MIDI device Settings surface");
    let mut save_load_settings = midi_settings.clone();
    save_load_settings
        .apply_semantic_action(SemanticAction::Navigate(Direction::Left))
        .unwrap();
    fixtures.push(("Save & Load Settings", save_load_settings));
    fixtures.push(("MIDI Device Settings", midi_settings.clone()));

    let mut controller = midi_settings;
    controller
        .apply(AppEvent::Navigate(Direction::Right))
        .unwrap();
    assert_eq!(
        controller.interaction().active_surface(),
        SurfaceId::ControllerSettings
    );
    fixtures.push(("Controller Settings disconnected", controller.clone()));
    controller
        .apply(AppEvent::Controller(ControllerEvent::PreferencesLoaded {
            result: Ok(None),
        }))
        .unwrap();
    controller
        .apply(AppEvent::Controller(ControllerEvent::DevicesChanged {
            devices: vec![ControllerDevice {
                id: 7,
                name: "Studio Gamepad".to_owned(),
            }],
        }))
        .unwrap();
    let mut capture = controller.clone();
    capture.apply(AppEvent::Activate).unwrap();
    fixtures.push(("Controller Settings capture", capture));
    for setting in crest_synth::control::ControllerSettingId::all() {
        navigate_to(&mut controller, |path| {
            path.control_id() == &SemanticControlId::ControllerSetting(setting)
        });
        fixtures.push(("Controller Settings connected", controller.clone()));
    }
    controller
        .apply(AppEvent::Controller(ControllerEvent::BackendFailed {
            failure: ControllerFailure::BackendUnavailable,
        }))
        .unwrap();
    fixtures.push(("Controller Settings failure", controller));

    let mut sends = fixture_state();
    sends.apply(AppEvent::Send(SendAction::Open)).unwrap();
    fixtures.push(("Sends INIT", sends));
    let mut sends = fixture_state();
    let mut returns = sends.bus_returns().clone();
    let bus = crest_synth::mixer::bus_id::BusId::default();
    let slot_id = EffectSlotId::new(1).unwrap();
    returns
        .set_effect_slot(
            sends.effects(),
            bus,
            slot_id,
            Some(&crest_synth::synth::EffectCapabilityId::new(CHORUS_CAPABILITY_ID).unwrap()),
        )
        .unwrap();
    sends = sends.with_initial_returns(returns);
    sends.apply(AppEvent::Send(SendAction::Open)).unwrap();
    sends.apply(AppEvent::Activate).unwrap();
    sends
        .apply(AppEvent::Send(SendAction::Rename {
            bus,
            name: "Long Hall".to_owned(),
        }))
        .unwrap();
    fixtures.push(("Sends occupied", sends.clone()));
    sends.apply(AppEvent::Activate).unwrap();
    fixtures.push(("Sends rename", sends.clone()));
    sends
        .apply(AppEvent::Send(SendAction::CancelRename))
        .unwrap();
    navigate_to(&mut sends, |path| {
        matches!(path.control_id(), SemanticControlId::Send(
        crest_synth::control::SendControlId::EffectSlot { slot_id: slot, .. }
    ) if *slot == slot_id)
    });
    sends.apply(AppEvent::Activate).unwrap();
    fixtures.push(("Sends effect choices", sends));
    for (fixture, state) in fixtures {
        let keys = serialization_keys(&state);
        for surface in semantic(&state).surfaces() {
            covered.insert(format!("{:?}", surface.id()));
        }
        for (site, string) in projected_screen_strings(&state) {
            checked += 1;
            assert!(
                !keys.contains(&string),
                "{fixture}: {site} puts the serialization key {string} on screen"
            );
            audited_sites.insert(site);
        }
    }
    assert_eq!(
        covered,
        SurfaceId::ALL
            .into_iter()
            .map(|surface| format!("{surface:?}"))
            .collect::<BTreeSet<_>>(),
        "the guard must cover every surface, not just the ones that were reported"
    );
    for site in [
        "surface sends section title",
        "surface sends /summary/name",
        "surface sends /summary/choiceGroupLabel",
        "surface controllerSettings /summary/summary",
        "surface controllerSettings /summary/description",
        "controller device name",
    ] {
        assert!(
            audited_sites.contains(site),
            "unvisited screen text: {site}"
        );
    }
    assert!(
        checked > 400,
        "only {checked} screen strings walked — the guard stopped seeing the projection"
    );
    checked
}

/// The MIDI input row lets the focused Patch join another Patch's channel and
/// changes nothing else.
///
/// End-to-end source fan-out is proved at `AutomaticMidiTest`, where one raw
/// channel message becomes one command per matching Patch. This integration
/// check owns the complementary reducer fact: shared subscriptions are valid
/// canonical state and both subscribers accept the same normalized message.
fn check_the_midi_input_row_accepts_shared_subscriptions() {
    let mut state = fixture_state();
    let patch_id = state.interaction().patch_focus().unwrap();
    let existing_subscriber = state.patches()[1].id();
    let before = focused_patch(&state).clone();
    let old_channel = before.channel();
    let graph_before = state.engine_selection().active_graph_revision();

    enter_utility_row(&mut state, &PatchControlId::MidiInput);
    set_mode(&mut state, InteractionMode::Adjust);
    state
        .apply(AppEvent::Adjust(Direction::Right))
        .expect("the Patch may join an already subscribed channel");
    set_mode(&mut state, InteractionMode::Navigate);

    let after = focused_patch(&state);
    let new_channel = after.channel();
    assert_eq!(
        new_channel.value(),
        old_channel.value() + 1,
        "the row edits the focused Patch's own channel by the declared step"
    );
    // Both Patches now subscribe to the same incoming channel.
    assert_eq!(
        state
            .patches()
            .iter()
            .filter(|patch| patch.channel() == new_channel)
            .map(Patch::id)
            .collect::<Vec<_>>(),
        vec![patch_id, existing_subscriber],
        "the edited Patch layers with the existing channel subscriber"
    );
    assert!(
        state
            .patches()
            .iter()
            .all(|patch| patch.channel() != old_channel),
        "no installed Patch remains subscribed to the old channel"
    );
    let message = MidiMessage::try_new(new_channel, MidiMessageKind::NoteOn, 60, 100).unwrap();
    for subscriber in [patch_id, existing_subscriber] {
        let outcome = state
            .apply(AppEvent::Midi {
                patch_id: subscriber,
                message,
            })
            .expect("each subscribed Patch accepts the same channel message");
        match outcome.audio_command() {
            Some(AudioCommand::PatchMidi {
                patch_id: id,
                message: dispatched,
            }) => {
                assert_eq!(*id, subscriber);
                assert_eq!(*dispatched, message);
            }
            other => panic!("a MIDI event must emit exactly one PatchMidi command, got {other:?}"),
        }
    }

    // Identity, config, envelope, effects, routing, and the active graph
    // revision are untouched by a channel edit.
    let after = focused_patch(&state);
    assert_eq!(after.id(), before.id());
    assert_eq!(after.name(), before.name());
    assert_eq!(after.instrument_config(), before.instrument_config());
    assert_eq!(after.envelope(), before.envelope());
    assert_eq!(after.effect_slots(), before.effect_slots());
    assert_eq!(after.output(), before.output());
    assert_eq!(after.voice_limit(), before.voice_limit());
    assert_eq!(
        state.engine_selection().active_graph_revision(),
        graph_before
    );
    assert!(!state.engine_selection().is_in_flight());
}

// ---------------------------------------------------------------------------
// T032 — per-row actions, requested values, painted ranges and units
// ---------------------------------------------------------------------------

/// Each control's action list is the same resolver's answer for the
/// counterfactual focus in which that control is focused, and at the row that
/// really is focused it is the model-level list itself.
///
/// The counterfactual is driven through the **reducer**: the cursor is moved
/// onto each row and the model-level list read there, so a per-row list that
/// diverged from what the reducer would actually accept fails. That is
/// strictly stronger than comparing two calls of one function.
fn check_per_row_actions_agree_with_the_model_level_list() -> usize {
    let state = fixture_state();
    let model = semantic(&state);
    assert_eq!(
        model
            .focused_control()
            .expect("PATCH always focuses a row")
            .valid_actions(),
        model.valid_actions(),
        "at the focused row the two lists are the same value, not two computations"
    );

    let mut detail = fixture_state();
    detail
        .apply(AppEvent::EnterSurface(SurfaceId::PatchDetail))
        .unwrap();
    let mut utility = fixture_state();
    utility
        .apply(AppEvent::EnterSurface(SurfaceId::PatchUtility))
        .unwrap();

    let mut compared = 0_usize;
    for fixture in [fixture_state(), detail, utility] {
        let fixture_model = semantic(&fixture);
        let surface = fixture_model
            .surface(fixture.interaction().active_surface())
            .expect("the active surface is projected");
        for control in surface.controls() {
            let path = control.path().clone();
            let advertised = control.valid_actions().to_vec();
            let mut moved = fixture.clone();
            navigate_to(&mut moved, |candidate| candidate == &path);
            let actual = semantic(&moved).valid_actions().to_vec();
            assert_eq!(
                advertised,
                actual,
                "the row {:?} advertises an action list the reducer does not honour there",
                path.control_id()
            );
            let kinds = advertised
                .iter()
                .map(|action| format!("{:?}", action.action()))
                .collect::<Vec<_>>();
            let unique = kinds.iter().cloned().collect::<BTreeSet<_>>();
            assert_eq!(
                kinds.len(),
                unique.len(),
                "a row's action list repeats itself"
            );
            compared += 1;
        }
    }
    assert!(compared >= 12, "only {compared} rows compared");
    compared
}

/// `requestedValue` is `Some` exactly while a correlated structural edit is in
/// flight, and `None` on every settled row.
fn check_requested_value_is_present_only_while_an_edit_is_in_flight() {
    // Settled: the count of rows claiming a requested value is zero, on every
    // PATCH fixture including both engines and both detail subjects.
    for (fixture, state) in screen_string_fixtures() {
        if state.engine_selection().is_in_flight() {
            continue;
        }
        let model = semantic(&state);
        let claiming: Vec<_> = all_controls(&model)
            .into_iter()
            .filter(|control| control.requested_value().is_some())
            .map(|control| control.path().control_id().clone())
            .collect();
        assert_eq!(
            claiming.len(),
            0,
            "{fixture}: {} settled row(s) claim a requested value: {claiming:?}",
            claiming.len()
        );
    }

    // In flight: exactly the correlated row carries one, and it carries the
    // value the reducer has already agreed to move it toward.
    let (state, control) = preset_swap_in_flight();
    let model = semantic(&state);
    let carrying: Vec<_> = model
        .surface(SurfaceId::PatchDetail)
        .unwrap()
        .controls()
        .iter()
        .filter(|row| row.requested_value().is_some())
        .collect();
    assert_eq!(
        carrying.len(),
        1,
        "exactly the correlated row carries the requested value"
    );
    assert_eq!(carrying[0].path().control_id(), &control);
    let intent_choice = state
        .engine_selection()
        .correlation()
        .unwrap()
        .intent()
        .choice_id()
        .expect("the in-flight intent names a choice")
        .to_owned();
    assert_eq!(
        carrying[0].requested_value(),
        Some(&SemanticControlValue::Parameter(ParameterValue::Choice(
            intent_choice
        ))),
        "the requested value comes from the correlated lifecycle, never from the input"
    );
    // And it is not the active value under another name.
    assert_ne!(carrying[0].requested_value(), Some(carrying[0].value()));
}

/// Every projected numeric control's range and unit are **rendered**, not
/// merely projected.
///
/// Read back through the render script's own `rangeHtml` / `rangeEndpointText`
/// and unit rules rather than off the projection, so a page that stopped
/// painting either fails here.
///
/// **How much each half is read back is three tiers, not two** (F-70). Values
/// are read back in full. Ranges are read back only *structurally*: the
/// comparison below parses the painted endpoints after applying the renderer's
/// unit scale. It therefore proves both native-unit bounds and normalized
/// percentage bounds, while the formatter correspondence covers precision.
fn check_ranges_and_units_are_rendered() -> usize {
    let mut rendered = 0_usize;
    let mut units = 0_usize;
    for (fixture, state) in screen_string_fixtures() {
        let document = document(&state);
        for surface in [
            "patchMain",
            "patchUtility",
            "patchDetail",
            "mixerMain",
            "mixerInspector",
        ] {
            for control in surface_controls(&document, surface) {
                let id = page_control_id(&control);
                let Some(range) = control.get("numericRange").filter(|r| r.is_object()) else {
                    continue;
                };
                let painted = page_range_text(&control).unwrap_or_else(|| {
                    panic!("{fixture}: {surface} {id} projects a range the page paints nothing for")
                });
                let minimum = range["minimum"].as_f64().unwrap();
                let maximum = range["maximum"].as_f64().unwrap();
                let scale = page_numeric_scale(&control);
                let (low, high) = painted.split_once(" — ").unwrap_or_else(|| {
                    panic!("{fixture}: {id} paints {painted} with no separator")
                });
                assert_eq!(
                    low.parse::<f64>().unwrap(),
                    minimum * scale,
                    "{fixture}: {id} paints a lower bound the projection does not declare"
                );
                assert_eq!(
                    high.parse::<f64>().unwrap(),
                    maximum * scale,
                    "{fixture}: {id} paints an upper bound the projection does not declare"
                );
                assert!(
                    range["fineStep"].as_f64().is_some_and(f64::is_finite)
                        && range["coarseStep"].as_f64().is_some_and(f64::is_finite),
                    "{fixture}: {id} projects a range with no step"
                );
                rendered += 1;
                if let Some(unit) = page_unit_text(&control) {
                    assert!(!unit.is_empty(), "{fixture}: {id} projects an empty unit");
                    units += 1;
                }
            }
        }
    }
    assert!(
        rendered > 100,
        "only {rendered} numeric rows painted a range"
    );
    assert!(units > 0, "no projected unit was painted");
    rendered
}

/// The page composes no label and no action label of its own.
///
/// Every projected control label, action label and action hint, checked against
/// the committed render script's own text: none of them appears there as a
/// literal. A page that hard-coded "Voice Limit", "Preset" or "Adjust" would
/// paint the right words for the wrong reason, and this is what says so.
///
/// Scoped to control and action strings deliberately. Surface labels are
/// excluded and named: the page carries `"DETAIL"` as the name of a structure
/// it must still mark when the projection supplies no detail surface at all,
/// which is the authored no-placeholder behaviour rather than a composed label.
fn check_the_page_composes_no_label_of_its_own() -> usize {
    let script = script_without_comments(&page_source("page.js"));
    // The canonical T key hint also spells the existing hexadecimal track
    // identifier prefix. Exclude that one formatting expression, while still
    // rejecting any literal T introduced in action/footer composition.
    let script = script.replacen(
        "return \"T\" + (hex.length",
        "return TRACK_PREFIX + (hex.length",
        1,
    );
    let mut checked = 0_usize;
    let mut vocabulary = BTreeSet::new();
    for (_, state) in screen_string_fixtures() {
        let document = document(&state);
        for surface in document
            .get("surfaces")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            for control in surface
                .get("controls")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                if let Some(label) = control.get("label").and_then(Value::as_str) {
                    vocabulary.insert(label.to_owned());
                }
                for action in control
                    .get("validActions")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
                {
                    for key in ["label", "hint"] {
                        if let Some(text) = action.get(key).and_then(Value::as_str) {
                            vocabulary.insert(text.to_owned());
                        }
                    }
                }
            }
        }
    }
    for text in &vocabulary {
        checked += 1;
        for quoted in [format!("\"{text}\""), format!("'{text}'")] {
            assert!(
                !script.contains(&quoted),
                "webview-page/page.js composes the projected string {text} itself"
            );
        }
    }
    assert!(
        checked > 20,
        "only {checked} projected strings checked against the render script"
    );
    checked
}

// ---------------------------------------------------------------------------
// T033 — one detail identity, two subjects, exact return
// ---------------------------------------------------------------------------

/// Opens the detail surface from the row `predicate` names, and reports the
/// surface identity and the subject it served.
fn open_detail(
    predicate: impl Fn(&FocusPath) -> bool,
) -> (AppState, SurfaceId, PatchDetailSubject, FocusPath) {
    let mut state = fixture_state();
    navigate_to(&mut state, &predicate);
    let origin = state.interaction().focus_path().clone();
    state
        .apply(AppEvent::EnterSurface(SurfaceId::PatchDetail))
        .expect("the origin row resolves a detail subject");
    let surface = state.interaction().active_surface();
    let subject = state
        .interaction()
        .detail_subject()
        .expect("an open detail surface holds its subject")
        .clone();
    (state, surface, subject, origin)
}

fn is_control(control: &PatchControlId) -> impl Fn(&FocusPath) -> bool + '_ {
    move |path: &FocusPath| path.control_id() == &SemanticControlId::Patch(control.clone())
}

/// **One** `PatchDetail` surface identity serves an instrument subject and an
/// effect subject, and everything it shows comes from the installed descriptor
/// with no branch on subject kind.
fn check_one_detail_identity_serves_two_subjects() -> (usize, usize) {
    let (instrument_state, instrument_surface, instrument_subject, _) =
        open_detail(is_control(&PatchControlId::Engine));
    let (effect_state, effect_surface, effect_subject, _) = open_detail(is_control(
        &PatchControlId::EffectSlot(EffectSlotIndex::ALL[0]),
    ));

    let identities: BTreeSet<String> = BTreeSet::from([
        format!("{instrument_surface:?}"),
        format!("{effect_surface:?}"),
    ]);
    let subjects: BTreeSet<String> = BTreeSet::from([
        format!("{instrument_subject:?}"),
        format!("{effect_subject:?}"),
    ]);
    assert_eq!(
        identities.len(),
        1,
        "two subjects were served by {} surface identities",
        identities.len()
    );
    assert_eq!(
        identities.into_iter().next(),
        Some(format!("{:?}", SurfaceId::PatchDetail))
    );
    assert_eq!(subjects.len(), 2, "one surface identity served one subject");
    assert!(matches!(
        instrument_subject,
        PatchDetailSubject::Instrument { .. }
    ));
    assert!(matches!(effect_subject, PatchDetailSubject::Effect { .. }));

    // Title, sections, controls, ranges, units and status all come from the
    // installed descriptor. Asserted by reading the descriptor the subject
    // names and comparing row for row, both times, with the same code.
    for (state, subject) in [
        (&instrument_state, &instrument_subject),
        (&effect_state, &effect_subject),
    ] {
        let page = page(state);
        let detail = page
            .detail()
            .expect("an open detail surface projects a page");
        let (label, specs): (String, Vec<_>) = match subject {
            PatchDetailSubject::Instrument { capability_id } => {
                let descriptor = state.capabilities().descriptor(capability_id).unwrap();
                (
                    descriptor.label().to_owned(),
                    descriptor.parameters().collect(),
                )
            }
            PatchDetailSubject::Effect { capability_id, .. } => {
                let descriptor = state.effects().descriptor(capability_id).unwrap();
                (
                    descriptor.label().to_owned(),
                    descriptor.parameters().collect(),
                )
            }
        };
        assert_eq!(detail.label(), label, "the title is the descriptor's own");
        let projected: Vec<_> = detail
            .sections()
            .iter()
            .flat_map(PatchPageSection::parameters)
            .collect();
        assert_eq!(projected.len(), specs.len());
        for (row, spec) in projected.iter().zip(&specs) {
            assert_eq!(row.id(), spec.id());
            assert_eq!(row.label(), spec.label());
            assert_eq!(row.kind(), spec.kind());
            assert_eq!(row.range(), spec.range());
            assert_eq!(row.unit(), spec.unit());
            assert_eq!(row.patch_interaction(), spec.patch_interaction());
        }
        assert_eq!(
            detail.status(),
            state.engine_selection().kind(),
            "the surface reports the one projected lifecycle"
        );
        // The semantic model's detail rows agree, so the two documents cannot
        // disagree about what the same descriptor declared. Instrument detail
        // additionally carries the canonical Patch envelope; effect detail
        // does not acquire an instrument-only row family.
        let model = semantic(state);
        let rows = model.surface(SurfaceId::PatchDetail).unwrap().controls();
        let schema_rows = rows
            .iter()
            .filter(|row| {
                matches!(
                    (subject, row.path().control_id()),
                    (
                        PatchDetailSubject::Instrument { .. },
                        SemanticControlId::Patch(PatchControlId::Capability(_)),
                    ) | (
                        PatchDetailSubject::Effect { .. },
                        SemanticControlId::Patch(PatchControlId::Effect(..)),
                    )
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(schema_rows.len(), specs.len());
        for (row, spec) in schema_rows.iter().zip(&specs) {
            assert_eq!(row.label(), spec.label());
            assert_eq!(row.patch_interaction(), Some(spec.patch_interaction()));
            assert_eq!(row.unit(), spec.unit());
        }
        let envelope_rows = rows
            .iter()
            .filter_map(|row| match row.path().control_id() {
                SemanticControlId::Patch(PatchControlId::Envelope(parameter)) => {
                    Some((row, *parameter))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        match subject {
            PatchDetailSubject::Instrument { .. } => {
                assert_eq!(
                    envelope_rows.len(),
                    crest_synth::synth::VoiceEnvelope::surface_descriptor().len()
                );
                for ((row, parameter), descriptor) in envelope_rows
                    .iter()
                    .zip(crest_synth::synth::VoiceEnvelope::surface_descriptor())
                {
                    assert_eq!(*parameter, descriptor.parameter());
                    assert_eq!(row.label(), descriptor.label());
                    assert_eq!(row.patch_interaction(), Some(PatchInteraction::ScalarEdit));
                }
            }
            PatchDetailSubject::Effect { .. } => assert!(envelope_rows.is_empty()),
        }
    }
    (1, 2)
}

/// Two positions holding the same registry entry are two subjects.
fn check_two_slots_of_one_entry_are_distinct_subjects() {
    let (_, _, first, _) = open_detail(is_control(&PatchControlId::EffectSlot(
        EffectSlotIndex::ALL[0],
    )));
    let (_, _, second, _) = open_detail(is_control(&PatchControlId::EffectSlot(
        EffectSlotIndex::ALL[1],
    )));
    let (
        PatchDetailSubject::Effect {
            slot_id: first_slot,
            capability_id: first_capability,
        },
        PatchDetailSubject::Effect {
            slot_id: second_slot,
            capability_id: second_capability,
        },
    ) = (&first, &second)
    else {
        panic!("both fixture positions hold an effect");
    };
    assert_eq!(
        first_capability, second_capability,
        "the fixture must hold the same registry entry in both positions, or this proves nothing"
    );
    assert_eq!(first_capability.as_str(), CHORUS_CAPABILITY_ID);
    assert_ne!(
        first_slot, second_slot,
        "two positions holding one entry must be two subjects"
    );
    assert_ne!(first, second);
}

/// Entry is refused from an empty slot and from a Utility row, as typed
/// unchanged rejections.
fn check_detail_entry_is_refused_where_no_subject_exists() {
    // The empty third position names a position, not a capability, and is
    // deliberately not repaired into a neighbouring subject.
    let mut empty = fixture_state();
    navigate_to(
        &mut empty,
        is_control(&PatchControlId::EffectSlot(EffectSlotIndex::ALL[2])),
    );
    let before = empty.clone();
    assert_eq!(
        empty.apply(AppEvent::EnterSurface(SurfaceId::PatchDetail)),
        Err(EventRejection::ActionUnavailableInContext),
        "an empty position resolves no subject"
    );
    assert_eq!(empty, before, "a refused entry leaves the state identical");
    assert_eq!(empty.interaction().detail_subject(), None);

    // A Utility row is not a capability row and never resolves a subject.
    let mut utility = entered_utility();
    let before = utility.clone();
    assert_eq!(
        utility.apply(AppEvent::EnterSurface(SurfaceId::PatchDetail)),
        Err(EventRejection::ActionUnavailableInContext),
        "a Utility row resolves no detail subject"
    );
    assert_eq!(utility, before);
}

/// Subordinate surfaces do not nest, in both directions.
fn check_subordinate_surfaces_do_not_nest() {
    let (mut detail, ..) = open_detail(is_control(&PatchControlId::Engine));
    let before = detail.clone();
    assert_eq!(
        detail.apply(AppEvent::EnterSurface(SurfaceId::PatchUtility)),
        Err(EventRejection::ActionUnavailableInContext),
        "the persistent side surface cannot open beneath the detail surface"
    );
    assert_eq!(detail, before);
    assert_eq!(
        detail.interaction().active_surface(),
        SurfaceId::PatchDetail
    );

    let mut utility = entered_utility();
    let before = utility.clone();
    assert_eq!(
        utility.apply(AppEvent::EnterSurface(SurfaceId::PatchDetail)),
        Err(EventRejection::ActionUnavailableInContext),
        "the detail surface cannot open beneath the persistent side surface"
    );
    assert_eq!(utility, before);
    assert!(utility.interaction().detail_invariant_holds());
}

/// Return lands on the **exact** originating row after reprojection, proved
/// from an origin that is not the first row.
fn check_return_lands_on_the_exact_origin() {
    let origin_control = PatchControlId::EffectSlot(EffectSlotIndex::ALL[1]);
    let (mut state, _, _, origin) = open_detail(is_control(&origin_control));
    let order = SemanticResolver::new(&state)
        .patch_main_paths(state.interaction().patch_focus().unwrap())
        .unwrap();
    assert_ne!(
        order.first(),
        Some(&origin),
        "the origin must not be the first row, or a recomputed default would pass"
    );
    assert_eq!(
        state.interaction().return_path().map(|path| path.origin()),
        Some(&origin)
    );
    state
        .apply(AppEvent::Return)
        .expect("an open detail surface can be left");
    assert_eq!(
        state.interaction().focus_path(),
        &origin,
        "return lands on the exact originating row, not a surface default"
    );
    assert_eq!(state.interaction().active_surface(), SurfaceId::PatchMain);
    assert_eq!(state.interaction().detail_subject(), None);
    assert!(state.interaction().detail_invariant_holds());
    // And the projection agrees, after reprojection.
    assert_eq!(semantic(&state).focused_control().unwrap().path(), &origin);
}

/// A capability-declared read-only section is marked in text or shape, and a
/// mid-preparation capability reports its lifecycle rather than emptying its
/// section set.
///
/// The read-only *fact* is `patchInteraction`, which is what the render script
/// discriminates on (`control.patchInteraction === "readOnly"` → the authored
/// `READ-ONLY` mark plus a dashed keyline, so the declaration reads in text and
/// in shape rather than in colour alone). Phase 7's shared Patch-envelope rows
/// are scalar-editable beside capability-owned read-only rows, so the two facts
/// must agree row by row rather than being inferred from a whole surface.
fn check_a_read_only_section_is_marked_and_a_preparing_one_reports_itself() {
    // Braids engine parameters and the canonical Patch envelope are all
    // scalar-editable on the shared instrument-detail shell.
    let mut braids = fixture_state();
    braids
        .apply(AppEvent::SelectPatch(Direction::Right))
        .unwrap();
    braids
        .apply(AppEvent::EnterSurface(SurfaceId::PatchDetail))
        .expect("the engine row resolves an instrument subject");
    let braids_document = document(&braids);
    let rows = surface_controls(&braids_document, "patchDetail");
    assert!(!rows.is_empty());
    let interactions = rows
        .iter()
        .filter_map(|row| row.get("patchInteraction").and_then(Value::as_str))
        .collect::<BTreeSet<_>>();
    assert_eq!(
        interactions,
        BTreeSet::from(["scalarEdit"]),
        "Braids engine parameters and the shared envelope must permit scalar editing"
    );
    assert!(rows.iter().all(
        |row| match row.get("patchInteraction").and_then(Value::as_str) {
            Some("readOnly") => row.get("editable") == Some(&Value::Bool(false)),
            Some("scalarEdit") => row.get("editable") == Some(&Value::Bool(true)),
            _ => false,
        }
    ));

    // SoundFont's detail carries one structural row beside its read-only file
    // row, so the mark discriminates rather than being uniformly true.
    let (soundfont, ..) = open_detail(is_control(&PatchControlId::Engine));
    let soundfont_document = document(&soundfont);
    let rows = surface_controls(&soundfont_document, "patchDetail");
    let interactions: BTreeSet<&str> = rows
        .iter()
        .filter_map(|row| row.get("patchInteraction").and_then(Value::as_str))
        .collect();
    assert_eq!(
        interactions,
        BTreeSet::from(["readOnly", "scalarEdit", "structuralChoice"]),
        "SoundFont detail must retain all three descriptor/shared interaction classes"
    );
    assert!(rows.iter().all(
        |row| match row.get("patchInteraction").and_then(Value::as_str) {
            Some("readOnly") => row.get("editable") == Some(&Value::Bool(false)),
            Some("scalarEdit" | "structuralChoice") => {
                row.get("editable") == Some(&Value::Bool(true))
            }
            _ => false,
        }
    ));

    // A capability mid-preparation reports its typed lifecycle on its own rows
    // and keeps the section set the installed descriptor declares.
    let (preparing, _) = preset_swap_in_flight();
    let settled_detail = rows.len();
    let preparing_document = document(&preparing);
    let rows = surface_controls(&preparing_document, "patchDetail");
    assert_eq!(
        rows.len(),
        settled_detail,
        "preparation must not empty the section set the descriptor declares"
    );
    assert!(
        rows.iter().all(|row| matches!(
            row.pointer("/status/kind").and_then(Value::as_str),
            Some("loading" | "validating" | "preparing" | "activating")
        )),
        "a preparing subject reports its typed lifecycle on its own rows"
    );
}

// ---------------------------------------------------------------------------
// T034 — the voice limit: bounded, seeded, enforced, falsifiable
// ---------------------------------------------------------------------------

/// Budgets accept every positive representable count, including values above old defaults.
fn check_the_voice_limit_is_bounded_and_refuses_rather_than_clamps() {
    assert_eq!(VoiceLimit::MINIMUM, 1);
    assert_eq!(VoiceLimit::MAXIMUM, u16::MAX);
    assert_eq!(VoiceLimit::new(1).map(VoiceLimit::value), Ok(1));
    assert_eq!(VoiceLimit::new(64).map(VoiceLimit::value), Ok(64));
    for value in [65, 128, u16::MAX] {
        assert_eq!(VoiceLimit::new(value).unwrap().value(), value);
    }
    assert_eq!(
        VoiceLimit::new(0),
        Err(VoiceLimitError::OutOfRange { value: 0 }),
        "zero must be refused rather than clamped"
    );
    let descriptor = VoiceLimit::descriptor();
    assert_eq!(descriptor.minimum(), VoiceLimit::MINIMUM);
    assert_eq!(descriptor.maximum(), VoiceLimit::MAXIMUM);
    assert_eq!(descriptor.kind(), ParameterKind::Stepped);
}

/// Every Patch is seeded from its own engine's declared default, and the
/// two engines seed differently.
fn check_each_patch_is_seeded_from_its_own_engine_default() {
    let state = fixture_state();
    let registry = state.capabilities();
    for patch in state.patches() {
        let ceiling = registry
            .descriptor(patch.instrument_config().capability_id())
            .expect("every installed Patch names an installed capability")
            .voice_policy()
            .initial_voices();
        assert_eq!(
            patch.voice_limit(),
            VoiceLimit::seeded_from_ceiling(ceiling),
            "{:?} was not seeded from its own engine's default",
            patch.id()
        );
    }
    let soundfont = patch_of(&state, PatchId::new(1).unwrap())
        .voice_limit()
        .value();
    let braids = patch_of(&state, PatchId::new(2).unwrap())
        .voice_limit()
        .value();
    assert_eq!(soundfont, HIDEF_POLYPHONY_CEILING);
    assert_eq!(braids, BRAIDS_FIXED_VOICES);
    assert_ne!(
        soundfont, braids,
        "a fixture whose engines seed the same limit cannot falsify per-engine seeding"
    );
}

/// The limit is edited as a `Stepped` control through the canonical reducer,
/// honouring the descriptor's fine and coarse steps, and refusing at its bound.
fn check_the_voice_limit_is_edited_as_a_stepped_control() {
    let descriptor = VoiceLimit::descriptor();
    let mut state = fixture_state();
    enter_utility_row(&mut state, &PatchControlId::VoiceLimit);
    let model = semantic(&state);
    let row = control_at(
        &model,
        &SemanticControlId::Patch(PatchControlId::VoiceLimit),
    );
    assert_eq!(row.kind(), SemanticControlKind::Stepped);
    let range = row
        .numeric_range()
        .expect("the limit row carries its bounds");
    assert_eq!(range.minimum(), f64::from(descriptor.minimum()));
    assert_eq!(range.maximum(), f64::from(descriptor.maximum()));
    assert_eq!(range.fine_step(), f64::from(descriptor.fine_step()));
    assert_eq!(range.coarse_step(), f64::from(descriptor.coarse_step()));

    set_mode(&mut state, InteractionMode::Adjust);
    let start = focused_patch(&state).voice_limit().value();
    state.apply(AppEvent::Adjust(Direction::Left)).unwrap();
    assert_eq!(
        focused_patch(&state).voice_limit().value(),
        start - descriptor.fine_step(),
        "Left moves by the descriptor's fine step"
    );
    state.apply(AppEvent::Adjust(Direction::Down)).unwrap();
    assert_eq!(
        focused_patch(&state).voice_limit().value(),
        start - descriptor.fine_step() - descriptor.coarse_step(),
        "Down moves by the descriptor's coarse step"
    );
    state.apply(AppEvent::Adjust(Direction::Up)).unwrap();
    assert_eq!(
        focused_patch(&state).voice_limit().value(),
        start - descriptor.fine_step() - descriptor.coarse_step(),
        "growth stays pending until activation"
    );
    for event in support::topology_activation_events(state.engine_selection()) {
        state.apply(event).unwrap();
    }
    assert_eq!(
        focused_patch(&state).voice_limit().value(),
        start - descriptor.fine_step(),
        "Up moves back by the descriptor's coarse step"
    );

    // Down to the bound, then a typed unchanged rejection.
    while focused_patch(&state).voice_limit().value() > descriptor.minimum() {
        state
            .apply(AppEvent::Adjust(Direction::Left))
            .expect("the row edits down to its bound");
    }
    let at_bound = state.clone();
    assert_eq!(
        state.apply(AppEvent::Adjust(Direction::Left)),
        Err(EventRejection::ParameterAtBoundary),
        "the bound refuses rather than wrapping or clamping silently"
    );
    assert_eq!(state, at_bound, "a refused edit leaves the state identical");
}

/// The limit rides the parameter snapshot, per Patch.
fn check_the_voice_limit_rides_the_parameter_snapshot() {
    let state = lowered_limit_state(3);
    let snapshot = StateProjector::for_graph(GraphRevision::INITIAL)
        .project(&state)
        .expect("the state projects")
        .2;
    for patch in state.patches() {
        assert_eq!(
            snapshot
                .patch(patch.id())
                .expect("every installed Patch has a snapshot entry")
                .voice_limit(),
            patch.voice_limit(),
            "{:?}'s snapshot entry does not carry its canonical limit",
            patch.id()
        );
    }
    assert_eq!(
        snapshot
            .patch(PatchId::new(1).unwrap())
            .unwrap()
            .voice_limit()
            .value(),
        3
    );
    assert!(
        crest_synth::real_time::parameter_snapshot::ParameterSnapshot::serialized_leaf_descriptor()
            .contains(&"patches[].voiceLimit"),
        "the limit must be visible in the trace, or no measured proof can \
         correlate a refused note with the limit that refused it"
    );
}

/// The focused SoundFont Patch's limit driven down to `target` through the
/// canonical reducer.
fn lowered_limit_state(target: u16) -> AppState {
    let mut state = fixture_state();
    enter_utility_row(&mut state, &PatchControlId::VoiceLimit);
    set_mode(&mut state, InteractionMode::Adjust);
    while focused_patch(&state).voice_limit().value() > target {
        let remaining = focused_patch(&state).voice_limit().value() - target;
        let direction = if remaining >= VoiceLimit::descriptor().coarse_step() {
            Direction::Down
        } else {
            Direction::Left
        };
        state
            .apply(AppEvent::Adjust(direction))
            .expect("the limit row edits down to the target");
    }
    assert_eq!(focused_patch(&state).voice_limit().value(), target);
    set_mode(&mut state, InteractionMode::Navigate);
    state
}

/// A lock-free latest-wins observation transport that keeps the field this
/// target measures.
///
/// The shipped `AtomicAudioObservation` does **not** carry
/// `voiceLimitRefusals` — `AtomicObservationFields` has no such atomic — so a
/// count read back through it is zero no matter what the callback counted.
/// Reading the mission's central real-time claim through that adapter would be
/// exactly the failure this mission keeps finding: a guard walking a value it
/// cannot fail on. So the *counting* stays where production does it
/// (`AudioRenderer::render`) and only the transport is local, published the
/// same way the production one is: two relaxed atomic stores, no allocation,
/// no lock, no blocking.
///
/// That the production adapter drops the field is recorded as a finding rather
/// than fixed here: `src/adapter/atomic_audio_observation.rs` belongs to WP06,
/// which is the package that has to correlate a refused note with the limit
/// that refused it on real hardware.
#[derive(Clone, Default)]
struct RefusalObservation {
    refusals: std::sync::Arc<std::sync::atomic::AtomicU64>,
    active_notes: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

struct RefusalWriter {
    refusals: std::sync::Arc<std::sync::atomic::AtomicU64>,
    active_notes: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

struct RefusalReader {
    refusals: std::sync::Arc<std::sync::atomic::AtomicU64>,
    active_notes: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl AudioObservation for RefusalObservation {
    type CallbackHandle = RefusalWriter;
    type ControlHandle = RefusalReader;

    fn into_handles(self) -> (Self::CallbackHandle, Self::ControlHandle) {
        (
            RefusalWriter {
                refusals: std::sync::Arc::clone(&self.refusals),
                active_notes: std::sync::Arc::clone(&self.active_notes),
            },
            RefusalReader {
                refusals: self.refusals,
                active_notes: self.active_notes,
            },
        )
    }
}

impl crest_synth::real_time::audio_observation::CallbackAudioObservation for RefusalWriter {
    fn publish_from_callback(
        &mut self,
        snapshot: crest_synth::real_time::audio_observation_snapshot::AudioObservationSnapshot,
    ) {
        use std::sync::atomic::Ordering;
        self.refusals
            .store(snapshot.voice_limit_refusals(), Ordering::Relaxed);
        self.active_notes
            .store(u64::from(snapshot.active_notes()), Ordering::Relaxed);
    }
}

/// The trait the transport must satisfy. It hands back a snapshot carrying the
/// two fields this target measures; every other field is the default, which is
/// exactly what makes the reconstruction honest — nothing here fabricates a
/// measurement the callback did not publish.
impl ControlAudioObservation for RefusalReader {
    fn read_latest_on_control(
        &self,
    ) -> crest_synth::real_time::audio_observation_snapshot::AudioObservationSnapshot {
        crest_synth::real_time::audio_observation_snapshot::AudioObservationSnapshot::default()
            .with_voice_limit_refusals(self.refusals())
    }
}

impl RefusalReader {
    fn refusals(&self) -> u64 {
        self.refusals.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn active_notes(&self) -> u32 {
        self.active_notes.load(std::sync::atomic::Ordering::Relaxed) as u32
    }
}

/// Drives the production callback with `note_count` note-ons on the focused
/// Patch at a limit of `limit`, then a note-off and an all-notes-off, and
/// reports `(refusals, allocations, destructions, active_notes_after_onsets)`.
fn drive_callback_at_limit(limit: u16, note_count: u8) -> (u64, usize, usize, u32) {
    let state = lowered_limit_state(limit);
    let patch = focused_patch(&state).clone();
    let parameters = StateProjector::for_graph(GraphRevision::INITIAL)
        .project(&state)
        .expect("the state projects")
        .2;
    let graph = PreparedGraphBuilder::new(
        state.capabilities(),
        &production_instrument_preparers().expect("the production instrument preparers"),
    )
    .with_effects(
        state.effects(),
        &production_effect_preparers().expect("the production effect preparers"),
    )
    .build(
        GraphRevision::INITIAL,
        state.patches(),
        parameters.clone(),
        SAMPLE_RATE,
        BLOCK_FRAMES,
    )
    .expect("the production graph builds for the fixture");

    let (mut control, audio) = LockFreeAudioBoundary::new(64, parameters).into_handles();
    let (writer, reader) = RefusalObservation::default().into_handles();
    let mut renderer =
        AudioRenderer::with_observation(audio, NoStructuralGraphChanges::new(), graph, writer);
    let mut output = [0.0_f32; BLOCK_SAMPLES];

    let message = |kind, key, velocity| {
        MidiMessage::try_new(patch.channel(), kind, key, velocity).expect("a valid MIDI message")
    };
    for key in 0..note_count {
        control
            .push_command(AudioCommand::patch_midi(
                patch.id(),
                message(MidiMessageKind::NoteOn, 60 + key, 100),
            ))
            .expect("the command bank holds every note-on");
    }
    begin_memory_count();
    renderer.render(&mut output);
    let (mut allocations, mut destructions) = finish_memory_count();
    let active_after_onsets = reader.active_notes();

    // A note-off and an all-notes-off are never refused, at any limit.
    control
        .push_command(AudioCommand::patch_midi(
            patch.id(),
            message(MidiMessageKind::NoteOff, 60, 0),
        ))
        .unwrap();
    control.push_command(AudioCommand::all_notes_off()).unwrap();
    begin_memory_count();
    renderer.render(&mut output);
    let (more_allocations, more_destructions) = finish_memory_count();
    allocations += more_allocations;
    destructions += more_destructions;

    assert_eq!(reader.active_notes(), 0, "all-notes-off is never refused");
    (
        reader.refusals(),
        allocations,
        destructions,
        active_after_onsets,
    )
}

/// The callback refuses a note-on beyond the limit, never truncates a latched
/// voice, never refuses a note-off or all-notes-off, counts each refusal, and
/// allocates and destroys nothing.
///
/// **Falsified by defeating the limit.** With the refusal branch removed from
/// `AudioRenderer::render`'s `PatchMidi` arm, the same fixture reports zero
/// refusals and five active notes, and the two assertions below fire. That
/// mutation was performed and its failure text recorded; it is not described.
fn check_the_callback_enforces_the_limit() -> u64 {
    let limit = 3_u16;
    let notes = 5_u8;
    let (refusals, allocations, destructions, active) = drive_callback_at_limit(limit, notes);
    assert_eq!(
        active,
        u32::from(limit),
        "the Patch sounds exactly its limit: {active} voices latched under a limit of {limit}"
    );
    assert_eq!(
        refusals,
        u64::from(notes) - u64::from(limit),
        "every note-on beyond the limit is refused and counted once"
    );
    assert!(
        refusals > 0,
        "a fixture that must exceed the limit reported zero refusals — the limit does not bite"
    );
    assert_eq!(
        allocations, 0,
        "the callback allocated {allocations} time(s) enforcing the limit"
    );
    assert_eq!(
        destructions, 0,
        "the callback destroyed {destructions} object(s) enforcing the limit"
    );

    // At a limit the fixture cannot exceed, nothing is refused — so the count
    // is a measurement of the limit and not of the note count.
    let (none, ..) = drive_callback_at_limit(u16::from(notes) + 1, notes);
    assert_eq!(
        none, 0,
        "a fixture within its limit must report no refusal at all"
    );
    refusals
}

// ---------------------------------------------------------------------------
// Named tests, so a failure says which claim broke
// ---------------------------------------------------------------------------

#[test]
fn the_transcribed_page_rules_match_the_committed_script() {
    check_the_transcribed_page_rules_match_the_committed_script();
}

#[test]
fn the_fixture_spans_more_than_two_patches_across_both_engines() {
    check_the_fixture_spans_more_than_two_patches_across_both_engines();
}

#[test]
fn a_switch_reprojects_the_destination_in_one_generation() {
    check_a_switch_reprojects_the_destination_in_one_generation();
}

#[test]
fn focus_recovers_against_the_destination_schema() {
    check_focus_recovers_against_the_destination_schema();
}

#[test]
fn the_ends_of_the_installed_order_refuse() {
    check_the_ends_of_the_installed_order_refuse();
}

#[test]
fn an_in_flight_edit_stays_correlated_and_a_subordinate_surface_is_left() {
    check_an_in_flight_edit_stays_correlated_and_a_subordinate_surface_is_left();
}

#[test]
fn the_overview_is_projected_section_structure() {
    check_the_overview_is_projected_section_structure();
}

#[test]
fn an_unresolved_section_path_fails_the_overview_check() {
    check_an_unresolved_section_path_fails_the_overview_check();
}

#[test]
fn an_empty_slot_remains_an_explicit_root_control() {
    check_an_empty_slot_remains_an_explicit_root_control();
}

#[test]
fn the_whole_patch_surface_marks_nothing_unavailable() {
    check_the_whole_patch_surface_marks_nothing_unavailable();
}

#[test]
fn utility_resolves_five_typed_rows_and_its_hint_line() {
    check_utility_resolves_five_typed_rows_and_its_hint_line();
}

#[test]
fn one_master_gain_owner() {
    check_one_master_gain_owner();
}

#[test]
fn no_projected_screen_string_is_a_serialization_key() {
    check_no_projected_screen_string_is_a_serialization_key();
}

#[test]
fn the_midi_input_row_accepts_shared_subscriptions() {
    check_the_midi_input_row_accepts_shared_subscriptions();
}

#[test]
fn per_row_actions_agree_with_the_model_level_list() {
    check_per_row_actions_agree_with_the_model_level_list();
}

#[test]
fn requested_value_is_present_only_while_an_edit_is_in_flight() {
    check_requested_value_is_present_only_while_an_edit_is_in_flight();
}

#[test]
fn ranges_and_units_are_rendered() {
    check_ranges_and_units_are_rendered();
}

#[test]
fn the_page_composes_no_label_of_its_own() {
    check_the_page_composes_no_label_of_its_own();
}

#[test]
fn one_detail_identity_serves_two_subjects() {
    check_one_detail_identity_serves_two_subjects();
}

#[test]
fn two_slots_of_one_entry_are_distinct_subjects() {
    check_two_slots_of_one_entry_are_distinct_subjects();
}

#[test]
fn detail_entry_is_refused_where_no_subject_exists() {
    check_detail_entry_is_refused_where_no_subject_exists();
}

#[test]
fn subordinate_surfaces_do_not_nest() {
    check_subordinate_surfaces_do_not_nest();
}

#[test]
fn return_lands_on_the_exact_origin() {
    check_return_lands_on_the_exact_origin();
}

#[test]
fn a_read_only_section_is_marked_and_a_preparing_one_reports_itself() {
    check_a_read_only_section_is_marked_and_a_preparing_one_reports_itself();
}

#[test]
fn the_voice_limit_is_bounded_and_refuses_rather_than_clamps() {
    check_the_voice_limit_is_bounded_and_refuses_rather_than_clamps();
}

#[test]
fn each_patch_is_seeded_from_its_own_engine_default() {
    check_each_patch_is_seeded_from_its_own_engine_default();
}

#[test]
fn the_voice_limit_is_edited_as_a_stepped_control() {
    check_the_voice_limit_is_edited_as_a_stepped_control();
}

#[test]
fn the_voice_limit_rides_the_parameter_snapshot() {
    check_the_voice_limit_rides_the_parameter_snapshot();
}

#[test]
fn the_callback_enforces_the_limit() {
    check_the_callback_enforces_the_limit();
}

/// The declared acceptance target.
///
/// Every check above runs here, in declared order, and the marker
/// `validation.functional_patch_editor` asserts on is printed strictly after
/// the last of them returns. A failing check panics before the print, so the
/// marker cannot appear on a red run, and it is emitted from exactly one place.
#[test]
fn functional_patch_editor_acceptance() {
    let transcriptions = check_the_transcribed_page_rules_match_the_committed_script();

    // T029
    check_the_fixture_spans_more_than_two_patches_across_both_engines();
    let switches = check_a_switch_reprojects_the_destination_in_one_generation();
    check_focus_recovers_against_the_destination_schema();
    check_the_ends_of_the_installed_order_refuse();
    check_an_in_flight_edit_stays_correlated_and_a_subordinate_surface_is_left();

    // T030
    let overview_sections = check_the_overview_is_projected_section_structure();
    let unresolved_path_rejection = check_an_unresolved_section_path_fails_the_overview_check();
    check_an_empty_slot_remains_an_explicit_root_control();
    let patch_rows = check_the_whole_patch_surface_marks_nothing_unavailable();

    // T031
    check_utility_resolves_five_typed_rows_and_its_hint_line();
    check_one_master_gain_owner();
    let screen_strings = check_no_projected_screen_string_is_a_serialization_key();
    check_the_midi_input_row_accepts_shared_subscriptions();

    // T032
    let action_rows = check_per_row_actions_agree_with_the_model_level_list();
    check_requested_value_is_present_only_while_an_edit_is_in_flight();
    let ranges = check_ranges_and_units_are_rendered();
    let authored_strings = check_the_page_composes_no_label_of_its_own();

    // T033
    let (detail_surface_identities, detail_subjects_served) =
        check_one_detail_identity_serves_two_subjects();
    check_two_slots_of_one_entry_are_distinct_subjects();
    check_detail_entry_is_refused_where_no_subject_exists();
    check_subordinate_surfaces_do_not_nest();
    check_return_lands_on_the_exact_origin();
    check_a_read_only_section_is_marked_and_a_preparing_one_reports_itself();

    // T034
    check_the_voice_limit_is_bounded_and_refuses_rather_than_clamps();
    check_each_patch_is_seeded_from_its_own_engine_default();
    check_the_voice_limit_is_edited_as_a_stepped_control();
    check_the_voice_limit_rides_the_parameter_snapshot();
    let refusals = check_the_callback_enforces_the_limit();

    println!(
        "CREST_FUNCTIONAL_PATCH_EDITOR_OBSERVATION patches={} engines=2 switches={} \
         overview_sections={} patch_rows={} screen_strings={} action_rows={} ranges_painted={} \
         authored_strings={} detail_surface_identities={} detail_subjects_served={} \
         voice_limit_refusals={} page_rules_pinned={}",
        fixture_state().patches().len(),
        switches,
        overview_sections,
        patch_rows,
        screen_strings,
        action_rows,
        ranges,
        authored_strings,
        detail_surface_identities,
        detail_subjects_served,
        refusals,
        transcriptions,
    );
    println!("CREST_FUNCTIONAL_PATCH_EDITOR_UNRESOLVED_PATH_REJECTED {unresolved_path_rejection}");
    println!("{ACCEPTANCE_MARKER}");
}
