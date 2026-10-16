use crate::synth::{
    DecodedSample, PreparedSamplePcm, SampleAssetError, WaveformPair, MAX_SAMPLE_GRAPH_PCM_BYTES,
    MAX_SAMPLE_SCALARS, MAX_WAVEFORM_PAIRS,
};
use std::collections::HashSet;
use std::sync::Arc;

/// Off-callback device-rate preparation with bounded linear resampling.
pub fn prepare_sample_pcm(
    decoded: DecodedSample,
    device_sample_rate: u32,
) -> Result<PreparedSamplePcm, SampleAssetError> {
    if device_sample_rate == 0 {
        return Err(SampleAssetError::UnsupportedSampleRate);
    }
    let metadata = decoded.metadata().clone();
    let channels = usize::from(metadata.channels());
    let source_frames =
        usize::try_from(metadata.frames()).map_err(|_| SampleAssetError::ArithmeticOverflow)?;
    if source_frames == 0 {
        return Err(SampleAssetError::MalformedPcm);
    }
    let target_frames = source_frames
        .checked_mul(
            usize::try_from(device_sample_rate)
                .map_err(|_| SampleAssetError::ArithmeticOverflow)?,
        )
        .ok_or(SampleAssetError::ArithmeticOverflow)?
        .checked_add(usize::try_from(metadata.sample_rate() / 2).unwrap_or(0))
        .ok_or(SampleAssetError::ArithmeticOverflow)?
        / usize::try_from(metadata.sample_rate())
            .map_err(|_| SampleAssetError::ArithmeticOverflow)?;
    let target_frames = target_frames.max(1);
    let target_scalars = target_frames
        .checked_mul(channels)
        .ok_or(SampleAssetError::ArithmeticOverflow)?;
    if target_scalars > MAX_SAMPLE_SCALARS {
        return Err(SampleAssetError::AssetScalarCapacityExceeded);
    }
    let mut output = Vec::new();
    output
        .try_reserve_exact(target_scalars)
        .map_err(|_| SampleAssetError::AllocationFailed)?;
    let source = decoded.interleaved();
    for target_frame in 0..target_frames {
        let position = if target_frames == 1 || source_frames == 1 {
            0.0
        } else {
            (target_frame as f64) * ((source_frames - 1) as f64) / ((target_frames - 1) as f64)
        };
        let left = position.floor() as usize;
        let right = (left + 1).min(source_frames - 1);
        let fraction = (position - left as f64) as f32;
        for channel in 0..channels {
            let a = source[left * channels + channel];
            let b = source[right * channels + channel];
            output.push(a + (b - a) * fraction);
        }
    }
    if output.iter().any(|sample| !sample.is_finite()) {
        return Err(SampleAssetError::NonFinitePcm);
    }
    let waveform = summarize_waveform(&output, metadata.channels())?;
    PreparedSamplePcm::new(
        metadata.asset_id().clone(),
        device_sample_rate,
        metadata.channels(),
        Arc::from(output),
        Arc::from(waveform),
    )
}

pub fn summarize_waveform(
    interleaved: &[f32],
    channels: u16,
) -> Result<Vec<WaveformPair>, SampleAssetError> {
    if !(1..=2).contains(&channels) || interleaved.is_empty() {
        return Err(SampleAssetError::MalformedPcm);
    }
    let channels = usize::from(channels);
    if !interleaved.len().is_multiple_of(channels)
        || interleaved.iter().any(|value| !value.is_finite())
    {
        return Err(SampleAssetError::MalformedPcm);
    }
    let frames = interleaved.len() / channels;
    let pairs = frames.min(MAX_WAVEFORM_PAIRS);
    let mut summary = Vec::new();
    summary
        .try_reserve_exact(pairs)
        .map_err(|_| SampleAssetError::AllocationFailed)?;
    for pair in 0..pairs {
        let start = pair * frames / pairs;
        let end = ((pair + 1) * frames / pairs).max(start + 1).min(frames);
        let mut left_min = f32::INFINITY;
        let mut left_max = f32::NEG_INFINITY;
        let mut right_min = f32::INFINITY;
        let mut right_max = f32::NEG_INFINITY;
        for frame in start..end {
            let left = interleaved[frame * channels];
            let right = if channels == 1 {
                left
            } else {
                interleaved[frame * channels + 1]
            };
            left_min = left_min.min(left);
            left_max = left_max.max(left);
            right_min = right_min.min(right);
            right_max = right_max.max(right);
        }
        summary.push(WaveformPair {
            left_min,
            left_max,
            right_min,
            right_max,
        });
    }
    Ok(summary)
}

