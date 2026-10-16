/// Build-time patch storage budget shared by admission and every RT transport.
///
/// This is a structural audio capacity, not a navigation limit: interaction
/// may still represent the trailing empty Patch position when all slots are
/// occupied. Configure for the target hardware with CREST_SYNTH_PATCH_CAPACITY.
/// Fixed-size snapshots remain Copy and callback-safe; changing this budget
/// requires rebuilding, not allocating or resizing in the callback.
pub const MAX_ACTIVE_PATCHES: usize = match option_env!("CREST_SYNTH_PATCH_CAPACITY") {
    Some(value) => parse_capacity(value),
    None => 32,
};

const fn parse_capacity(value: &str) -> usize {
    let bytes = value.as_bytes();
    let mut result = 0_usize;
    let mut index = 0;
    while index < bytes.len() {
        assert!(
            bytes[index] >= b'0' && bytes[index] <= b'9',
            "CREST_SYNTH_PATCH_CAPACITY must be a positive integer"
        );
        let multiplied = match result.checked_mul(10) {
            Some(value) => value,
            None => panic!("CREST_SYNTH_PATCH_CAPACITY overflow"),
        };
        result = match multiplied.checked_add((bytes[index] - b'0') as usize) {
            Some(value) => value,
            None => panic!("CREST_SYNTH_PATCH_CAPACITY overflow"),
        };
        index += 1;
    }
    assert!(result > 0, "CREST_SYNTH_PATCH_CAPACITY must be positive");
    result
}

#[cfg(test)]
mod tests {
    use super::parse_capacity;

    #[test]
    fn prepared_budget_accepts_hardware_specific_capacities() {
        assert_eq!(parse_capacity("16"), 16);
        assert_eq!(parse_capacity("32"), 32);
        assert_eq!(parse_capacity("64"), 64);
    }

    #[test]
    #[should_panic(expected = "must be positive")]
    fn zero_capacity_is_rejected() {
        parse_capacity("0");
    }

    #[test]
    #[should_panic(expected = "must be a positive integer")]
    fn malformed_capacity_is_rejected() {
        parse_capacity("32x");
    }
}
