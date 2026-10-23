//! Hardware-free MPE capture, persistence, and playback regressions.
use super::tests::{capture_fixture, clock};
use super::*;
use crate::midi_output::{MpeOutputAction, MpeOutputProfile, MpeOwner, MpeSource};
use crest_synth::control::{MidiConnectionRevision, PhysicalMidiEvent};
use crest_tracker::mpe::{BendRange, MpeEvent, MpeProfile, MpeZone};

const EXTERNAL_ENDPOINT: &str = "fixture:mpe-output";

fn output_profile() -> MpeOutputProfile {
    MpeOutputProfile {
        endpoint: EXTERNAL_ENDPOINT.into(),
        zones: vec![MpeZone::lower(2).unwrap(), MpeZone::upper(2).unwrap()],
    }
}

fn waiting_capture(anchor: u64) -> Runtime {
    let mut runtime = capture_fixture();
    runtime.recorder.discard();
    {
        let app = runtime.app.borrow();
        runtime
            .recorder
            .arm_mpe(
                &app.editor,
                app.occurrence().unwrap(),
                MpeProfile::new(
                    "fixture".into(),
                    MpeZone::lower(15).unwrap(),
                    BendRange::new(48, 0).unwrap(),
                    BendRange::new(2, 0).unwrap(),
                )
                .unwrap(),
            )
            .unwrap();
    }
    runtime.recorder.request_record(Beat::ZERO).unwrap();
    runtime.anchor = anchor;
    runtime
}

fn packet(channel: u8, kind: MidiMessageKind, a: u8, b: u8, timestamp: u64) -> PhysicalMidiEvent {
    PhysicalMidiEvent::new(
        MidiConnectionRevision::new(1).unwrap(),
        timestamp,
        MidiMessage::try_new(MidiChannel::new(channel - 1).unwrap(), kind, a, b).unwrap(),
    )
}

fn input(runtime: &mut Runtime, channel: u8, kind: MidiMessageKind, a: u8, b: u8, timestamp: u64) {
    assert!(runtime.capture_midi_input(packet(channel, kind, a, b, timestamp), clock()));
}

#[test]
fn pre_anchor_attack_is_monitored_once_and_boundary_interruption_delivers_owned_cleanup() {
    let mut runtime = waiting_capture(48_000);
    input(
        &mut runtime,
        5,
        MidiMessageKind::PitchBend,
        17,
        65,
        1_310_000,
    );
    input(&mut runtime, 5, MidiMessageKind::NoteOn, 60, 99, 1_510_000);
    assert_eq!(
        runtime.recorder.state(),
        &CaptureState::Waiting {
            boundary: Beat::ZERO
        }
    );
    assert!(runtime.recorder.draft().is_none());
    assert!(runtime.monitor_pending.is_empty());
    let monitored = std::mem::take(&mut runtime.monitor_owned_pending);
    let [MpeEvent::Attack { id, member, .. }] = monitored.as_slice() else {
        panic!("one owned pre-roll attack expected")
    };
    assert!(member.contains(&Control::Bend { value: 8337 }));
    assert!(!member
        .iter()
        .any(|control| matches!(control, Control::Pressure { .. })));

    // The host reaches its audio anchor with this key still held. No input
    // packet arrives to trigger or drain the recorder's boundary cleanup.
    runtime.recorder.advance(Beat::ZERO);
    assert!(matches!(
        runtime.recorder.state(),
        CaptureState::Interrupted { .. }
    ));
    assert!(runtime.recorder.draft().is_none());
    assert_eq!(runtime.recorder.held_count(), 0);
    runtime.queue_mpe_monitor().unwrap();
    assert!(matches!(runtime.monitor_owned_pending.as_slice(), [
        MpeEvent::Release { id: release, .. },
        MpeEvent::Expression { id: pedal, master: true, control: Control::Cc { number: 64, value: 0 }, .. },
        MpeEvent::End { id: ended, .. }
    ] if release == id && pedal == id && ended == id));
    runtime.queue_mpe_monitor().unwrap();
    assert_eq!(runtime.monitor_owned_pending.len(), 3);
    assert!(runtime.monitor_pending.is_empty());
}

