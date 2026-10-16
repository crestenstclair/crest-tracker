use crest_tracker::composition::{Channel, Control, InstrumentId, Target};
use crest_tracker::mpe::{
    BendRange, MpeDecoder, MpeDecoderCheckpoint, MpeError, MpeEvent, MpeProfile, MpeProvenance,
    MpeZone,
};
use crest_tracker::recording::CaptureMessage;
use crest_tracker::time::Beat;
use serde_json::{json, Value};

fn channel(value: u8) -> Channel {
    Channel::new(value).unwrap()
}
fn profile() -> MpeProfile {
    MpeProfile::new(
        "midi:source:exact".into(),
        MpeZone::lower(3).unwrap(),
        BendRange::new(48, 0).unwrap(),
        BendRange::new(2, 0).unwrap(),
    )
    .unwrap()
}
fn decoder() -> MpeDecoder {
    MpeDecoder::new(profile(), 1000).unwrap()
}
fn target() -> Target {
    Target::instrument(InstrumentId::new(17).unwrap())
}
fn cc(number: u8, value: u8) -> Control {
    Control::Cc { number, value }
}
fn input(
    decoder: &mut MpeDecoder,
    at: u64,
    member: u8,
    message: CaptureMessage,
) -> Result<Vec<MpeEvent>, MpeError> {
    decoder.input(Beat::integer(at), channel(member), message, target())
}
fn control(decoder: &mut MpeDecoder, at: u64, member: u8, control: Control) -> Vec<MpeEvent> {
    input(decoder, at, member, CaptureMessage::Control(control)).unwrap()
}
fn attack(decoder: &mut MpeDecoder, at: u64, member: u8, pitch: u8) -> Vec<MpeEvent> {
    input(
        decoder,
        at,
        member,
        CaptureMessage::NoteOn {
            pitch,
            velocity: 100,
        },
    )
    .unwrap()
}
fn release(
    decoder: &mut MpeDecoder,
    at: u64,
    member: u8,
    pitch: u8,
    velocity: u8,
) -> Vec<MpeEvent> {
    input(
        decoder,
        at,
        member,
        CaptureMessage::NoteOff { pitch, velocity },
    )
    .unwrap()
}
fn expression(id: u64, at: u64, master: bool, control: Control) -> MpeEvent {
    MpeEvent::Expression {
        id,
        at: Beat::integer(at),
        master,
        control,
    }
}
fn range(semitones: u8, cents: u8) -> Vec<Control> {
    vec![
        cc(101, 0),
        cc(100, 0),
        cc(6, semitones),
        cc(38, cents),
        cc(101, 127),
        cc(100, 127),
    ]
}
fn snapshot(decoder: &MpeDecoder) -> Value {
    serde_json::to_value(decoder.checkpoint()).unwrap()
}

#[test]
fn source_types_validate_construction_deserialization_and_zone_membership() {
    assert!(BendRange::new(128, 0).is_err());
    assert!(BendRange::new(0, 100).is_err());
    assert_eq!(BendRange::new(127, 99).unwrap().cents(), 99);
    assert!(MpeZone::lower(0).is_err());
    assert!(MpeZone::upper(16).is_err());
    let lower = MpeZone::lower(3).unwrap();
    assert_eq!(lower.master(), channel(1));
    assert!(lower.contains_member(channel(2)));
    assert!(lower.contains_member(channel(4)));
    assert!(!lower.contains_member(channel(1)));
    assert!(!lower.contains_member(channel(5)));
    let upper = MpeZone::upper(2).unwrap();
    assert_eq!(upper.master(), channel(16));
    assert!(upper.contains_member(channel(14)));
    assert!(upper.contains_member(channel(15)));
    assert!(!upper.contains_member(channel(13)));
    assert!(!upper.contains_member(channel(16)));
    assert!(MpeProfile::new(
        " ".into(),
        lower,
        BendRange::new(0, 0).unwrap(),
        BendRange::new(0, 0).unwrap()
    )
    .is_err());
    assert!(MpeProvenance::new(profile(), channel(1)).is_err());
    let provenance = MpeProvenance::new(profile(), channel(3)).unwrap();
    assert_eq!(provenance.profile().source(), "midi:source:exact");
    assert_eq!(provenance.member(), channel(3));
    assert_eq!(
        serde_json::from_value::<MpeProvenance>(serde_json::to_value(&provenance).unwrap())
            .unwrap(),
        provenance
    );
    assert!(serde_json::from_value::<BendRange>(json!({"semitones":128,"cents":0})).is_err());
    assert!(serde_json::from_value::<MpeZone>(json!({"kind":"lower","members":0})).is_err());
    let mut invalid = serde_json::to_value(provenance).unwrap();
    invalid["member"] = json!(16);
    assert!(serde_json::from_value::<MpeProvenance>(invalid).is_err());
    let mut invalid = serde_json::to_value(profile()).unwrap();
    invalid["source"] = json!("bad\0source");
    assert!(serde_json::from_value::<MpeProfile>(invalid).is_err());
}

