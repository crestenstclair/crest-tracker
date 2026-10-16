use crest_tracker::composition::*;
use crest_tracker::editing::Editor;
use crest_tracker::time::{Beat, Meter};

fn linked_song() -> Song {
    let mut song = Song::default();
    let instrument = InstrumentId::new(17).unwrap();
    let phrase = PhraseId::new(18).unwrap();
    let chain = ChainId::new(20).unwrap();
    let mut note = Note::new(60, 101, Beat::new(5, 6).unwrap()).unwrap();
    note.release_velocity = 37;
    note.expression = vec![
        Expression {
            sequence: None,
            offset: Beat::ZERO,
            control: Control::Bend { value: 8193 },
        },
        Expression {
            sequence: None,
            offset: Beat::new(2, 3).unwrap(),
            control: Control::Pressure { value: 112 },
        },
    ];
    song.instruments.insert(
        instrument,
        InstrumentUse {
            name: "Lead".into(),
            resource: None,
        },
    );
    song.phrases.insert(
        phrase,
        Phrase {
            events: vec![Event::note(
                EventId::new(19).unwrap(),
                Beat::new(1, 3).unwrap(),
                Target::instrument(instrument),
                note,
            )],
            ..Phrase::default()
        },
    );
    song.chains.insert(
        chain,
        Chain {
            name: "A".into(),
            phrases: vec![PhraseUse {
                id: UseId::new(21).unwrap(),
                phrase,
                repeats: 2,
            }],
        },
    );
    song.tracks[0].slots.push(Some(ChainUse {
        id: UseId::new(22).unwrap(),
        chain,
        repeats: 3,
    }));
    song.validate().unwrap();
    song
}

fn first_phrase(song: &mut Song) -> &mut Phrase {
    song.phrases.values_mut().next().unwrap()
}

#[test]
fn portable_round_trip_preserves_raw_expression_and_distinct_instrument_uses() {
    let mut song = linked_song();
    let resource = ResourceRevision {
        resource: "bank/lead".into(),
        revision: "immutable-revision-a".into(),
    };
    song.instruments.values_mut().next().unwrap().resource = Some(resource.clone());
    song.instruments.insert(
        InstrumentId::new(23).unwrap(),
        InstrumentUse {
            name: "Second lead".into(),
            resource: Some(resource),
        },
    );
    let decoded: Song = serde_json::from_str(&serde_json::to_string(&song).unwrap()).unwrap();
    decoded.validate().unwrap();
    assert_eq!(decoded, song);
    assert_eq!(decoded.instruments.len(), 2);
    assert_eq!(decoded.next_identity().unwrap(), 24);
}

#[test]
fn missing_graph_and_target_references_never_enter_an_editor() {
    let mut missing_chain = linked_song();
    missing_chain.chains.clear();
    assert!(matches!(
        Editor::new(missing_chain),
        Err(DocumentError::Missing {
            kind: "chain",
            id: 20
        })
    ));
    let mut missing_phrase = linked_song();
    missing_phrase.phrases.clear();
    assert!(matches!(
        Editor::new(missing_phrase),
        Err(DocumentError::Missing {
            kind: "phrase",
            id: 18
        })
    ));
    let mut missing_instrument = linked_song();
    missing_instrument.instruments.clear();
    assert!(matches!(
        Editor::new(missing_instrument),
        Err(DocumentError::Missing {
            kind: "instrument",
            id: 17
        })
    ));
}

#[test]
fn duplicate_stable_ids_and_zero_repeats_are_rejected() {
    let mut song = linked_song();
    song.tracks[1].id = song.tracks[0].id;
    assert!(song.validate().is_err());
    let mut song = linked_song();
    song.chains.values_mut().next().unwrap().phrases[0].id = UseId::new(22).unwrap();
    assert!(song.validate().is_err());
    let mut song = linked_song();
    let event = first_phrase(&mut song).events[0].clone();
    first_phrase(&mut song).events.push(event);
    assert!(song.validate().is_err());
    let mut song = linked_song();
    song.chains.values_mut().next().unwrap().phrases[0].repeats = 0;
    assert!(song.validate().is_err());
}

#[test]
fn malformed_serialized_values_cannot_cross_the_document_boundary() {
    assert!(serde_json::from_str::<EventId>("0").is_err());
    assert!(serde_json::from_str::<UseId>("0").is_err());
    assert!(serde_json::from_str::<Channel>("0").is_err());
    assert!(serde_json::from_str::<Channel>("17").is_err());
    let mut encoded = serde_json::to_value(linked_song()).unwrap();
    encoded["unexpected"] = true.into();
    assert!(serde_json::from_value::<Song>(encoded).is_err());
    let mut encoded = serde_json::to_value(linked_song()).unwrap();
    encoded["phrases"]["18"]["events"][0]["note"]["pitch"] = 128.into();
    let decoded = serde_json::from_value::<Song>(encoded).unwrap();
    assert!(Editor::new(decoded).is_err());
}

#[test]
fn notes_controls_expression_and_event_positions_obey_their_ranges() {
    let mut song = linked_song();
    first_phrase(&mut song).events[0].at = Beat::integer(4);
    assert!(song.validate().is_err());
    let mut song = linked_song();
    first_phrase(&mut song).events[0]
        .note
        .as_mut()
        .unwrap()
        .expression[0]
        .control = Control::Bend { value: 16384 };
    assert!(song.validate().is_err());
    let mut song = linked_song();
    first_phrase(&mut song).events[0]
        .note
        .as_mut()
        .unwrap()
        .expression[0]
        .offset = Beat::ONE;
    assert!(song.validate().is_err());
    let mut song = linked_song();
    first_phrase(&mut song).events[0].target = Target::Channel {
        channel: Channel::FIRST,
        endpoint: Some(String::new()),
    };
    assert!(song.validate().is_err());
    let mut song = linked_song();
    song.tracks[0].id = TrackId::new(u64::MAX).unwrap();
    assert!(song.next_identity().is_err());
}

#[test]
fn meter_changes_follow_bar_boundaries_of_the_preceding_meter() {
    let mut song = linked_song();
    song.meters.push(MeterChange {
        at: Beat::integer(4),
        meter: Meter::new(3, 4).unwrap(),
    });
    song.meters.push(MeterChange {
        at: Beat::integer(10),
        meter: Meter::COMMON,
    });
    song.validate().unwrap();
    song.meters[2].at = Beat::integer(8);
    assert!(
        song.validate().is_err(),
        "beat 8 is not a bar boundary after 3/4 begins at beat 4"
    );
}

#[test]
fn literal_chord_fx_use_the_same_validated_intervals_as_chord_shapes() {
    for intervals in [vec![0, 0, 7], vec![-13, 0], vec![0, 13]] {
        let mut song = linked_song();
        first_phrase(&mut song).events[0].fx[0] = Some(Fx::Chord { intervals });
        assert!(
            song.validate().is_err(),
            "invalid chord offsets must not survive document validation"
        );
    }
}

#[test]
fn random_generator_rejects_an_empty_scale_constrained_pitch_range() {
    let mut song = linked_song();
    first_phrase(&mut song).form = PhraseForm::Random {
        seed: 1,
        vary: true,
        density: 100,
        low: 61,
        high: 61,
        velocity_low: 80,
        velocity_high: 100,
        gate: Beat::new(1, 4).unwrap(),
        gate_high: None,
        target: Target::instrument(InstrumentId::new(17).unwrap()),
    };
    assert!(
        song.validate().is_err(),
        "C# is absent from the default key, so this generator has no candidates"
    );
}