#[test]
fn captured_member_reuse_survives_commit_document_reopen_and_owned_playback() {
    let mut runtime = waiting_capture(4_800);
    let original = runtime.app.borrow().editor.song().clone();
    let occurrence = runtime.app.borrow().occurrence().unwrap();
    input(
        &mut runtime,
        1,
        MidiMessageKind::ControlChange,
        64,
        127,
        1_050_000,
    );
    input(
        &mut runtime,
        5,
        MidiMessageKind::PitchBend,
        17,
        65,
        1_060_000,
    );
    assert!(runtime.recorder.draft().is_none());
    assert!(runtime.monitor_owned_pending.is_empty());

    input(&mut runtime, 5, MidiMessageKind::NoteOn, 60, 99, 1_610_000);
    input(&mut runtime, 5, MidiMessageKind::NoteOff, 60, 41, 1_810_000);
    input(&mut runtime, 5, MidiMessageKind::NoteOn, 62, 101, 1_910_000);
    input(
        &mut runtime,
        5,
        MidiMessageKind::PitchBend,
        41,
        70,
        2_010_000,
    );
    input(
        &mut runtime,
        1,
        MidiMessageKind::ChannelPressure,
        93,
        0,
        2_010_000,
    );
    input(
        &mut runtime,
        1,
        MidiMessageKind::ControlChange,
        64,
        0,
        2_110_000,
    );
    input(&mut runtime, 5, MidiMessageKind::NoteOff, 62, 57, 2_210_000);
    runtime.recorder.finish_at(Beat::integer(3)).unwrap();
    runtime.queue_mpe_monitor().unwrap();
    assert!(runtime.monitor_pending.is_empty());
    let monitored = std::mem::take(&mut runtime.monitor_owned_pending);
    assert_eq!(
        monitored
            .iter()
            .filter(|event| matches!(event, MpeEvent::Attack { .. }))
            .count(),
        2
    );
    assert_eq!(
        monitored
            .iter()
            .filter(|event| matches!(event, MpeEvent::Release { .. }))
            .count(),
        2
    );
    assert_eq!(
        monitored
            .iter()
            .filter(|event| matches!(event, MpeEvent::End { .. }))
            .count(),
        2
    );
    let captured = runtime.recorder.draft().unwrap().events.clone();
    let [first, second] = captured.as_slice() else {
        panic!("two captured notes expected")
    };
    let first_note = first.note.as_ref().unwrap();
    let second_note = second.note.as_ref().unwrap();
    assert_eq!(
        (
            first.at,
            first_note.duration,
            first_note.expression_duration(),
            first_note.release_velocity
        ),
        (Beat::ONE, Beat::new(2, 5).unwrap(), Beat::ONE, 41)
    );
    assert_eq!(
        (
            second.at,
            second_note.duration,
            second_note.expression_duration(),
            second_note.release_velocity
        ),
        (
            Beat::new(8, 5).unwrap(),
            Beat::new(3, 5).unwrap(),
            Beat::new(3, 5).unwrap(),
            57
        )
    );
    assert!(!first_note
        .expression
        .iter()
        .any(|point| point.control == Control::Bend { value: 9001 }));
    assert!(second_note
        .expression
        .iter()
        .any(|point| point.offset == Beat::new(1, 5).unwrap()
            && point.control == Control::Bend { value: 9001 }));
    assert!(first_note
        .master_expression
        .iter()
        .any(|point| point.offset == Beat::new(4, 5).unwrap()
            && point.control == Control::Pressure { value: 93 }));
    assert!(second_note
        .master_expression
        .iter()
        .any(|point| point.offset == Beat::new(1, 5).unwrap()
            && point.control == Control::Pressure { value: 93 }));

    // Exercise the application's validated publication step. Native audio and
    // its separate sound document are not created by this hardware-free fixture.
    runtime.publish_take().unwrap();
    assert!(runtime.recorder.draft().is_none());
    let committed = runtime.app.borrow().editor.song().clone();
    runtime.app.borrow_mut().editor.undo().unwrap();
    assert_eq!(runtime.app.borrow().editor.song(), &original);
    runtime.app.borrow_mut().editor.redo().unwrap();
    assert_eq!(runtime.app.borrow().editor.song(), &committed);
    let document = Document::new(
        committed.clone(),
        serde_json::json!({}),
        runtime.app.borrow().mappings.clone(),
    );
    document.validate().unwrap();
    let path = std::env::temp_dir().join(format!(
        "crest-mpe-roundtrip-{}.crest-song",
        uuid::Uuid::new_v4()
    ));
    atomic_write(&path, &serde_json::to_vec_pretty(&document).unwrap()).unwrap();
    let reopened = Document::read(&path).unwrap();
    std::fs::remove_file(path).unwrap();
    assert_eq!(reopened.song, committed);
    let editor = Editor::new(reopened.song).unwrap();
    let snapshot =
        Snapshot::compile(editor.song(), editor.revision(), PlaybackBudget::default()).unwrap();
    let mut playback = Playback::new_looping(Arc::new(snapshot), occurrence).unwrap();
    let events = playback.window(Beat::integer(3)).unwrap();
    let note_trace: Vec<_> = events
        .iter()
        .filter_map(|event| match event.message {
            Message::NoteOn { pitch, velocity } => Some((event.at, true, pitch, velocity)),
            Message::NoteOff { pitch, velocity } => Some((event.at, false, pitch, velocity)),
            _ => None,
        })
        .collect();
    assert_eq!(
        note_trace,
        vec![
            (Beat::ONE, true, 60, 99),
            (Beat::new(7, 5).unwrap(), false, 60, 41),
            (Beat::new(8, 5).unwrap(), true, 62, 101),
            (Beat::new(11, 5).unwrap(), false, 62, 57),
        ]
    );
    let mut ledger = ScheduledVoices::default();
    let mut attacks = BTreeMap::new();
    let mut pressure_recipients = Vec::new();
    for event in &events {
        let voice = ledger.voice_for(event).unwrap();
        match event.message {
            Message::NoteOn { .. } => {
                attacks.insert(event.owner.event, voice.unwrap());
            }
            Message::MasterExpression {
                control: Control::Pressure { value: 93 },
            } => {
                pressure_recipients.push((event.owner.event, voice.unwrap()));
            }
            Message::Expression {
                control: Control::Bend { value: 9001 },
            } => {
                assert_eq!(event.owner.event, second.id);
                assert_eq!(voice, attacks.get(&second.id).copied());
            }
            _ => {}
        }
    }
    assert_ne!(attacks[&first.id], attacks[&second.id]);
    assert_eq!(
        pressure_recipients,
        vec![
            (first.id, attacks[&first.id]),
            (second.id, attacks[&second.id])
        ]
    );
    assert!(ledger.entries.is_empty());
}

