use crate::synth::{
    AssetFileId, DecodedSample, SampleAssetError, SampleDecoderPort, SampleEncoding,
    SampleMetadata, MAX_SAMPLE_SCALARS, MAX_SAMPLE_SOURCE_BYTES,
};
use std::io::Cursor;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct WavePreflight {
    channels: u16,
    sample_rate: u32,
    bits_per_sample: u16,
    encoding: SampleEncoding,
    frames: u64,
}

/// Production RIFF/WAVE decoder. Dependency types do not cross this adapter.
#[derive(Clone, Copy, Debug, Default)]
pub struct WavSampleDecoder;

impl WavSampleDecoder {
    /// Reads and validates the admitted RIFF/WAVE metadata without exposing
    /// dependency types. Catalog projection can therefore report format,
    /// duration, and channel state without retaining decoded PCM.
    pub fn metadata(asset: &AssetFileId, bytes: &[u8]) -> Result<SampleMetadata, SampleAssetError> {
        if bytes.is_empty() {
            return Err(SampleAssetError::EmptyFile);
        }
        if u64::try_from(bytes.len()).map_err(|_| SampleAssetError::SourceTooLarge)?
            > MAX_SAMPLE_SOURCE_BYTES
        {
            return Err(SampleAssetError::SourceTooLarge);
        }
        if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
            return Err(SampleAssetError::UnsupportedContainer);
        }
        let preflight = preflight_wave(bytes)?;
        SampleMetadata::new(
            asset.clone(),
            u64::try_from(bytes.len()).map_err(|_| SampleAssetError::SourceTooLarge)?,
            preflight.sample_rate,
            preflight.channels,
            preflight.bits_per_sample,
            preflight.encoding,
            preflight.frames,
        )
    }
}

impl SampleDecoderPort for WavSampleDecoder {
    fn decode(&self, asset: &AssetFileId, bytes: &[u8]) -> Result<DecodedSample, SampleAssetError> {
        let metadata = Self::metadata(asset, bytes)?;

        let mut reader = hound::WavReader::new(Cursor::new(bytes))
            .map_err(|_| SampleAssetError::MalformedWave)?;
        let spec = reader.spec();
        let encoding = match spec.sample_format {
            hound::SampleFormat::Int => SampleEncoding::SignedPcm,
            hound::SampleFormat::Float => SampleEncoding::Float,
        };
        let frames = u64::from(reader.duration());
        if spec.channels != metadata.channels()
            || spec.sample_rate != metadata.sample_rate()
            || spec.bits_per_sample != metadata.bits_per_sample()
            || encoding != metadata.encoding()
            || frames != metadata.frames()
        {
            return Err(SampleAssetError::MalformedWave);
        }
        let scalar_count = usize::try_from(frames)
            .ok()
            .and_then(|frames| frames.checked_mul(usize::from(spec.channels)))
            .ok_or(SampleAssetError::ArithmeticOverflow)?;
        if scalar_count > MAX_SAMPLE_SCALARS {
            return Err(SampleAssetError::AssetScalarCapacityExceeded);
        }
        let mut interleaved = Vec::new();
        interleaved
            .try_reserve_exact(scalar_count)
            .map_err(|_| SampleAssetError::AllocationFailed)?;
        match (encoding, spec.bits_per_sample) {
            (SampleEncoding::SignedPcm, 16) => {
                for sample in reader.samples::<i16>() {
                    interleaved.push(
                        f32::from(sample.map_err(|_| SampleAssetError::MalformedWave)?) / 32_768.0,
                    );
                }
            }
            (SampleEncoding::SignedPcm, bits @ (24 | 32)) => {
                let scale = (1_u64 << (bits - 1)) as f32;
                for sample in reader.samples::<i32>() {
                    interleaved
                        .push(sample.map_err(|_| SampleAssetError::MalformedWave)? as f32 / scale);
                }
            }
            (SampleEncoding::Float, 32) => {
                for sample in reader.samples::<f32>() {
                    let sample = sample.map_err(|_| SampleAssetError::MalformedWave)?;
                    if !sample.is_finite() {
                        return Err(SampleAssetError::NonFinitePcm);
                    }
                    interleaved.push(sample);
                }
            }
            (SampleEncoding::SignedPcm, _) => return Err(SampleAssetError::UnsupportedBitDepth),
            (SampleEncoding::Float, _) => return Err(SampleAssetError::UnsupportedEncoding),
        }
        DecodedSample::new(metadata, interleaved)
    }
}