#[test]
fn attack_initialization_preserves_raw_pre_attack_values_and_scope_order() {
    let mut decoder = decoder();
    for (member, value) in [
        (2, Control::Bend { value: 8231 }),
        (2, Control::Pressure { value: 87 }),
        (2, cc(74, 91)),
        (1, Control::Bend { value: 8371 }),
        (1, Control::Pressure { value: 99 }),
        (1, cc(64, 127)),
    ] {
        assert!(control(&mut decoder, 0, member, value).is_empty());
    }
    let events = attack(&mut decoder, 1, 2, 60);
    let MpeEvent::Attack {
        id,
        at,
        target: actual_target,
        pitch,
        velocity,
        provenance,
        member,
        master,
    } = &events[0]
    else {
        panic!("attack expected")
    };
    assert_eq!((*id, *at, *pitch, *velocity), (1, Beat::ONE, 60, 100));
    assert_eq!(actual_target, &target());
    assert_eq!(
        provenance,
        &MpeProvenance::new(profile(), channel(2)).unwrap()
    );
    let mut expected = range(48, 0);
    expected.extend([
        Control::Bend { value: 8231 },
        Control::Pressure { value: 87 },
        cc(74, 91),
    ]);
    assert_eq!(member, &expected);
    let mut expected = range(2, 0);
    expected.extend([
        Control::Bend { value: 8371 },
        Control::Pressure { value: 99 },
        cc(64, 127),
    ]);
    assert_eq!(master, &expected);
    assert_eq!(decoder.active_count(), 1);
    assert_eq!(decoder.profile(), &profile());
}

#[test]
fn unobserved_pressure_is_absent_instead_of_invented_zero() {
    let events = attack(&mut decoder(), 0, 2, 60);
    let MpeEvent::Attack { member, master, .. } = &events[0] else {
        panic!("attack expected")
    };
    let mut expected = range(48, 0);
    expected.extend([Control::Bend { value: 8192 }, cc(74, 64)]);
    assert_eq!(member, &expected);
    let mut expected = range(2, 0);
    expected.extend([Control::Bend { value: 8192 }, cc(64, 0)]);
    assert_eq!(master, &expected);

    let mut observed = decoder();
    control(&mut observed, 0, 2, Control::Pressure { value: 0 });
    let events = attack(&mut observed, 0, 2, 60);
    let MpeEvent::Attack { member, .. } = &events[0] else {
        panic!("attack expected")
    };
    assert!(member.contains(&Control::Pressure { value: 0 }));
}

#[test]
fn same_pitch_voices_on_different_members_keep_independent_curves_and_releases() {
    let mut decoder = decoder();
    attack(&mut decoder, 0, 2, 60);
    attack(&mut decoder, 0, 3, 60);
    assert_eq!(
        control(&mut decoder, 1, 2, Control::Bend { value: 8193 }),
        vec![expression(1, 1, false, Control::Bend { value: 8193 })]
    );
    assert_eq!(
        control(&mut decoder, 1, 3, Control::Pressure { value: 73 }),
        vec![expression(2, 1, false, Control::Pressure { value: 73 })]
    );
    assert_eq!(
        control(&mut decoder, 2, 1, Control::Bend { value: 8111 }),
        vec![
            expression(1, 2, true, Control::Bend { value: 8111 }),
            expression(2, 2, true, Control::Bend { value: 8111 })
        ]
    );
    assert_eq!(
        release(&mut decoder, 3, 2, 60, 43),
        vec![
            MpeEvent::Release {
                id: 1,
                at: Beat::integer(3),
                velocity: 43
            },
            MpeEvent::End {
                id: 1,
                at: Beat::integer(3)
            }
        ]
    );
    assert_eq!(
        control(&mut decoder, 4, 1, Control::Pressure { value: 64 }),
        vec![expression(2, 4, true, Control::Pressure { value: 64 })]
    );
    assert_eq!(decoder.active_count(), 1);
}