#[test]
fn external_capture_requires_exact_declared_profile_without_raw_monitor_fallback() {
    let mut runtime = waiting_capture(0);
    runtime.capture_target = Some(Target::Channel {
        channel: Channel::FIRST,
        endpoint: Some(EXTERNAL_ENDPOINT.into()),
    });
    input(&mut runtime, 5, MidiMessageKind::NoteOn, 60, 99, 1_510_000);
    assert!(runtime.monitor_pending.is_empty());
    assert!(
        matches!(runtime.monitor_owned_pending.as_slice(), [MpeEvent::Attack { target: Target::Channel { endpoint: Some(endpoint), .. }, .. }]
        if endpoint == EXTERNAL_ENDPOINT)
    );
    assert_eq!(runtime.recorder.held_count(), 1);
    assert_eq!(runtime.outputs.snapshot().pending, 0);
    assert_eq!(
        runtime
            .flush_external_monitor(EXTERNAL_ENDPOINT)
            .unwrap_err(),
        "MPE output profile disappeared"
    );
    let mut wrong_endpoint = output_profile();
    wrong_endpoint.endpoint.push_str("-another-device");
    runtime.sync.preferences.mpe_outputs.push(wrong_endpoint);
    assert_eq!(
        runtime
            .flush_external_monitor(EXTERNAL_ENDPOINT)
            .unwrap_err(),
        "MPE output profile disappeared"
    );
    assert!(runtime.monitor_pending.is_empty());
    assert_eq!(runtime.monitor_owned_pending.len(), 1);
    assert_eq!(runtime.outputs.snapshot().pending, 0);
    assert_eq!(runtime.recorder.held_count(), 1);
}

