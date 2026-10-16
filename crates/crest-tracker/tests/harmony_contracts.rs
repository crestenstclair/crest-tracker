use crest_tracker::harmony::{
    Chord, ChordKind, Direction, HarmonyError, Pitch, Scale, ScaleKind, Spelling,
};

fn pitches(values: &[u8]) -> Vec<Pitch> {
    values
        .iter()
        .map(|value| Pitch::new(*value).unwrap())
        .collect()
}

#[test]
fn c_minor_voicings_match_the_version_one_fixtures() {
    let chord = Chord::from_kind(ChordKind::Minor);
    let root = Pitch::new(60).unwrap();
    for (inversion, open, expected) in [
        (0, false, [60, 63, 67]),
        (1, false, [63, 67, 72]),
        (2, false, [67, 72, 75]),
        (0, true, [60, 67, 75]),
        (1, true, [63, 72, 79]),
        (2, true, [67, 75, 84]),
    ] {
        assert_eq!(
            chord.resolve(root, inversion, open, 0).unwrap(),
            pitches(&expected)
        );
    }
    assert_eq!(
        chord.resolve(root, 0, false, 1).unwrap(),
        pitches(&[60, 63, 67, 72])
    );
    assert_eq!(
        chord.resolve(root, 0, false, 3).unwrap(),
        pitches(&[60, 63, 67, 72, 75, 79])
    );
    assert_eq!(
        chord.resolve(root, 1, true, 2).unwrap(),
        pitches(&[63, 72, 79, 84, 87])
    );
}

#[test]
fn literal_major_stays_major_in_c_minor_and_display_preserves_pitch() {
    let scale = Scale::new(0, ScaleKind::NaturalMinor).unwrap();
    let notes = Chord::from_kind(ChordKind::Major)
        .resolve(Pitch::new(60).unwrap(), 0, false, 0)
        .unwrap();
    assert_eq!(notes, pitches(&[60, 64, 67]));
    assert!(!scale.contains(notes[1]));
    assert_eq!(scale.pitch_name(Pitch::new(63).unwrap()), "Eb4");
    assert_eq!(scale.interval_name(notes[1]), "3");
    assert_eq!(notes[1].midi(), 64);
    assert_eq!(Pitch::new(0).unwrap().name(Spelling::Sharps), "C-1");
    assert_eq!(Pitch::new(127).unwrap().name(Spelling::Sharps), "G9");
}

#[test]
fn all_default_chords_fit_each_root_of_every_catalog_scale() {
    for kind in ScaleKind::CATALOG {
        verify_catalog_roots(kind);
    }
    let unusual = Scale::custom(0, vec![0, 1, 6]).unwrap();
    verify_default_bank(&unusual);
    assert!(unusual
        .default_chords()
        .iter()
        .all(|entry| entry.chord.name().starts_with("Intervals")));
    verify_default_bank(&Scale::custom(0, vec![0]).unwrap());
    verify_default_bank(&Scale::custom(0, vec![0, 1]).unwrap());
    verify_default_bank(&Scale::custom(0, vec![0, 1, 6, 11]).unwrap());
}

fn verify_catalog_roots(kind: ScaleKind) {
    for root in 0..12 {
        verify_default_bank(&Scale::new(root, kind).unwrap());
    }
}

fn verify_default_bank(scale: &Scale) {
    let bank = scale.default_chords();
    assert!(!bank.is_empty());
    for entry in bank {
        let root = Pitch::new(60 + entry.root).unwrap();
        assert!(entry
            .chord
            .resolve(root, 0, false, 0)
            .unwrap()
            .iter()
            .all(|pitch| scale.contains(*pitch)));
    }
}

#[test]
fn stepping_respects_scale_and_midi_boundaries() {
    let scale = Scale::new(0, ScaleKind::Major).unwrap();
    assert_eq!(
        scale.next_pitch(Pitch::new(60).unwrap(), Direction::Up),
        Some(Pitch::new(62).unwrap())
    );
    assert_eq!(
        scale.next_pitch(Pitch::new(61).unwrap(), Direction::Down),
        Some(Pitch::new(60).unwrap())
    );
    assert_eq!(
        scale.next_pitch(Pitch::new(0).unwrap(), Direction::Down),
        None
    );
    assert_eq!(
        scale.next_pitch(Pitch::new(127).unwrap(), Direction::Up),
        None
    );
    assert!(Pitch::new(0).unwrap().transpose(-1).is_err());
    assert!(Pitch::new(127).unwrap().transpose(1).is_err());
}

#[test]
fn voicing_failures_are_explicit_without_partial_or_clipped_output() {
    let chord = Chord::from_kind(ChordKind::Minor);
    assert!(matches!(
        chord.resolve(Pitch::new(125).unwrap(), 0, false, 0),
        Err(HarmonyError::PitchOutOfRange(_))
    ));
    assert!(matches!(
        chord.resolve(Pitch::new(60).unwrap(), 3, false, 0),
        Err(HarmonyError::InvalidInversion { .. })
    ));
    assert_eq!(
        chord.resolve(Pitch::new(120).unwrap(), 0, false, 1),
        Err(HarmonyError::SpreadExhausted {
            requested: 1,
            available: 0
        })
    );
    assert!(chord
        .resolve(Pitch::new(60).unwrap(), 0, false, usize::MAX)
        .is_err());
    assert_eq!(
        Chord::new(vec![0, 7])
            .unwrap()
            .resolve(Pitch::new(60).unwrap(), 0, true, 0),
        Err(HarmonyError::OpenNeedsThreeVoices)
    );
    assert_eq!(
        Chord::new(vec![-12, 0, 12])
            .unwrap()
            .resolve(Pitch::new(60).unwrap(), 0, true, 0),
        Err(HarmonyError::DuplicateVoice(72))
    );
}

#[test]
fn serde_rejects_invalid_pitch_scale_and_chord_invariants() {
    assert!(serde_json::from_str::<Pitch>("128").is_err());
    for invalid in ["[]", "[3,7]", "[0,0,7]", "[0,13]", "[-13,0]"] {
        assert!(serde_json::from_str::<Chord>(invalid).is_err());
    }
    for invalid in [
        r#"{"root":12,"kind":"custom","pitch_classes":[0],"spelling":"sharps"}"#,
        r#"{"root":0,"kind":"custom","pitch_classes":[0,0],"spelling":"sharps"}"#,
        r#"{"root":0,"kind":"major","pitch_classes":[0,3,7],"spelling":"flats"}"#,
        r#"{"root":0,"kind":"custom","pitch_classes":[1,7],"spelling":"flats"}"#,
    ] {
        assert!(serde_json::from_str::<Scale>(invalid).is_err());
    }
}

#[test]
fn serialized_shape_recall_applies_voicing_once_and_never_mutates_the_shape() {
    let original = Chord::from_kind(ChordKind::Minor);
    let root = Pitch::new(60).unwrap();
    let expected = original.resolve(root, 2, true, 2).unwrap();
    let saved = serde_json::to_string(&original).unwrap();
    let recalled: Chord = serde_json::from_str(&saved).unwrap();
    assert_eq!(recalled.resolve(root, 2, true, 2).unwrap(), expected);
    assert_eq!(recalled.resolve(root, 2, true, 2).unwrap(), expected);
    assert_eq!(recalled.intervals(), &[0, 3, 7]);
    let scale = Scale::new(5, ScaleKind::Dorian)
        .unwrap()
        .with_spelling(Spelling::Flats);
    assert_eq!(
        serde_json::from_str::<Scale>(&serde_json::to_string(&scale).unwrap()).unwrap(),
        scale
    );
}