fn preflight_wave(bytes: &[u8]) -> Result<WavePreflight, SampleAssetError> {
    let declared_riff_bytes = u64::from(read_u32(bytes, 4)?)
        .checked_add(8)
        .ok_or(SampleAssetError::ArithmeticOverflow)?;
    if declared_riff_bytes > MAX_SAMPLE_SOURCE_BYTES {
        return Err(SampleAssetError::SourceTooLarge);
    }
    let mut offset = 12_usize;
    let mut format = None;
    let mut data_bytes = None;
    while offset < bytes.len() {
        let header_end = offset
            .checked_add(8)
            .ok_or(SampleAssetError::ArithmeticOverflow)?;
        if header_end > bytes.len() {
            return Err(SampleAssetError::MalformedWave);
        }
        let chunk_id = &bytes[offset..offset + 4];
        let chunk_size = usize::try_from(read_u32(bytes, offset + 4)?)
            .map_err(|_| SampleAssetError::ArithmeticOverflow)?;
        let payload = header_end;
        let payload_end = payload
            .checked_add(chunk_size)
            .ok_or(SampleAssetError::ArithmeticOverflow)?;
        if chunk_id == b"fmt " {
            if chunk_size < 16 || payload_end > bytes.len() {
                return Err(SampleAssetError::MalformedWave);
            }
            let format_code = read_u16(bytes, payload)?;
            let bits_per_sample = read_u16(bytes, payload + 14)?;
            let encoding = match format_code {
                1 => SampleEncoding::SignedPcm,
                3 => SampleEncoding::Float,
                0xfffe => extensible_encoding(bytes, payload, chunk_size, bits_per_sample)?,
                _ => return Err(SampleAssetError::UnsupportedEncoding),
            };
            format = Some((
                read_u16(bytes, payload + 2)?,
                read_u32(bytes, payload + 4)?,
                read_u16(bytes, payload + 12)?,
                bits_per_sample,
                encoding,
            ));
        } else if chunk_id == b"data" {
            data_bytes = Some(chunk_size);
            if payload_end > bytes.len() {
                // Preserve typed admission failures from hostile headers
                // before classifying an otherwise admitted truncated file as
                // malformed below.
                break;
            }
        }
        if payload_end > bytes.len() {
            return Err(SampleAssetError::MalformedWave);
        }
        offset = payload_end
            .checked_add(chunk_size & 1)
            .ok_or(SampleAssetError::ArithmeticOverflow)?;
    }
    let (channels, sample_rate, block_align, bits_per_sample, encoding) =
        format.ok_or(SampleAssetError::MalformedWave)?;
    let data_bytes = data_bytes.ok_or(SampleAssetError::MalformedWave)?;
    if block_align == 0 || data_bytes % usize::from(block_align) != 0 {
        return Err(SampleAssetError::MalformedWave);
    }
    let frames = u64::try_from(data_bytes / usize::from(block_align))
        .map_err(|_| SampleAssetError::ArithmeticOverflow)?;
    // Reuse the canonical admission oracle before allowing the dependency to
    // allocate or walk the payload.
    SampleMetadata::new(
        AssetFileId::new("preflight.wav")?,
        bytes.len() as u64,
        sample_rate,
        channels,
        bits_per_sample,
        encoding,
        frames,
    )?;
    let scalar_count = usize::try_from(frames)
        .ok()
        .and_then(|frames| frames.checked_mul(usize::from(channels)))
        .ok_or(SampleAssetError::ArithmeticOverflow)?;
    if scalar_count > MAX_SAMPLE_SCALARS {
        return Err(SampleAssetError::AssetScalarCapacityExceeded);
    }
    let expected_align = usize::from(channels)
        .checked_mul(usize::from(bits_per_sample) / 8)
        .ok_or(SampleAssetError::ArithmeticOverflow)?;
    if usize::from(block_align) != expected_align {
        return Err(SampleAssetError::MalformedWave);
    }
    if declared_riff_bytes != bytes.len() as u64 {
        return Err(SampleAssetError::MalformedWave);
    }
    Ok(WavePreflight {
        channels,
        sample_rate,
        bits_per_sample,
        encoding,
        frames,
    })
}