#[test]
fn external_mpe_profiles_roundtrip_exact_endpoint_and_reject_invalid_replacements() {
    let directory =
        std::env::temp_dir().join(format!("crest-mpe-preferences-{}", uuid::Uuid::new_v4()));
    let mut preferences = crate::preferences::MachinePreferences::default();
    let mut profile = output_profile();
    profile.endpoint = "  output:CoreMIDI:鍵盤 #17  ".into();
    preferences.mpe_outputs.push(profile.clone());
    crate::preferences::save(&directory, &preferences).unwrap();
    assert_eq!(crate::preferences::load(&directory).unwrap(), preferences);
    let original = std::fs::read(directory.join("preferences.json")).unwrap();

    let mut duplicate = preferences.clone();
    duplicate.mpe_outputs.push(profile);
    assert!(crate::preferences::save(&directory, &duplicate)
        .unwrap_err()
        .contains("Duplicate MPE destination"));
    let mut overlap = preferences.clone();
    overlap.mpe_outputs[0].zones = vec![MpeZone::lower(15).unwrap(), MpeZone::upper(15).unwrap()];
    assert!(crate::preferences::save(&directory, &overlap)
        .unwrap_err()
        .contains("overlap"));
    assert_eq!(
        std::fs::read(directory.join("preferences.json")).unwrap(),
        original
    );
    assert_eq!(crate::preferences::load(&directory).unwrap(), preferences);
    std::fs::remove_dir_all(directory).unwrap();
}

fn external_capture() -> (Runtime, crate::midi_output::MidiOutputTestDriver) {
    let mut runtime = waiting_capture(0);
    runtime.capture_target = Some(Target::Channel {
        channel: Channel::FIRST,
        endpoint: Some(EXTERNAL_ENDPOINT.into()),
    });
    runtime.sync.preferences.mpe_outputs.push(output_profile());
    let (outputs, driver) = MidiOutputService::recording_for_test(EXTERNAL_ENDPOINT, 4096);
    runtime.outputs = outputs;
    (runtime, driver)
}

fn action_owner(action: &MpeOutputAction) -> MpeOwner {
    match action {
        MpeOutputAction::Attack { owner, .. }
        | MpeOutputAction::Release { owner, .. }
        | MpeOutputAction::Expression { owner, .. }
        | MpeOutputAction::End { owner } => *owner,
    }
}

fn flush_driver(driver: &mut crate::midi_output::MidiOutputTestDriver) {
    driver
        .flush_due(std::time::Instant::now() + std::time::Duration::from_secs(1))
        .unwrap();
}

