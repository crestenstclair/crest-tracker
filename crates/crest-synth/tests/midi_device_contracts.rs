use crest_synth::control::{
    MidiConnectionRevision, MidiInputDeviceId, MidiInputPreference, MidiPreferredInput,
    PhysicalMidiIngress, PhysicalMidiIngressOutcome, MIDI_INPUT_PREFERENCE_VERSION,
    PHYSICAL_MIDI_QUEUE_CAPACITY,
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};

thread_local! {
    static COUNT_CALLBACK_MEMORY: Cell<bool> = const { Cell::new(false) };
    static CALLBACK_ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    static CALLBACK_DEALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

struct CountingAllocator;

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        COUNT_CALLBACK_MEMORY.with(|enabled| {
            if enabled.get() {
                CALLBACK_ALLOCATIONS.with(|count| count.set(count.get() + 1));
            }
        });
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        COUNT_CALLBACK_MEMORY.with(|enabled| {
            if enabled.get() {
                CALLBACK_DEALLOCATIONS.with(|count| count.set(count.get() + 1));
            }
        });
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        COUNT_CALLBACK_MEMORY.with(|enabled| {
            if enabled.get() {
                CALLBACK_ALLOCATIONS.with(|count| count.set(count.get() + 1));
                CALLBACK_DEALLOCATIONS.with(|count| count.set(count.get() + 1));
            }
        });
        unsafe { System.realloc(pointer, layout, size) }
    }
}

fn count_callback_memory<T>(operation: impl FnOnce() -> T) -> (T, usize, usize) {
    CALLBACK_ALLOCATIONS.with(|count| count.set(0));
    CALLBACK_DEALLOCATIONS.with(|count| count.set(0));
    COUNT_CALLBACK_MEMORY.with(|enabled| enabled.set(true));
    let value = operation();
    COUNT_CALLBACK_MEMORY.with(|enabled| enabled.set(false));
    let allocations = CALLBACK_ALLOCATIONS.with(Cell::get);
    let deallocations = CALLBACK_DEALLOCATIONS.with(Cell::get);
    (value, allocations, deallocations)
}

fn rust_sources(root: &Path, output: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(root).expect("source directory must be readable") {
        let path = entry.expect("source entry must be readable").path();
        if path.is_dir() {
            rust_sources(&path, output);
        } else if path.extension().and_then(|value| value.to_str()) == Some("rs") {
            output.push(path);
        }
    }
}

#[test]
fn canonical_midi_device_concepts_have_one_public_declaration() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut paths = Vec::new();
    rust_sources(&source_root, &mut paths);
    let source = paths
        .iter()
        .map(|path| fs::read_to_string(path).expect("Rust source must be readable"))
        .collect::<Vec<_>>()
        .join("\n");

    for name in [
        "MidiInputDeviceId",
        "MidiInputDescriptor",
        "MidiInputPortFacts",
        "MidiInputScanId",
        "MidiConnectionRequestId",
        "MidiConnectionRevision",
        "MidiInputConnectionStatus",
        "MidiDeviceFailure",
        "ConnectMidiInput",
        "PhysicalMidiEvent",
        "MidiActivitySnapshot",
        "ActiveMidiInput",
        "MidiInputPreference",
        "MidiInputDevicePort",
        "MidiInputPreferencePort",
    ] {
        let declarations = source
            .lines()
            .filter(|line| {
                let line = line.trim_start();
                ["struct", "enum", "trait"].iter().any(|kind| {
                    let prefix = format!("pub {kind} {name}");
                    line.strip_prefix(&prefix).is_some_and(|rest| {
                        rest.chars()
                            .next()
                            .is_some_and(|character| matches!(character, ' ' | '(' | '{' | ':'))
                    })
                })
            })
            .count()
            + source.matches(&format!("monotonic_id!({name},")).count();
        assert_eq!(declarations, 1, "{name} must have one public declaration");
    }
}

#[test]
fn callback_values_and_public_ports_leak_no_owned_or_backend_type() {
    let source = include_str!("../src/control/midi_device.rs");
    let event = source
        .split("pub struct PhysicalMidiEvent")
        .nth(1)
        .unwrap()
        .split("impl PhysicalMidiEvent")
        .next()
        .unwrap();
    let diagnostics = source
        .split("pub struct MidiCallbackDiagnostics")
        .nth(1)
        .unwrap()
        .split("pub struct MidiActivitySnapshot")
        .next()
        .unwrap();
    let public_ports = source
        .split("pub trait MidiInputDevicePort")
        .nth(1)
        .unwrap()
        .split("#[cfg(test)]")
        .next()
        .unwrap();

    for forbidden in ["String", "Vec<", "Box<", "Arc<", "Mutex<", "midir::"] {
        assert!(!event.contains(forbidden), "event leaked {forbidden}");
        assert!(
            !diagnostics.contains(forbidden),
            "diagnostics leaked {forbidden}"
        );
    }
    for forbidden in ["midir::", "MidiInputPort", "MidiInputConnection"] {
        assert!(!public_ports.contains(forbidden), "port leaked {forbidden}");
    }
}

