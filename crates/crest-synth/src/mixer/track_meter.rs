use core::fmt;
use serde::{Deserialize, Deserializer, Serialize};

/// Fixed-size pre-gate measurement for one mixer track.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackMeter {
    left_peak: f32,
    right_peak: f32,
    rms: f32,
}

impl TrackMeter {
    pub const ZERO: Self = Self {
        left_peak: 0.0,
        right_peak: 0.0,
        rms: 0.0,
    };

    pub fn new(left_peak: f32, right_peak: f32, rms: f32) -> Result<Self, TrackMeterError> {
        for (field, value) in [
            (TrackMeterField::LeftPeak, left_peak),
            (TrackMeterField::RightPeak, right_peak),
            (TrackMeterField::Rms, rms),
        ] {
            if !value.is_finite() || value < 0.0 {
                return Err(TrackMeterError { field, value });
            }
        }
        Ok(Self {
            left_peak,
            right_peak,
            rms,
        })
    }

    pub const fn left_peak(self) -> f32 {
        self.left_peak
    }

    pub const fn right_peak(self) -> f32 {
        self.right_peak
    }

    pub const fn rms(self) -> f32 {
        self.rms
    }
}

impl<'de> Deserialize<'de> for TrackMeter {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Value {
            left_peak: f32,
            right_peak: f32,
            rms: f32,
        }

        let value = Value::deserialize(deserializer)?;
        Self::new(value.left_peak, value.right_peak, value.rms).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrackMeterField {
    LeftPeak,
    RightPeak,
    Rms,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackMeterError {
    field: TrackMeterField,
    value: f32,
}

impl fmt::Display for TrackMeterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "track meter {:?} must be finite and nonnegative, got {}",
            self.field, self.value
        )
    }
}

impl std::error::Error for TrackMeterError {}

#[cfg(test)]
mod tests {
    use super::TrackMeter;

    #[test]
    fn meter_is_copy_fixed_size_and_validated() {
        fn assert_copy<T: Copy>() {}
        assert_copy::<TrackMeter>();
        assert!(!core::mem::needs_drop::<TrackMeter>());
        let meter = TrackMeter::new(0.5, 0.25, 0.125).unwrap();
        assert_eq!(meter.left_peak(), 0.5);
        assert!(TrackMeter::new(-0.1, 0.0, 0.0).is_err());
        assert!(TrackMeter::new(0.0, f32::NAN, 0.0).is_err());
        assert!(serde_json::from_str::<TrackMeter>(
            r#"{"leftPeak":-1.0,"rightPeak":0.0,"rms":0.0}"#
        )
        .is_err());
    }
}