#[test]
fn external_runtime_batches_equal_beat_master_fanout_and_same_beat_ends() {
    let (mut runtime, mut driver) = external_capture();
    input(
        &mut runtime,
        1,
        MidiMessageKind::ControlChange,
        64,
        127,
        1_010_000,
    );
    input(&mut runtime, 5, MidiMessageKind::NoteOn, 60, 99, 1_510_000);
    input(&mut runtime, 6, MidiMessageKind::NoteOn, 64, 101, 1_510_000);
    input(&mut runtime, 5, MidiMessageKind::NoteOff, 60, 41, 1_610_000);
    input(&mut runtime, 6, MidiMessageKind::NoteOff, 64, 57, 1_710_000);
    input(
        &mut runtime,
        1,
        MidiMessageKind::ChannelPressure,
        93,
        0,
        1_810_000,
    );
    input(
        &mut runtime,
        1,
        MidiMessageKind::ControlChange,
        64,
        0,
        1_910_000,
    );
    runtime.flush_external_monitor(EXTERNAL_ENDPOINT).unwrap();
    flush_driver(&mut driver);
    let batches = driver.batches();
    assert_eq!(batches.len(), 5);
    assert!(batches
        .iter()
        .all(|batch| batch.profile == output_profile()));
    assert!(batches
        .iter()
        .all(|batch| batch.deadline == batches[0].deadline));
    assert!(matches!(
        batches[0].actions.as_slice(),
        [
            MpeOutputAction::Attack { .. },
            MpeOutputAction::Attack { .. }
        ]
    ));
    assert!(matches!(
        batches[3].actions.as_slice(),
        [
            MpeOutputAction::Expression {
                master: true,
                control: Control::Pressure { value: 93 },
                ..
            },
            MpeOutputAction::Expression {
                master: true,
                control: Control::Pressure { value: 93 },
                ..
            }
        ]
    ));
    assert!(matches!(
        batches[4].actions.as_slice(),
        [
            MpeOutputAction::Expression {
                master: true,
                control: Control::Cc {
                    number: 64,
                    value: 0
                },
                ..
            },
            MpeOutputAction::Expression {
                master: true,
                control: Control::Cc {
                    number: 64,
                    value: 0
                },
                ..
            },
            MpeOutputAction::End { .. },
            MpeOutputAction::End { .. }
        ]
    ));
    let owners: Vec<_> = batches[0].actions.iter().map(action_owner).collect();
    assert_ne!(owners[0].voice, owners[1].voice);
    assert!(owners
        .iter()
        .all(|owner| owner.source == MpeSource::Monitor && owner.group == 1));
    let packets = driver.packets();
    let notes: Vec<_> = packets
        .iter()
        .filter(|bytes| matches!(bytes[0] >> 4, 8 | 9))
        .cloned()
        .collect();
    assert_eq!(
        notes,
        vec![
            vec![0x91, 60, 99],
            vec![0x92, 64, 101],
            vec![0x81, 60, 41],
            vec![0x82, 64, 57]
        ]
    );
    assert_eq!(
        packets
            .iter()
            .filter(|bytes| bytes.as_slice() == [0xd0, 93])
            .count(),
        2
    );
    assert!(runtime.monitor_pending.is_empty());
    assert!(runtime.monitor_owned_pending.is_empty());
    assert_eq!(runtime.outputs.snapshot().pending, 0);
    let sent = packets.len();
    runtime.flush_external_monitor(EXTERNAL_ENDPOINT).unwrap();
    flush_driver(&mut driver);
    assert_eq!(driver.packets().len(), sent);
}

fn playback_event(
    runtime: &Runtime,
    identity: u64,
    cycle: u64,
    at: Beat,
    message: Message,
) -> TimedEvent {
    let occurrence = runtime.app.borrow().occurrence().unwrap();
    TimedEvent {
        at,
        target: Target::Channel {
            channel: Channel::FIRST,
            endpoint: Some(EXTERNAL_ENDPOINT.into()),
        },
        owner: Owner {
            track: occurrence.track,
            chain_use: occurrence.chain_use,
            phrase_use: occurrence.phrase_use,
            cycle,
            event: EventId::new(identity).unwrap(),
            voice: 0,
        },
        message,
    }
}

fn playback_action(runtime: &mut Runtime, event: &TimedEvent, attack: bool) -> MpeOutputAction {
    let voice = runtime.voices.voice_for(event).unwrap();
    let initial = (Vec::new(), Vec::new());
    runtime
        .external_expression
        .event(event, voice, attack.then_some(&initial))
        .unwrap()
        .unwrap()
}

fn send_playback_batch(runtime: &mut Runtime, at: Beat, actions: Vec<MpeOutputAction>) {
    let mut batches = BTreeMap::from([(EXTERNAL_ENDPOINT.into(), actions)]);
    runtime
        .flush_external_expression(at, &mut batches, clock())
        .unwrap();
    assert!(batches.is_empty());
}

#[test]
fn empty_external_batch_does_not_require_external_audio_clock() {
    let mut runtime = capture_fixture();
    let mut clock = clock();
    clock.audio_clock = None;
    runtime
        .flush_external_expression(Beat::ZERO, &mut BTreeMap::new(), clock)
        .unwrap();

    let event = playback_event(
        &runtime,
        201,
        0,
        Beat::ZERO,
        Message::NoteOn {
            pitch: 67,
            velocity: 100,
        },
    );
    let action = playback_action(&mut runtime, &event, true);
    let mut batches = BTreeMap::from([(EXTERNAL_ENDPOINT.into(), vec![action])]);
    assert_eq!(
        runtime.flush_external_expression(Beat::ZERO, &mut batches, clock),
        Err("Audio timing is not ready".into())
    );
    assert_eq!(
        batches[EXTERNAL_ENDPOINT].len(),
        1,
        "failed timing must retain actual external work"
    );
}