#[test]
fn sustain_retains_old_member_voice_while_new_member_expression_follows_reuse() {
    let mut decoder = decoder();
    control(&mut decoder, 0, 1, cc(64, 127));
    attack(&mut decoder, 1, 2, 60);
    assert_eq!(
        release(&mut decoder, 2, 2, 60, 61),
        vec![MpeEvent::Release {
            id: 1,
            at: Beat::integer(2),
            velocity: 61
        }]
    );
    assert_eq!(
        control(&mut decoder, 3, 2, Control::Bend { value: 8231 }),
        vec![expression(1, 3, false, Control::Bend { value: 8231 })]
    );
    let replacement = attack(&mut decoder, 4, 2, 60);
    assert!(matches!(
        replacement.as_slice(),
        [MpeEvent::Attack { id: 2, .. }]
    ));
    assert_eq!(
        control(&mut decoder, 5, 2, Control::Pressure { value: 97 }),
        vec![expression(2, 5, false, Control::Pressure { value: 97 })]
    );
    assert_eq!(
        control(&mut decoder, 6, 1, Control::Bend { value: 8123 }),
        vec![
            expression(1, 6, true, Control::Bend { value: 8123 }),
            expression(2, 6, true, Control::Bend { value: 8123 })
        ]
    );
    assert_eq!(
        control(&mut decoder, 7, 1, cc(64, 0)),
        vec![
            expression(1, 7, true, cc(64, 0)),
            expression(2, 7, true, cc(64, 0)),
            MpeEvent::End {
                id: 1,
                at: Beat::integer(7)
            }
        ]
    );
    assert_eq!(decoder.active_count(), 1);
    assert_eq!(
        release(&mut decoder, 8, 2, 60, 49),
        vec![
            MpeEvent::Release {
                id: 2,
                at: Beat::integer(8),
                velocity: 49
            },
            MpeEvent::End {
                id: 2,
                at: Beat::integer(8)
            }
        ]
    );
    assert_eq!(decoder.active_count(), 0);
}

#[test]
fn repeated_attacks_unmatched_releases_and_unsupported_controls_reject_atomically() {
    let mut decoder = decoder();
    attack(&mut decoder, 1, 2, 60);
    let before = snapshot(&decoder);
    for (member, message) in [
        (
            2,
            CaptureMessage::NoteOn {
                pitch: 61,
                velocity: 99,
            },
        ),
        (
            2,
            CaptureMessage::NoteOff {
                pitch: 61,
                velocity: 23,
            },
        ),
        (
            3,
            CaptureMessage::NoteOff {
                pitch: 60,
                velocity: 23,
            },
        ),
        (
            1,
            CaptureMessage::NoteOn {
                pitch: 60,
                velocity: 99,
            },
        ),
        (2, CaptureMessage::Control(cc(64, 127))),
        (1, CaptureMessage::Control(cc(74, 90))),
        (2, CaptureMessage::Control(cc(99, 0))),
        (2, CaptureMessage::Control(cc(98, 0))),
        (2, CaptureMessage::Control(Control::Program { value: 1 })),
        (
            2,
            CaptureMessage::Control(Control::PolyPressure {
                pitch: 60,
                value: 99,
            }),
        ),
        (2, CaptureMessage::Control(Control::Bend { value: 16384 })),
        (
            2,
            CaptureMessage::NoteOff {
                pitch: 60,
                velocity: 128,
            },
        ),
    ] {
        assert!(input(&mut decoder, 2, member, message).is_err());
        assert_eq!(snapshot(&decoder), before);
    }
    assert_eq!(
        input(
            &mut decoder,
            2,
            2,
            CaptureMessage::NoteOn {
                pitch: 60,
                velocity: 0
            }
        )
        .unwrap(),
        vec![
            MpeEvent::Release {
                id: 1,
                at: Beat::integer(2),
                velocity: 0
            },
            MpeEvent::End {
                id: 1,
                at: Beat::integer(2)
            }
        ]
    );
    let before = snapshot(&decoder);
    assert!(input(
        &mut decoder,
        3,
        2,
        CaptureMessage::NoteOff {
            pitch: 60,
            velocity: 0
        }
    )
    .is_err());
    assert_eq!(snapshot(&decoder), before);
}

