use crest_tracker::composition::*;
use crest_tracker::harmony::Pitch;
use crest_tracker::playback::{Message, Playback, PlaybackBudget, Snapshot, TimedEvent};
use crest_tracker::time::Beat;
use std::sync::Arc;

fn generated_song(form: PhraseForm, steps: u32) -> Song {
    let mut song = Song::default();
    song.tracks.truncate(1);
    let phrase = PhraseId::new(101).unwrap();
    let chain = ChainId::new(102).unwrap();
    song.phrases.insert(
        phrase,
        Phrase {
            steps,
            form,
            ..Phrase::default()
        },
    );
    song.chains.insert(
        chain,
        Chain {
            name: "Generator".into(),
            phrases: vec![PhraseUse {
                id: UseId::new(103).unwrap(),
                phrase,
                repeats: 1,
            }],
        },
    );
    song.tracks[0].slots = vec![Some(ChainUse {
        id: UseId::new(104).unwrap(),
        chain: chain.into(),
        repeats: 1,
    })];
    song.tracks[0].looped = true;
    song
}

fn target() -> Target {
    Target::Channel {
        channel: Channel::FIRST,
        endpoint: None,
    }
}

fn random_song() -> Song {
    generated_song(
        PhraseForm::Random {
            seed: 1984,
            vary: true,
            density: 75,
            low: 36,
            high: 84,
            velocity_low: 32,
            velocity_high: 126,
            gate: Beat::new(3, 4).unwrap(),
            gate_high: None,
            target: target(),
        },
        16,
    )
}

fn player(song: &Song) -> Playback {
    Playback::new(Arc::new(
        Snapshot::compile(song, 42, PlaybackBudget::default()).unwrap(),
    ))
}

