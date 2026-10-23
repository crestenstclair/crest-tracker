use super::PlaybackError;
use crate::time::{Beat, Tempo, TimeError};
use num_rational::BigRational;
use num_traits::ToPrimitive;

const MAX_CHANGES: usize = 65_536;
const MAX_TIME_BITS: u64 = 16_384;

#[derive(Debug, Clone)]
struct Change {
    at: Beat,
    tempo: Tempo,
    seconds: BigRational,
}

/// Exact accumulated tempo segments, rounded only at the audio-frame boundary.
/// Unrelated tempo denominators must not overflow the Song's u64 beat format.
/// This map belongs to the control thread, never the real-time audio callback.
#[derive(Debug, Clone)]
pub struct TempoMap {
    initial: Tempo,
    changes: Vec<Change>,
    budget_exceeded: bool,
}

impl TempoMap {
    pub fn new(initial: Tempo) -> Self {
        Self {
            initial,
            changes: Vec::new(),
            budget_exceeded: false,
        }
    }

    pub fn insert(&mut self, at: Beat, tempo: Tempo) {
        if self.budget_exceeded {
            return;
        }
        let index = match self.changes.binary_search_by_key(&at, |change| change.at) {
            Ok(index) => {
                self.changes[index].tempo = tempo;
                index
            }
            Err(index) => {
                if self.changes.len() == MAX_CHANGES {
                    self.budget_exceeded = true;
                    return;
                }
                self.changes.insert(
                    index,
                    Change {
                        at,
                        tempo,
                        seconds: rational(Beat::ZERO),
                    },
                );
                index
            }
        };
        // Cache cumulative time once per edit, rather than resumming the entire
        // performance for every scheduled note and cursor observation.
        for index in index..self.changes.len() {
            let seconds = self.seconds_in_segment(self.changes[index].at, index);
            if seconds.numer().bits().max(seconds.denom().bits()) > MAX_TIME_BITS {
                self.budget_exceeded = true;
                return;
            }
            self.changes[index].seconds = seconds;
        }
    }

    pub fn frame_at(&self, at: Beat, rate: u32) -> Result<u64, PlaybackError> {
        self.admit(rate)?;
        let index = self.changes.partition_point(|change| change.at <= at);
        let frames = self.seconds_in_segment(at, index) * BigRational::from_integer(rate.into());
        frames
            .round()
            .to_integer()
            .to_u64()
            .ok_or(TimeError::Overflow.into())
    }

    pub fn beat_at_frame(&self, frame: u64, rate: u32) -> Result<Beat, PlaybackError> {
        self.admit(rate)?;
        let seconds = BigRational::new(frame.into(), rate.into());
        let index = self
            .changes
            .partition_point(|change| change.seconds <= seconds);
        let (previous, tempo, start) = self.segment(index);
        let at = rational(previous)
            + (seconds - start) * BigRational::new(tempo.milli_bpm().into(), 60_000.into());
        if let Some(beat) = representable(&at) {
            return Ok(beat);
        }

        // Only a sampled host cursor may need projection back into u64 beats.
        // Floor to 1/1024 of one audio frame at this tempo, clamped to the
        // segment start. Source notes and forward frame scheduling stay exact.
        let quantum = BigRational::new(
            tempo.milli_bpm().into(),
            (u64::from(rate) * 60_000 * 1024).into(),
        );
        let units = (&at / &quantum).to_integer();
        let projected = (quantum * BigRational::from_integer(units)).max(rational(previous));
        representable(&projected).ok_or(TimeError::Overflow.into())
    }

    fn segment(&self, index: usize) -> (Beat, Tempo, BigRational) {
        match index.checked_sub(1).map(|index| &self.changes[index]) {
            Some(change) => (change.at, change.tempo, change.seconds.clone()),
            None => (Beat::ZERO, self.initial, rational(Beat::ZERO)),
        }
    }

    fn admit(&self, rate: u32) -> Result<(), PlaybackError> {
        if rate == 0 {
            return Err(TimeError::ZeroDenominator.into());
        }
        if self.budget_exceeded {
            return Err(PlaybackError::Budget(
                "tempo map exceeds exact-time admission budget",
            ));
        }
        Ok(())
    }

    fn seconds_in_segment(&self, at: Beat, index: usize) -> BigRational {
        let (previous, tempo, seconds) = self.segment(index);
        seconds
            + (rational(at) - rational(previous))
                * BigRational::new(60_000.into(), tempo.milli_bpm().into())
    }
}

fn rational(beat: Beat) -> BigRational {
    BigRational::new(beat.numerator().into(), beat.denominator().into())
}

fn representable(value: &BigRational) -> Option<Beat> {
    Beat::new(value.numer().to_u64()?, value.denom().to_u64()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_maps_fail_queries_without_unbounded_growth() {
        let mut map = TempoMap::new(Tempo::DEFAULT);
        for beat in 0..=MAX_CHANGES {
            map.insert(Beat::integer(beat as u64), Tempo::DEFAULT);
        }
        assert_eq!(map.changes.len(), MAX_CHANGES);
        assert!(matches!(
            map.frame_at(Beat::ONE, 48_000),
            Err(PlaybackError::Budget(_))
        ));
        assert!(matches!(
            map.beat_at_frame(1, 48_000),
            Err(PlaybackError::Budget(_))
        ));
    }
}