#[test]
fn rpn_ranges_are_scoped_cached_and_replayed_as_complete_transactions() {
    let mut decoder = decoder();
    for (member, semitones, cents) in [(2, 12, 37), (1, 4, 25)] {
        for value in [cc(101, 0), cc(100, 0), cc(6, semitones), cc(38, cents)] {
            assert!(control(&mut decoder, 0, member, value).is_empty());
        }
    }
    let events = attack(&mut decoder, 1, 2, 60);
    let MpeEvent::Attack { member, master, .. } = &events[0] else {
        panic!("attack expected")
    };
    assert_eq!(&member[..6], range(12, 37));
    assert_eq!(&master[..6], range(4, 25));
    // The incoming selector remains RPN 0, but the attack snapshot ends with
    // null selectors. A subsequent data entry must still resolve correctly.
    assert_eq!(
        control(&mut decoder, 2, 2, cc(6, 13)),
        range(13, 37)
            .into_iter()
            .map(|value| expression(1, 2, false, value))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        control(&mut decoder, 3, 1, cc(38, 99)),
        range(4, 99)
            .into_iter()
            .map(|value| expression(1, 3, true, value))
            .collect::<Vec<_>>()
    );
    let before = snapshot(&decoder);
    assert!(input(&mut decoder, 4, 2, CaptureMessage::Control(cc(38, 100))).is_err());
    assert_eq!(snapshot(&decoder), before);
    assert!(input(&mut decoder, 4, 2, CaptureMessage::Control(cc(100, 6))).is_err());
    assert_eq!(snapshot(&decoder), before);
    control(&mut decoder, 4, 2, cc(101, 127));
    control(&mut decoder, 4, 2, cc(100, 127));
    let before = snapshot(&decoder);
    assert!(input(&mut decoder, 5, 2, CaptureMessage::Control(cc(6, 5))).is_err());
    assert_eq!(snapshot(&decoder), before);
}

#[test]
fn master_zone_rpn_may_restate_members_but_cannot_change_the_declared_profile() {
    let mut decoder = decoder();
    control(&mut decoder, 0, 1, cc(101, 0));
    control(&mut decoder, 0, 1, cc(100, 6));
    assert!(control(&mut decoder, 1, 1, cc(6, 3)).is_empty());
    assert!(control(&mut decoder, 1, 1, cc(38, 0)).is_empty());
    let before = snapshot(&decoder);
    assert_eq!(
        input(&mut decoder, 2, 1, CaptureMessage::Control(cc(6, 4))),
        Err(MpeError::ZoneChange {
            expected: 3,
            actual: 4
        })
    );
    assert_eq!(snapshot(&decoder), before);
    assert!(input(&mut decoder, 2, 1, CaptureMessage::Control(cc(38, 1))).is_err());
    assert_eq!(snapshot(&decoder), before);
    assert!(input(&mut decoder, 2, 1, CaptureMessage::Control(cc(101, 1))).is_err());
    assert_eq!(snapshot(&decoder), before);
    assert_eq!(decoder.profile(), &profile());
}

#[test]
fn upper_zone_filter_does_not_consume_time_or_packets_and_order_is_exact() {
    let profile = MpeProfile::new(
        "upper".into(),
        MpeZone::upper(2).unwrap(),
        BendRange::new(12, 0).unwrap(),
        BendRange::new(2, 0).unwrap(),
    )
    .unwrap();
    let mut decoder = MpeDecoder::new(profile, 2).unwrap();
    let before = snapshot(&decoder);
    assert!(input(
        &mut decoder,
        999,
        13,
        CaptureMessage::Control(Control::Bend { value: 65535 })
    )
    .unwrap()
    .is_empty());
    assert_eq!(snapshot(&decoder), before);
    attack(&mut decoder, 3, 14, 60);
    let before = snapshot(&decoder);
    assert_eq!(
        input(
            &mut decoder,
            2,
            14,
            CaptureMessage::Control(Control::Pressure { value: 1 })
        ),
        Err(MpeError::Backwards)
    );
    assert_eq!(snapshot(&decoder), before);
    assert!(input(
        &mut decoder,
        0,
        1,
        CaptureMessage::NoteOn {
            pitch: 128,
            velocity: 255
        }
    )
    .unwrap()
    .is_empty());
    assert_eq!(
        control(&mut decoder, 3, 16, Control::Bend { value: 8193 }),
        vec![expression(1, 3, true, Control::Bend { value: 8193 })]
    );
}