#[test]
fn preference_schema_is_exact_and_validated_on_round_trip() {
    let selected = MidiPreferredInput::new(
        MidiInputDeviceId::new("midir-v1", "coremidi:opaque-a").unwrap(),
        "Controller A",
    )
    .unwrap();
    let preference = MidiInputPreference::new(selected);
    let json = serde_json::to_value(&preference).unwrap();
    assert_eq!(json["version"], MIDI_INPUT_PREFERENCE_VERSION);
    assert_eq!(json["selectedInput"]["identitySchema"], "midir-v1");
    assert_eq!(json["selectedInput"]["identity"], "coremidi:opaque-a");
    assert_eq!(
        serde_json::from_value::<MidiInputPreference>(json).unwrap(),
        preference
    );
    assert!(serde_json::from_str::<MidiInputPreference>(
        r#"{"version":2,"selectedInput":{"identitySchema":"midir-v1","identity":"a","lastKnownDisplayName":"A"}}"#
    )
    .is_err());
}

#[test]
fn production_midi_lifecycle_mutation_is_confined_to_app_state_reduction() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut paths = Vec::new();
    rust_sources(&source_root, &mut paths);
    for path in paths {
        let source = fs::read_to_string(&path).expect("Rust source must be readable");
        if source.contains(".midi_input.set_") {
            assert!(
                path.ends_with("src/control/app_state.rs"),
                "reducer-owned MIDI lifecycle mutation leaked into {}",
                path.display()
            );
        }
    }
    assert!(include_str!("../src/control/app_state.rs").contains("midi_input: MidiInputState"));
    assert!(
        include_str!("../src/control/serialized_state.rs").contains("midi_input: MidiInputState")
    );
    let app_state = include_str!("../src/control/app_state.rs")
        .split("#[cfg(test)]")
        .next()
        .unwrap();
    assert!(app_state.contains("let mut next = self.clone();"));
    assert!(app_state.contains("let effects = next.reduce(event)?;"));
    assert!(app_state.contains("*self = next;"));
}

#[test]
fn application_callback_body_has_no_forbidden_operation_surface() {
    let source = include_str!("../src/control/midi_device.rs");
    let callback = source
        .split("pub fn receive_raw(")
        .nth(1)
        .unwrap()
        .split("fn normalize_raw_midi")
        .next()
        .unwrap();
    for forbidden in [
        "String",
        "Vec<",
        "Box<",
        "Mutex",
        ".lock(",
        "sleep(",
        "println!",
        "eprintln!",
        "format!",
        "write!",
        "panic!",
        ".unwrap(",
        "AppState",
        "Graphical",
        "Window",
    ] {
        assert!(
            !callback.contains(forbidden),
            "callback body exposes forbidden operation `{forbidden}`"
        );
    }
}

#[test]
fn committed_settings_renderer_is_projection_driven_revision_gated_and_fluid() {
    let page = include_str!("../webview-page/page.js");
    let css = include_str!("../webview-page/page.css");
    for required in [
        "AVAILABLE INPUTS",
        "ACTIVE INPUT",
        "UNKNOWN / NOT REPORTED",
        "data-midi-status",
        "data-marker",
        "snapshot.revision",
        "inspector.revision",
        "snapshot.acceptedCount",
        "message.kind",
        "message.channel",
        "message.data1",
        "message.data2",
        "observeMidiActivity",
    ] {
        assert!(page.contains(required), "renderer lost {required:?}");
    }
    for forbidden_fixture in ["Keystep", "Launchkey", "MPK Mini", "opaque-a", "opaque-b"] {
        assert!(
            !page.contains(forbidden_fixture),
            "renderer hardcodes fixture device {forbidden_fixture:?}"
        );
    }
    assert!(css.contains("body.settings-active #main-region"));
    assert!(css.contains("4fr"));
    assert!(css.contains("@container shell (inline-size < 45rem)"));
    assert!(!css.contains("aspect-ratio"));
    assert!(!css.contains("@media") || !css.contains("settings-wide"));
}

#[test]
fn physical_callback_supported_malformed_sysex_and_overflow_allocate_and_destroy_nothing() {
    let revision = MidiConnectionRevision::FIRST;
    let (mut ingress, control) = PhysicalMidiIngress::bounded(revision);
    control.enable();
    let large_sysex = vec![0xF0; 1_000_000];
    let (outcomes, allocations, deallocations) = count_callback_memory(|| {
        let supported = ingress.receive_raw(1, &[0x90, 60, 100]);
        let malformed = ingress.receive_raw(2, &[0x90, 60]);
        let sysex = ingress.receive_raw(3, &large_sysex);
        for timestamp in 0..(PHYSICAL_MIDI_QUEUE_CAPACITY - 1) {
            let _ = ingress.receive_raw(timestamp as u64 + 4, &[0x80, 60, 0]);
        }
        let overflow = ingress.receive_raw(2_000, &[0x80, 60, 0]);
        (supported, malformed, sysex, overflow)
    });
    assert_eq!(outcomes.0, PhysicalMidiIngressOutcome::Accepted);
    assert!(matches!(
        outcomes.1,
        PhysicalMidiIngressOutcome::Malformed(_)
    ));
    assert!(matches!(
        outcomes.2,
        PhysicalMidiIngressOutcome::Unsupported(_)
    ));
    assert_eq!(outcomes.3, PhysicalMidiIngressOutcome::CapacityFailure);
    assert_eq!((allocations, deallocations), (0, 0));
}
