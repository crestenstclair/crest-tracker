use crest_tracker::time::{Beat, Division, Meter, Tempo, TimeError};

#[test]
fn straight_and_triplets_align_for_ten_thousand_cycles() {
    let straight = Division::SIXTEENTH
        .beat_length()
        .checked_scale(16, 1)
        .unwrap();
    let triplet = Division::new(8, 3, 2)
        .unwrap()
        .beat_length()
        .checked_scale(12, 1)
        .unwrap();
    let mut left = Beat::ZERO;
    let mut right = Beat::ZERO;
    for _ in 0..10_000 {
        left = left.checked_add(straight).unwrap();
        right = right.checked_add(triplet).unwrap();
    }
    assert_eq!(left, Beat::integer(40_000));
    assert_eq!(left, right);
    assert_eq!(left.frames(Tempo::DEFAULT, 48_000).unwrap(), 960_000_000);
}

#[test]
fn quantize_preserves_exact_positions_and_midpoint_policy() {
    let grid = Division::SIXTEENTH.beat_length();
    assert_eq!(
        Beat::new(103, 100).unwrap().quantized(grid).unwrap(),
        Beat::ONE
    );
    assert_eq!(
        Beat::new(9, 8).unwrap().quantized(grid).unwrap(),
        Beat::new(5, 4).unwrap()
    );
    assert_eq!(
        Beat::new(1, 3)
            .unwrap()
            .frames(Tempo::DEFAULT, 48_000)
            .unwrap(),
        8_000
    );
    assert_eq!(
        Beat::from_elapsed_micros(515_000, Tempo::DEFAULT).unwrap(),
        Beat::new(103, 100).unwrap()
    );
}

#[test]
fn deserialization_cannot_bypass_time_invariants() {
    assert!(serde_json::from_str::<Beat>(r#"{"numerator":1,"denominator":0}"#).is_err());
    assert!(serde_json::from_str::<Tempo>("0").is_err());
    assert!(serde_json::from_str::<Meter>("[4,3]").is_err());
    assert!(
        serde_json::from_str::<Division>(r#"{"denominator":16,"notes":0,"in_time_of":2}"#).is_err()
    );
    let time: Beat = serde_json::from_str(r#"{"numerator":6,"denominator":8}"#).unwrap();
    assert_eq!(time, Beat::new(3, 4).unwrap());
}

#[test]
fn arithmetic_reports_overflow_and_negative_results() {
    assert_eq!(
        Beat::integer(u64::MAX).checked_add(Beat::ONE),
        Err(TimeError::Overflow)
    );
    assert_eq!(Beat::ZERO.checked_sub(Beat::ONE), Err(TimeError::Negative));
    assert_eq!(
        Beat::ONE.checked_scale(1, 0),
        Err(TimeError::ZeroDenominator)
    );
    let large = Beat::new(u64::MAX, u64::MAX - 1).unwrap();
    assert!(large > Beat::ONE);
    assert_eq!(
        large.checked_scale(u64::MAX - 1, u64::MAX).unwrap(),
        Beat::ONE
    );
}