#[test]
fn exhausted_capacity_never_steals_voices_and_flush_always_remains_available() {
    assert!(MpeDecoder::new(profile(), 0).is_err());
    let mut decoder = MpeDecoder::new(profile(), 2).unwrap();
    attack(&mut decoder, 0, 2, 60);
    attack(&mut decoder, 0, 3, 60);
    let before = snapshot(&decoder);
    assert_eq!(
        input(
            &mut decoder,
            1,
            4,
            CaptureMessage::NoteOn {
                pitch: 62,
                velocity: 100
            }
        ),
        Err(MpeError::PacketCapacity)
    );
    assert_eq!(snapshot(&decoder), before);
    assert_eq!(decoder.active_count(), 2);
    assert_eq!(
        decoder.flush(Beat::ONE).unwrap(),
        vec![
            MpeEvent::Release {
                id: 1,
                at: Beat::ONE,
                velocity: 0
            },
            MpeEvent::Release {
                id: 2,
                at: Beat::ONE,
                velocity: 0
            },
            expression(1, 1, true, cc(64, 0)),
            expression(2, 1, true, cc(64, 0)),
            MpeEvent::End {
                id: 1,
                at: Beat::ONE
            },
            MpeEvent::End {
                id: 2,
                at: Beat::ONE
            }
        ]
    );
    assert_eq!(decoder.active_count(), 0);
    assert!(decoder.flush(Beat::ONE).unwrap().is_empty());
    let before = snapshot(&decoder);
    assert_eq!(decoder.flush(Beat::ZERO), Err(MpeError::Backwards));
    assert_eq!(snapshot(&decoder), before);
}

#[test]
fn flush_does_not_duplicate_physical_releases_for_sustained_voices() {
    let mut decoder = decoder();
    control(&mut decoder, 0, 1, cc(64, 127));
    attack(&mut decoder, 1, 2, 60);
    attack(&mut decoder, 1, 3, 62);
    release(&mut decoder, 2, 2, 60, 85);
    assert_eq!(
        decoder.flush(Beat::integer(3)).unwrap(),
        vec![
            MpeEvent::Release {
                id: 2,
                at: Beat::integer(3),
                velocity: 0
            },
            expression(1, 3, true, cc(64, 0)),
            expression(2, 3, true, cc(64, 0)),
            MpeEvent::End {
                id: 1,
                at: Beat::integer(3)
            },
            MpeEvent::End {
                id: 2,
                at: Beat::integer(3)
            }
        ]
    );
    let next = attack(&mut decoder, 4, 2, 64);
    assert!(matches!(next.as_slice(), [MpeEvent::Attack { id: 3, .. }]));
}

fn sustained_checkpoint() -> MpeDecoder {
    let mut decoder = decoder();
    control(&mut decoder, 0, 1, cc(64, 127));
    attack(&mut decoder, 1, 2, 60);
    attack(&mut decoder, 2, 3, 62);
    release(&mut decoder, 3, 2, 60, 43);
    attack(&mut decoder, 4, 2, 60);
    decoder
}
fn recover(value: Value) -> Result<MpeDecoder, String> {
    let checkpoint: MpeDecoderCheckpoint =
        serde_json::from_value(value).map_err(|error| error.to_string())?;
    MpeDecoder::recover(checkpoint).map_err(|error| error.to_string())
}