fn extensible_encoding(
    bytes: &[u8],
    payload: usize,
    chunk_size: usize,
    bits_per_sample: u16,
) -> Result<SampleEncoding, SampleAssetError> {
    if chunk_size < 40
        || read_u16(bytes, payload + 16)? < 22
        || read_u16(bytes, payload + 18)? != bits_per_sample
    {
        return Err(SampleAssetError::MalformedWave);
    }
    let guid_start = payload
        .checked_add(24)
        .ok_or(SampleAssetError::ArithmeticOverflow)?;
    let guid_end = guid_start
        .checked_add(16)
        .ok_or(SampleAssetError::ArithmeticOverflow)?;
    let guid = bytes
        .get(guid_start..guid_end)
        .ok_or(SampleAssetError::MalformedWave)?;
    const BASE_GUID_TAIL: [u8; 14] = [
        0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71,
    ];
    if guid[2..] != BASE_GUID_TAIL {
        return Err(SampleAssetError::UnsupportedEncoding);
    }
    match u16::from_le_bytes([guid[0], guid[1]]) {
        1 => Ok(SampleEncoding::SignedPcm),
        3 => Ok(SampleEncoding::Float),
        _ => Err(SampleAssetError::UnsupportedEncoding),
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, SampleAssetError> {
    let end = offset
        .checked_add(2)
        .ok_or(SampleAssetError::ArithmeticOverflow)?;
    let value: [u8; 2] = bytes
        .get(offset..end)
        .ok_or(SampleAssetError::MalformedWave)?
        .try_into()
        .map_err(|_| SampleAssetError::MalformedWave)?;
    Ok(u16::from_le_bytes(value))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, SampleAssetError> {
    let end = offset
        .checked_add(4)
        .ok_or(SampleAssetError::ArithmeticOverflow)?;
    let value: [u8; 4] = bytes
        .get(offset..end)
        .ok_or(SampleAssetError::MalformedWave)?
        .try_into()
        .map_err(|_| SampleAssetError::MalformedWave)?;
    Ok(u32::from_le_bytes(value))
}

#[cfg(test)]
mod tests {
    use super::WavSampleDecoder;
    use crate::synth::{AssetFileId, SampleAssetError, SampleDecoderPort, SampleEncoding};
    use std::io::Cursor;

    fn wav_bytes<T: hound::Sample + Copy>(spec: hound::WavSpec, samples: &[T]) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let cursor = Cursor::new(&mut bytes);
            let mut writer = hound::WavWriter::new(cursor, spec).unwrap();
            for sample in samples {
                writer.write_sample(*sample).unwrap();
            }
            writer.finalize().unwrap();
        }
        bytes
    }

    #[test]
    fn decodes_admitted_mono_pcm16_and_stereo_float32() {
        let decoder = WavSampleDecoder;
        let mono = wav_bytes(
            hound::WavSpec {
                channels: 1,
                sample_rate: 48_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
            &[i16::MIN, 0, i16::MAX],
        );
        let decoded = decoder
            .decode(&AssetFileId::new("mono.wav").unwrap(), &mono)
            .unwrap();
        assert_eq!(decoded.metadata().channels(), 1);
        assert_eq!(decoded.metadata().encoding(), SampleEncoding::SignedPcm);
        assert_eq!(decoded.interleaved().len(), 3);

        let stereo = wav_bytes(
            hound::WavSpec {
                channels: 2,
                sample_rate: 44_100,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
            &[-1.0_f32, 1.0, -0.5, 0.5],
        );
        let decoded = decoder
            .decode(&AssetFileId::new("stereo.wav").unwrap(), &stereo)
            .unwrap();
        assert_eq!(decoded.metadata().channels(), 2);
        assert_eq!(decoded.metadata().frames(), 2);
        assert_eq!(decoded.interleaved(), &[-1.0, 1.0, -0.5, 0.5]);
    }

    #[test]
    fn decodes_admitted_pcm24_and_pcm32_without_dependency_types_escaping() {
        let decoder = WavSampleDecoder;
        for bits in [24, 32] {
            let bytes = wav_bytes(
                hound::WavSpec {
                    channels: 2,
                    sample_rate: 96_000,
                    bits_per_sample: bits,
                    sample_format: hound::SampleFormat::Int,
                },
                &[-1_i32, 0, 1, ((1_i64 << (bits - 1)) - 1) as i32],
            );
            let decoded = decoder
                .decode(&AssetFileId::new(format!("pcm{bits}.wav")).unwrap(), &bytes)
                .unwrap();
            assert_eq!(decoded.metadata().bits_per_sample(), bits);
            assert_eq!(decoded.metadata().channels(), 2);
            assert_eq!(decoded.interleaved().len(), 4);
            assert!(decoded
                .interleaved()
                .iter()
                .all(|sample| sample.is_finite()));
        }
    }

    #[test]
    fn renamed_non_wave_and_non_finite_float_are_typed_rejections() {
        let decoder = WavSampleDecoder;
        let id = AssetFileId::new("renamed.wav").unwrap();
        assert_eq!(decoder.decode(&id, &[]), Err(SampleAssetError::EmptyFile));
        assert_eq!(
            decoder.decode(&id, b"ID3-not-wave"),
            Err(SampleAssetError::UnsupportedContainer)
        );

        let mut float = wav_bytes(
            hound::WavSpec {
                channels: 1,
                sample_rate: 48_000,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
            &[0.25_f32],
        );
        let needle = 0.25_f32.to_le_bytes();
        let offset = float
            .windows(needle.len())
            .position(|window| window == needle)
            .unwrap();
        float[offset..offset + 4].copy_from_slice(&f32::NAN.to_le_bytes());
        assert_eq!(
            decoder.decode(&id, &float),
            Err(SampleAssetError::NonFinitePcm)
        );
    }

    #[test]
    fn rejects_rifx_rf64_channels_rates_depth_and_duration_from_headers() {
        let decoder = WavSampleDecoder;
        let id = AssetFileId::new("invalid.wav").unwrap();
        for magic in [b"RIFX", b"RF64"] {
            let mut bytes = Vec::from(&magic[..]);
            bytes.extend_from_slice(&[0; 4]);
            bytes.extend_from_slice(b"WAVE");
            assert_eq!(
                decoder.decode(&id, &bytes),
                Err(SampleAssetError::UnsupportedContainer)
            );
        }
        let invalid_rate = wav_bytes(
            hound::WavSpec {
                channels: 1,
                sample_rate: 7_999,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
            &[0_i16],
        );
        assert_eq!(
            decoder.decode(&id, &invalid_rate),
            Err(SampleAssetError::UnsupportedSampleRate)
        );

        let channels = wav_bytes(
            hound::WavSpec {
                channels: 3,
                sample_rate: 48_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
            &[0_i16, 0, 0],
        );
        assert_eq!(
            decoder.decode(&id, &channels),
            Err(SampleAssetError::UnsupportedChannelCount)
        );

        let mut oversize = vec![0_u8; 12];
        oversize[0..4].copy_from_slice(b"RIFF");
        oversize[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        oversize[8..12].copy_from_slice(b"WAVE");
        assert_eq!(
            decoder.decode(&id, &oversize),
            Err(SampleAssetError::SourceTooLarge)
        );

        let duration = hostile_header(1, 48_000, 16, 48_000_u32 * 301 * 2, 1);
        assert_eq!(
            decoder.decode(&id, &duration),
            Err(SampleAssetError::DurationTooLong)
        );

        let compressed = hostile_header(1, 48_000, 16, 2, 6);
        assert_eq!(
            decoder.decode(&id, &compressed),
            Err(SampleAssetError::UnsupportedEncoding)
        );

        let unsupported_depth = hostile_header(1, 48_000, 8, 1, 1);
        assert_eq!(
            decoder.decode(&id, &unsupported_depth),
            Err(SampleAssetError::UnsupportedBitDepth)
        );

        let scalar_capacity = hostile_header(2, 192_000, 32, 192_000 * 100 * 8, 1);
        assert_eq!(
            decoder.decode(&id, &scalar_capacity),
            Err(SampleAssetError::AssetScalarCapacityExceeded)
        );

        let mut malformed = Vec::from(&b"RIFF\x0a\x00\x00\x00WAVEfmt \x02\x00\x00\x00\x01\x00"[..]);
        let malformed_riff_size = (malformed.len() - 8) as u32;
        malformed[4..8].copy_from_slice(&malformed_riff_size.to_le_bytes());
        assert_eq!(
            decoder.decode(&id, &malformed),
            Err(SampleAssetError::MalformedWave)
        );
    }

    fn hostile_header(
        channels: u16,
        sample_rate: u32,
        bits: u16,
        data_bytes: u32,
        format_code: u16,
    ) -> Vec<u8> {
        let block_align = channels * (bits / 8);
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36_u32.saturating_add(data_bytes)).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&format_code.to_le_bytes());
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&sample_rate.to_le_bytes());
        bytes.extend_from_slice(&(sample_rate * u32::from(block_align)).to_le_bytes());
        bytes.extend_from_slice(&block_align.to_le_bytes());
        bytes.extend_from_slice(&bits.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_bytes.to_le_bytes());
        bytes
    }
}
