//! Compatibility with independent, immutable library consumers and unsupported
//! parameter preservation at the Tracker boundary.
use crest_linnstrument::Geometry;
use crest_performance_core::{
    self as library, BeatTime, Command, DestinationId, Gesture, Limits, MorphParameter,
    ParameterAddress, Session, SlotState, SurfaceMode,
};
use crest_tracker::{
    composition::InstrumentId,
    performance::{self, Document, Performance, Route},
    time::Beat,
};

fn document(session: Session) -> Document {
    Document {
        version: 1,
        routes: session
            .tracks
            .iter()
            .map(|track| Route {
                destination: track.instrument.destination,
                instrument: InstrumentId::new(u64::from(track.instrument.destination.0)).unwrap(),
            })
            .collect(),
        session,
    }
}

#[test]
fn schema8_preserves_every_field_and_matches_the_api10_consumers_ordered_trace() {
    let legacy: Session =
        serde_json::from_str(include_str!("fixtures/performance-schema8.json")).unwrap();
    assert_eq!(legacy.version, 8);
    let expected: Vec<library::OutputEvent> =
        serde_json::from_str(include_str!("fixtures/performance-schema8-playback.json")).unwrap();
    let mut engine = Performance::new(
        document(legacy.clone()),
        Geometry::Pads200,
        Limits::default(),
    )
    .unwrap();
    let saved = engine.document().unwrap();
    let mut upgraded = legacy;
    upgraded.version = library::SESSION_VERSION;
    assert_eq!(saved.session, upgraded);
    assert_eq!(saved.clone().migrate().unwrap(), saved);
    engine.command(Command::SetFill(true)).unwrap();
    for slot in 0..3 {
        engine
            .command(Command::Launch {
                slot,
                quantum: BeatTime(0),
            })
            .unwrap();
    }
    engine
        .advance(performance::tracker_time(BeatTime(4 * library::TICKS_PER_BEAT - 1)).unwrap())
        .unwrap();
    engine.command(Command::Stop).unwrap();
    let actual: Vec<_> = engine.drain_output().collect();
    assert_eq!(actual, expected);
    assert_eq!(actual.len(), 164);
    assert_eq!(
        actual
            .iter()
            .filter(|e| matches!(e.gesture, Gesture::On { .. }))
            .count(),
        49
    );
    assert_eq!(
        actual
            .iter()
            .filter(|e| matches!(e.gesture, Gesture::Off { .. }))
            .count(),
        49
    );
    assert_eq!(engine.snapshot().active_voices, 0);
}

fn parameter_document() -> Document {
    let session: Session =
        serde_json::from_str(include_str!("fixtures/performance-schema2.json")).unwrap();
    let mut document = document(session).migrate().unwrap();
    let clip = document.session.slots[0].clip.as_ref().unwrap().clone();
    // A later lock must reject the whole launch, including earlier unlocked notes.
    let note = clip.notes().unwrap().last().unwrap().id;
    document.session.slots[1].clip = Some(clip.clone());
    document.session.slots[0].clip = Some(clip.with_note_parameter(note, 4, Some(54_321)).unwrap());
    document.session.scene_morph = Some(
        vec![MorphParameter {
            address: ParameterAddress {
                destination: DestinationId(1),
                parameter: 7,
            },
            a: 0,
            b: u16::MAX,
        }]
        .try_into()
        .unwrap(),
    );
    document
}

#[test]
fn unsupported_parameters_roundtrip_but_reject_before_launch_without_affecting_other_clips() {
    let original = parameter_document();
    let serialized = serde_json::to_vec(&original).unwrap();
    let reopened = serde_json::from_slice(&serialized).unwrap();
    let mut engine = Performance::new(reopened, Geometry::Pads200, Limits::default()).unwrap();
    assert_eq!(engine.document().unwrap(), original);
    engine
        .command(Command::Launch {
            slot: 1,
            quantum: BeatTime(0),
        })
        .unwrap();
    engine
        .advance(performance::tracker_time(BeatTime(30_000)).unwrap())
        .unwrap();
    let sounding = engine.snapshot().active_voices;
    assert!(sounding > 0);
    engine.drain_output().for_each(drop);
    let error = engine
        .command(Command::Launch {
            slot: 0,
            quantum: BeatTime(library::TICKS_PER_BEAT),
        })
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("does not support per-note parameter locks"));
    assert_eq!(engine.snapshot().slots[0], SlotState::Recorded);
    assert_eq!(engine.slot_launch_time(0).unwrap(), None);
    assert_eq!(engine.snapshot().slots[1], SlotState::Playing);
    assert_eq!(engine.snapshot().active_voices, sounding);
    assert_eq!(engine.drain_output().count(), 0);
    let error = engine
        .command(Command::SetMorphPosition(32_768))
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("does not support a scene parameter"));
    assert_eq!(engine.document().unwrap(), original);
    engine.command(Command::Stop).unwrap();
    assert_eq!(engine.snapshot().active_voices, 0);
}

#[test]
fn firmware_user_pad_cannot_bypass_parameter_capabilities() {
    let original = parameter_document();
    let mut engine =
        Performance::new(original.clone(), Geometry::Pads200, Limits::default()).unwrap();
    engine
        .command(Command::SetMode(SurfaceMode::Clips))
        .unwrap();
    let error = engine.accept_midi(Beat::ZERO, &[0x90, 1, 127]).unwrap_err();
    assert!(error
        .to_string()
        .contains("does not support per-note parameter locks"));
    assert_eq!(engine.snapshot().slots[0], SlotState::Recorded);
    assert_eq!(engine.snapshot().active_voices, 0);
    assert_eq!(engine.drain_output().count(), 0);
    assert_eq!(engine.document().unwrap(), original);
    engine.accept_midi(Beat::ZERO, &[0x90, 2, 127]).unwrap();
    engine
        .advance(performance::tracker_time(BeatTime(30_000)).unwrap())
        .unwrap();
    assert_eq!(engine.snapshot().slots[1], SlotState::Playing);
    assert!(engine
        .drain_output()
        .any(|e| matches!(e.gesture, Gesture::On { .. })));
    engine.command(Command::Stop).unwrap();
    assert_eq!(engine.snapshot().active_voices, 0);
}