/// Validates one complete graph's deduplicated Sample PCM ownership.
pub fn validate_sample_graph_budget<'a>(
    assets: impl IntoIterator<Item = &'a PreparedSamplePcm>,
) -> Result<usize, SampleAssetError> {
    let mut identities = HashSet::new();
    let mut bytes = 0_usize;
    for asset in assets {
        let identity = (
            asset.asset_id().clone(),
            asset.sample_rate(),
            asset.channels(),
            asset.frames(),
        );
        if identities.insert(identity) {
            bytes = bytes
                .checked_add(asset.byte_len())
                .ok_or(SampleAssetError::GraphPcmCapacityExceeded)?;
            if bytes > MAX_SAMPLE_GRAPH_PCM_BYTES {
                return Err(SampleAssetError::GraphPcmCapacityExceeded);
            }
        }
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::{prepare_sample_pcm, summarize_waveform, validate_sample_graph_budget};
    use crate::synth::{
        AssetFileId, DecodedSample, SampleEncoding, SampleMetadata, MAX_WAVEFORM_PAIRS,
    };

    fn decoded(channels: u16, rate: u32, values: Vec<f32>) -> DecodedSample {
        let frames = values.len() as u64 / u64::from(channels);
        DecodedSample::new(
            SampleMetadata::new(
                AssetFileId::new("fixture.wav").unwrap(),
                128,
                rate,
                channels,
                32,
                SampleEncoding::Float,
                frames,
            )
            .unwrap(),
            values,
        )
        .unwrap()
    }

    #[test]
    fn resampling_preserves_endpoints_duration_and_channel_shape() {
        let mono = prepare_sample_pcm(decoded(1, 24_000, vec![-1.0, 0.0, 1.0]), 48_000).unwrap();
        assert_eq!(mono.channels(), 1);
        assert_eq!(mono.frames(), 6);
        assert_eq!(mono.interleaved().first(), Some(&-1.0));
        assert_eq!(mono.interleaved().last(), Some(&1.0));

        let stereo =
            prepare_sample_pcm(decoded(2, 48_000, vec![-1.0, 1.0, -0.5, 0.5]), 48_000).unwrap();
        assert_eq!(stereo.channels(), 2);
        assert_eq!(stereo.interleaved(), &[-1.0, 1.0, -0.5, 0.5]);
    }

    #[test]
    fn waveform_is_bounded_covers_both_endpoints_and_duplicates_mono() {
        let values = (0..5_000)
            .map(|index| index as f32 / 5_000.0)
            .collect::<Vec<_>>();
        let summary = summarize_waveform(&values, 1).unwrap();
        assert_eq!(summary.len(), MAX_WAVEFORM_PAIRS);
        assert_eq!(summary[0].left_min, values[0]);
        assert_eq!(summary.last().unwrap().left_max, *values.last().unwrap());
        assert!(summary
            .iter()
            .all(|pair| pair.left_min == pair.right_min && pair.left_max == pair.right_max));
    }

    #[test]
    fn graph_budget_deduplicates_same_preparation_identity() {
        let asset = prepare_sample_pcm(decoded(1, 48_000, vec![0.0, 1.0]), 48_000).unwrap();
        assert_eq!(
            validate_sample_graph_budget([&asset, &asset]).unwrap(),
            asset.byte_len()
        );
    }
}