fn notes(events: &[TimedEvent], track: TrackId) -> Vec<(Beat, u8, u8)> {
    events
        .iter()
        .filter_map(|event| match event.message {
            Message::NoteOn { pitch, velocity } if event.owner.track == track => {
                Some((event.at, pitch, velocity))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn random_generation_is_partition_invariant_and_projections_never_mutate_music() {
    let song = random_song();
    let source = song.clone();
    let expected = player(&song).window(Beat::integer(32)).unwrap();
    let mut playback = player(&song);
    let mut actual = Vec::new();
    for window in 1..=128 {
        let _ = serde_json::to_value(playback.playheads()).unwrap();
        actual.extend(playback.window(Beat::new(window, 4).unwrap()).unwrap());
    }
    assert_eq!(actual, expected);
    assert_eq!(song, source);
    let generated = notes(&actual, song.tracks[0].id);
    assert!(!generated.is_empty());
    assert!(generated.iter().all(|(_, pitch, velocity)| song
        .key
        .contains(Pitch::new(*pitch).unwrap())
        && (36..=84).contains(pitch)
        && (32..=126).contains(velocity)));
}

#[test]
fn shared_chain_occurrences_use_independent_random_streams() {
    let mut song = random_song();
    let original = notes(
        &player(&song).window(Beat::integer(32)).unwrap(),
        song.tracks[0].id,
    );
    let mut second = song.tracks[0].clone();
    second.id = TrackId::new(2).unwrap();
    second.slots[0].as_mut().unwrap().id = UseId::new(105).unwrap();
    song.tracks.push(second);
    let output = player(&song).window(Beat::integer(32)).unwrap();
    let first = notes(&output, song.tracks[0].id);
    let second = notes(&output, song.tracks[1].id);
    assert_eq!(
        first, original,
        "adding an occurrence must not perturb an existing stream"
    );
    assert!(
        first != second,
        "different track/chain-use occurrences must not share random decisions"
    );
}

fn euclidean_steps(rotation: u32) -> Vec<u64> {
    let song = generated_song(
        PhraseForm::Euclidean {
            pitches: vec![],
            pulses: 5,
            rotation,
            pitch: 36,
            velocity: 100,
            gate: Beat::new(1, 2).unwrap(),
            target: target(),
        },
        13,
    );
    let events = player(&song).window(Beat::new(13, 4).unwrap()).unwrap();
    notes(&events, song.tracks[0].id)
        .iter()
        .map(|(at, _, _)| at.units(Beat::new(1, 4).unwrap()).unwrap())
        .collect()
}

#[test]
fn euclidean_rotation_preserves_pulse_count_and_even_spacing() {
    let base = euclidean_steps(0);
    let rotated = euclidean_steps(3);
    assert_eq!(base.len(), 5);
    assert_eq!(rotated.len(), 5);
    let mut expected: Vec<_> = base.iter().map(|step| (step + 3) % 13).collect();
    expected.sort_unstable();
    assert_eq!(rotated, expected);
    let mut wrapped = base.clone();
    wrapped.push(base[0] + 13);
    let gaps: Vec<_> = wrapped.windows(2).map(|pair| pair[1] - pair[0]).collect();
    assert_eq!(gaps.iter().max().unwrap() - gaps.iter().min().unwrap(), 1);
}

#[test]
fn euclidean_pitches_follow_pulses_through_rotation_and_old_documents_load() {
    let mut song = generated_song(
        PhraseForm::Euclidean {
            pitches: vec![Some(60), None, Some(67), Some(71), Some(74)],
            pulses: 5,
            rotation: 0,
            pitch: 64,
            velocity: 100,
            gate: Beat::new(1, 2).unwrap(),
            target: target(),
        },
        13,
    );
    let attacks = |song: &Song| {
        notes(
            &player(song).window(Beat::new(13, 4).unwrap()).unwrap(),
            song.tracks[0].id,
        )
    };
    let original = attacks(&song);
    assert_eq!(
        original
            .iter()
            .map(|(_, pitch, _)| *pitch)
            .collect::<Vec<_>>(),
        [60, 64, 67, 71, 74]
    );
    if let PhraseForm::Euclidean { rotation, .. } =
        &mut song.phrases.values_mut().next().unwrap().form
    {
        *rotation = 3;
    }
    let mut expected: Vec<_> = original
        .iter()
        .map(|(at, pitch, velocity)| {
            (
                Beat::new((at.units(Beat::new(1, 4).unwrap()).unwrap() + 3) % 13, 4).unwrap(),
                *pitch,
                *velocity,
            )
        })
        .collect();
    expected.sort_by_key(|item| item.0);
    assert_eq!(attacks(&song), expected);
    if let PhraseForm::Euclidean { pulses, .. } =
        &mut song.phrases.values_mut().next().unwrap().form
    {
        *pulses = 2;
    }
    assert_eq!(attacks(&song).len(), 2);
    if let PhraseForm::Euclidean { pulses, .. } =
        &mut song.phrases.values_mut().next().unwrap().form
    {
        *pulses = 5;
    }
    assert_eq!(attacks(&song), expected);
    let form = &song.phrases.values().next().unwrap().form;
    let mut legacy = serde_json::to_value(form).unwrap();
    legacy.as_object_mut().unwrap().remove("pitches");
    assert!(
        matches!(serde_json::from_value::<PhraseForm>(legacy).unwrap(), PhraseForm::Euclidean { pitches, .. } if pitches.is_empty())
    );
    if let PhraseForm::Euclidean { pitches, .. } =
        &mut song.phrases.values_mut().next().unwrap().form
    {
        pitches[0] = Some(128);
    }
    assert!(song.validate().is_err());
}

#[test]
fn generator_density_extremes_are_explicit_and_repeatable() {
    let mut silent = random_song();
    if let PhraseForm::Random { density, .. } =
        &mut silent.phrases.values_mut().next().unwrap().form
    {
        *density = 0;
    }
    assert!(player(&silent).window(Beat::integer(8)).unwrap().is_empty());
    let mut full = silent;
    if let PhraseForm::Random { density, .. } = &mut full.phrases.values_mut().next().unwrap().form
    {
        *density = 100;
    }
    let events = player(&full).window(Beat::integer(8)).unwrap();
    assert_eq!(notes(&events, full.tracks[0].id).len(), 32);
}

#[test]
fn random_gate_range_is_exact_bounded_deterministic_and_legacy_compatible() {
    let legacy = random_song();
    let encoded = serde_json::to_value(&legacy).unwrap();
    let restored: Song = serde_json::from_value(encoded).unwrap();
    assert_eq!(
        player(&legacy).window(Beat::integer(16)).unwrap(),
        player(&restored).window(Beat::integer(16)).unwrap()
    );
    let mut ranged = legacy.clone();
    if let PhraseForm::Random {
        gate,
        gate_high,
        density,
        ..
    } = &mut ranged.phrases.values_mut().next().unwrap().form
    {
        *gate = Beat::new(1, 4).unwrap();
        *gate_high = Some(Beat::new(3, 4).unwrap());
        *density = 100;
    }
    let expected = player(&ranged).window(Beat::integer(32)).unwrap();
    let mut partitioned = player(&ranged);
    let mut actual = Vec::new();
    for step in 1..=128 {
        actual.extend(partitioned.window(Beat::new(step, 4).unwrap()).unwrap());
    }
    assert_eq!(actual, expected);
    let durations: std::collections::BTreeSet<_> = expected
        .iter()
        .filter_map(|attack| {
            if !matches!(attack.message, Message::NoteOn { .. }) {
                return None;
            }
            expected
                .iter()
                .find(|release| {
                    release.owner == attack.owner
                        && matches!(release.message, Message::NoteOff { .. })
                })
                .map(|release| release.at.checked_sub(attack.at).unwrap())
        })
        .collect();
    assert!(durations.len() > 10);
    assert!(durations
        .iter()
        .all(|duration| *duration >= Beat::new(1, 16).unwrap()
            && *duration <= Beat::new(3, 16).unwrap()));
    if let PhraseForm::Random { gate_high, .. } =
        &mut ranged.phrases.values_mut().next().unwrap().form
    {
        *gate_high = Some(Beat::new(1, 8).unwrap());
    }
    assert!(ranged.validate().is_err());
}

#[test]
fn random_keep_repeats_preview_while_vary_retains_legacy_behavior() {
    let mut song = random_song();
    if let PhraseForm::Random { vary, .. } = &mut song.phrases.values_mut().next().unwrap().form {
        *vary = false;
    }
    let events = player(&song).window(Beat::integer(8)).unwrap();
    let all = notes(&events, song.tracks[0].id);
    let first: Vec<_> = all
        .iter()
        .filter(|(at, _, _)| *at < Beat::integer(4))
        .copied()
        .collect();
    let second: Vec<_> = all
        .iter()
        .filter(|(at, _, _)| *at >= Beat::integer(4))
        .map(|(at, pitch, velocity)| (at.checked_sub(Beat::integer(4)).unwrap(), *pitch, *velocity))
        .collect();
    assert_eq!(first, second);
    let mut encoded = serde_json::to_value(&song).unwrap();
    for phrase in encoded["phrases"].as_object_mut().unwrap().values_mut() {
        phrase["form"].as_object_mut().unwrap().remove("vary");
    }
    let legacy: Song = serde_json::from_value(encoded).unwrap();
    assert!(matches!(
        legacy.phrases.values().next().unwrap().form,
        PhraseForm::Random { vary: true, .. }
    ));
}