#[test]
fn checkpoint_round_trip_continues_member_reuse_ranges_and_master_release_identically() {
    let mut original = sustained_checkpoint();
    control(&mut original, 4, 2, cc(101, 0));
    control(&mut original, 4, 2, cc(100, 0));
    control(&mut original, 4, 3, Control::Pressure { value: 55 });
    let mut restored = recover(snapshot(&original)).unwrap();
    assert_eq!(restored.last_input(), Some(Beat::integer(4)));
    assert_eq!(restored.packets(), 8);
    assert_eq!(restored.capacity(), 1000);
    let voices: Vec<_> = restored.voices().collect();
    assert_eq!(voices.len(), 3);
    assert_eq!(
        (voices[0].id(), voices[0].member(), voices[0].pitch()),
        (1, channel(2), 60)
    );
    assert_eq!(voices[0].attack_at(), Beat::ONE);
    assert_eq!(voices[0].released_at(), Some(Beat::integer(3)));
    assert_eq!(voices[2].id(), 3);
    assert_eq!(voices[2].released_at(), None);
    assert_eq!(snapshot(&restored), snapshot(&original));
    assert_eq!(
        control(&mut restored, 5, 2, cc(6, 24)),
        control(&mut original, 5, 2, cc(6, 24))
    );
    assert_eq!(
        control(&mut restored, 6, 1, cc(64, 0)),
        control(&mut original, 6, 1, cc(64, 0))
    );
    assert_eq!(
        restored.flush(Beat::integer(7)).unwrap(),
        original.flush(Beat::integer(7)).unwrap()
    );
    assert_eq!(snapshot(&restored), snapshot(&original));
}

#[test]
fn checkpoint_recovery_rejects_inconsistent_capacity_ids_caches_timestamps_and_ownership() {
    let base = snapshot(&sustained_checkpoint());
    let changes: Vec<fn(&mut Value)> = vec![
        |v| v["version"] = json!(2),
        |v| v["capacity"] = json!(0),
        |v| v["capacity"] = json!(1),
        |v| v["packets"] = json!(1),
        |v| v["next_id"] = json!(0),
        |v| v["next_id"] = json!(3),
        |v| v["last_input"] = Value::Null,
        |v| v["last_input"] = json!(Beat::ZERO),
        |v| v["master"]["bend"] = json!(16384),
        |v| v["master"]["pressure"] = json!(128),
        |v| v["master"]["range"]["cents"] = json!(100),
        |v| v["master"]["extra"] = json!(0),
        |v| v["master"]["rpn"]["msb"] = json!(1),
        |v| v["members"][1]["rpn"]["lsb"] = json!(6),
        |v| v["members"][0]["bend"] = json!(123),
        |v| v["current"][1] = json!(1),
        |v| v["current"][0] = json!(3),
        |v| v["voices"][0]["id"] = json!(0),
        |v| v["voices"][0]["member"] = json!(1),
        |v| v["voices"][0]["pitch"] = json!(128),
        |v| v["voices"][0]["released_at"] = json!(Beat::ZERO),
        |v| v["voices"][0]["released_at"] = Value::Null,
        |v| v["voices"][0]["released_at"] = json!(Beat::integer(5)),
        |v| v["voices"][1]["attack_at"] = json!(Beat::ZERO),
        |v| {
            v["voices"][0]["released_at"] = json!(Beat::integer(4));
            v["voices"][2]["attack_at"] = json!(Beat::integer(3));
        },
        |v| {
            let voice = v["voices"][0].clone();
            v["voices"].as_array_mut().unwrap().push(voice);
        },
        |v| v["profile"]["source"] = json!(""),
        |v| v["profile"]["zone"]["members"] = json!(1),
    ];
    for (index, change) in changes.into_iter().enumerate() {
        let mut invalid = base.clone();
        change(&mut invalid);
        assert!(recover(invalid).is_err(), "mutation {index}");
    }
    let mut empty = snapshot(&decoder());
    empty["members"][1]["pressure"] = json!(0);
    assert!(recover(empty).is_err());
    let mut flushed = decoder();
    flushed.flush(Beat::integer(3)).unwrap();
    assert!(recover(snapshot(&flushed)).is_ok());
}

#[cfg(target_pointer_width = "64")]
#[test]
fn identity_exhaustion_rejects_without_wrapping_or_consuming_the_last_packet() {
    let mut value = snapshot(&decoder());
    value["capacity"] = json!(usize::MAX);
    value["packets"] = json!(usize::MAX - 1);
    value["next_id"] = json!(u64::MAX);
    value["last_input"] = json!(Beat::ZERO);
    let mut decoder = recover(value).unwrap();
    let before = snapshot(&decoder);
    assert_eq!(
        input(
            &mut decoder,
            1,
            2,
            CaptureMessage::NoteOn {
                pitch: 60,
                velocity: 100
            }
        ),
        Err(MpeError::IdentityExhausted)
    );
    assert_eq!(snapshot(&decoder), before);
}