#[test]
fn runtime_external_sources_and_occurrences_keep_distinct_leases_through_monitor_cancel() {
    let (mut runtime, mut driver) = external_capture();
    input(&mut runtime, 5, MidiMessageKind::NoteOn, 60, 99, 1_510_000);
    runtime.flush_external_monitor(EXTERNAL_ENDPOINT).unwrap();
    flush_driver(&mut driver);
    let monitored = action_owner(&driver.batches()[0].actions[0]);
    let mut first = playback_event(
        &runtime,
        201,
        0,
        Beat::ZERO,
        Message::NoteOn {
            pitch: 67,
            velocity: 100,
        },
    );
    let mut second = playback_event(
        &runtime,
        202,
        0,
        Beat::ZERO,
        Message::NoteOn {
            pitch: 69,
            velocity: 102,
        },
    );
    let attacks = vec![
        playback_action(&mut runtime, &first, true),
        playback_action(&mut runtime, &second, true),
    ];
    let playback_owner = action_owner(&attacks[0]);
    assert_eq!(monitored.voice, playback_owner.voice);
    assert_eq!(monitored.group, playback_owner.group);
    assert_ne!(monitored.source, playback_owner.source);
    assert_eq!(action_owner(&attacks[1]).group, playback_owner.group);
    send_playback_batch(&mut runtime, Beat::ZERO, attacks);
    flush_driver(&mut driver);
    let notes: Vec<_> = driver
        .packets()
        .into_iter()
        .filter(|bytes| bytes[0] >> 4 == 9)
        .collect();
    assert_eq!(
        notes,
        vec![vec![0x91, 60, 99], vec![0x9d, 67, 100], vec![0x9e, 69, 102]]
    );

    let before_cancel = driver.packets().len();
    runtime.monitor_cleanup = true;
    runtime.flush_external_monitor(EXTERNAL_ENDPOINT).unwrap();
    flush_driver(&mut driver);
    let cleanup = driver.packets()[before_cancel..].to_vec();
    assert!(cleanup.contains(&vec![0x81, 60, 0]));
    assert!(cleanup.iter().all(|bytes| matches!(bytes[0] & 15, 0 | 1)));
    assert!(runtime.external_expression.contains(&first.owner));
    assert!(runtime.external_expression.contains(&second.owner));
    let snapshot = runtime.outputs.snapshot();
    assert_eq!(
        snapshot.requested_monitor_generation,
        snapshot.settled_monitor_generation
    );

    // A new occurrence gets the freed lower zone. The earlier playback group
    // retains its upper zone and both voices despite sharing numeric IDs with monitoring.
    let mut next = playback_event(
        &runtime,
        201,
        1,
        Beat::ONE,
        Message::NoteOn {
            pitch: 72,
            velocity: 103,
        },
    );
    let attack = playback_action(&mut runtime, &next, true);
    assert_ne!(action_owner(&attack).group, playback_owner.group);
    send_playback_batch(&mut runtime, Beat::ONE, vec![attack]);
    flush_driver(&mut driver);
    assert!(driver.packets().contains(&vec![0x91, 72, 103]));

    let mut end_actions = Vec::new();
    for (event, pitch, velocity) in [
        (&mut first, 67, 71),
        (&mut second, 69, 72),
        (&mut next, 72, 73),
    ] {
        event.at = Beat::integer(2);
        event.message = Message::NoteOff { pitch, velocity };
        end_actions.push(playback_action(&mut runtime, event, false));
        event.message = Message::EndExpression;
        end_actions.push(playback_action(&mut runtime, event, false));
    }
    send_playback_batch(&mut runtime, Beat::integer(2), end_actions);
    flush_driver(&mut driver);
    let releases: Vec<_> = driver
        .packets()
        .into_iter()
        .filter(|bytes| bytes[0] >> 4 == 8)
        .collect();
    assert_eq!(
        releases,
        vec![
            vec![0x81, 60, 0],
            vec![0x8d, 67, 71],
            vec![0x8e, 69, 72],
            vec![0x81, 72, 73]
        ]
    );
    assert!(!runtime.external_expression.contains(&first.owner));
    assert!(!runtime.external_expression.contains(&second.owner));
    assert!(!runtime.external_expression.contains(&next.owner));
    assert_eq!(runtime.outputs.snapshot().pending, 0);
}
