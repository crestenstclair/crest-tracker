//! Validated Yamaha DX7 file import around the original Apache-2.0 MSFA core.
use super::filesystem_file_browser::FilesystemFileBrowser;
use super::filesystem_sample_catalog::read_sample_path;
use crate::kernel::midi_message::MidiMessageKind;
use crate::synth::*;

pub const CAPABILITY: &str = "instrument.msfa.dx7";
pub const FILE: &str = "msfa.file";
pub const PRESET: &str = "msfa.preset";
pub const BUNDLED: &str = "@bundled/msfa-epiano.syx";
extern "C" {
    fn crest_msfa_unpack(packed: *const u8, unpacked: *mut u8);
    fn crest_msfa_default_patch(unpacked: *mut u8);
}
#[derive(Clone)]
pub struct Dx7Preset {
    pub id: String,
    pub name: String,
    pub data: [u8; 155],
}
pub struct Dx7Library {
    pub presets: Vec<Dx7Preset>,
}
impl Dx7Library {
    pub fn bundled() -> Self {
        let mut data = [0; 155];
        unsafe {
            crest_msfa_default_patch(data.as_mut_ptr());
        }
        Self {
            presets: vec![preset(0, 0, data)],
        }
    }
    pub fn parse(bytes: &[u8]) -> Result<Self, SampleAssetError> {
        let invalid = SampleAssetError::MalformedSysEx;
        let mut offset = 0usize;
        let mut message = 0usize;
        let mut presets = Vec::new();
        while offset < bytes.len() {
            let head = bytes.get(offset..offset + 6).ok_or(invalid)?;
            if head[0] != 0xf0 || head[1] != 0x43 || head[2] > 15 || head[4] > 127 || head[5] > 127
            {
                return Err(invalid);
            }
            let length = usize::from(head[4]) * 128 + usize::from(head[5]);
            if !matches!((head[3], length), (0, 155) | (9, 4096)) {
                return Err(invalid);
            }
            let end = offset.checked_add(6 + length).ok_or(invalid)?;
            let payload = bytes.get(offset + 6..end).ok_or(invalid)?;
            let tail = bytes.get(end..end + 2).ok_or(invalid)?;
            if payload.iter().any(|b| *b > 127)
                || tail[0] > 127
                || tail[1] != 0xf7
                || (payload
                    .iter()
                    .fold(u32::from(tail[0]), |s, b| s + u32::from(*b))
                    & 127)
                    != 0
            {
                return Err(invalid);
            }
            if head[3] == 0 {
                let data: [u8; 155] = payload.try_into().map_err(|_| invalid)?;
                validate(&data)?;
                presets.push(preset(message, 0, data));
            } else {
                for (ordinal, packed) in payload.chunks_exact(128).enumerate() {
                    let mut data = [0; 155];
                    unsafe {
                        crest_msfa_unpack(packed.as_ptr(), data.as_mut_ptr());
                    }
                    validate(&data)?;
                    presets.push(preset(message, ordinal, data));
                }
            }
            offset = end + 2;
            message += 1;
        }
        if presets.is_empty() {
            return Err(invalid);
        }
        Ok(Self { presets })
    }
    pub fn descriptor(
        &self,
        reference: AssetReference,
    ) -> Result<CapabilityDescriptor, CapabilityError> {
        let file = ParameterId::new(FILE).unwrap();
        let choices = self
            .presets
            .iter()
            .map(|p| ParameterChoice::new(&p.id, &p.name))
            .collect::<Result<Vec<_>, _>>()?;
        let preset = ParameterSpec::new_with_patch_interaction(
            ParameterId::new(PRESET).unwrap(),
            "Preset",
            ParameterKind::Choice,
            ParameterUpdate::Structural,
            if choices.len() > 1 {
                PatchInteraction::StructuralChoice
            } else {
                PatchInteraction::ReadOnly
            },
            ParameterDefault::Value(ParameterValue::Choice(self.presets[0].id.clone())),
            None,
            choices,
            None,
            None,
            None,
            "choice",
            None,
            None,
        )?;
        let asset = ParameterSpec::new_with_patch_interaction(
            file.clone(),
            "DX7 SysEx Library",
            ParameterKind::Asset,
            ParameterUpdate::Structural,
            PatchInteraction::ReadOnly,
            ParameterDefault::Asset(reference),
            None,
            Vec::new(),
            None,
            None,
            None,
            "asset",
            None,
            None,
        )?;
        Ok(CapabilityDescriptor::new(
            CapabilityId::new(CAPABILITY).unwrap(),
            "MSFA DX7 SysEx",
            "instrument.msfa",
            vec![CapabilitySection::new(
                "library",
                "Library",
                vec![asset, preset],
            )?],
            vec![AssetRequirement::new(file, true)],
            VoicePolicy::Configurable { default_voices: 16 },
            vec![
                MidiMessageKind::NoteOn,
                MidiMessageKind::NoteOff,
                MidiMessageKind::ControlChange,
                MidiMessageKind::PitchBend,
                MidiMessageKind::ChannelPressure,
                MidiMessageKind::AllNotesOff,
            ],
        )?
        .with_asset_scoped_choices())
    }
}
fn preset(message: usize, ordinal: usize, data: [u8; 155]) -> Dx7Preset {
    // Names are display text only. Message/voice positions preserve duplicates.
    let name = data[145..155]
        .iter()
        .map(|b| {
            if (32..=126).contains(b) {
                char::from(*b)
            } else {
                ' '
            }
        })
        .collect::<String>();
    let name = if name.trim().is_empty() {
        "Untitled".into()
    } else {
        name.trim().to_owned()
    };
    Dx7Preset {
        id: format!("message-{message}.voice-{ordinal}"),
        name,
        data,
    }
}
fn validate(data: &[u8; 155]) -> Result<(), SampleAssetError> {
    const OP: [u8; 21] = [
        99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 3, 3, 7, 3, 7, 99, 1, 31, 99, 14,
    ];
    const GLOBAL: [u8; 19] = [
        99, 99, 99, 99, 99, 99, 99, 99, 31, 7, 1, 99, 99, 99, 99, 1, 5, 7, 48,
    ];
    if data[..126]
        .chunks_exact(21)
        .any(|op| op.iter().zip(OP).any(|(v, max)| *v > max))
        || data[126..145].iter().zip(GLOBAL).any(|(v, max)| *v > max)
    {
        return Err(SampleAssetError::MalformedSysEx);
    }
    Ok(())
}
pub fn browser() -> Result<FilesystemFileBrowser, SampleAssetError> {
    let root =
        std::path::PathBuf::from(std::env::var_os("HOME").ok_or(SampleAssetError::Unavailable)?)
            .join("Music/Crest Synth/SysEx");
    std::fs::create_dir_all(&root).map_err(|_| SampleAssetError::Unavailable)?;
    Ok(FilesystemFileBrowser::new(root)?.with_user_locations())
}
pub fn load(reference: &AssetReference) -> Result<Dx7Library, SampleAssetError> {
    if reference.kind() != AssetKind::SysEx {
        return Err(SampleAssetError::MalformedSysEx);
    }
    if reference.locator() == BUNDLED {
        return Ok(Dx7Library::bundled());
    }
    let id = AssetFileId::new(reference.locator())?;
    if id.is_external() {
        return Err(SampleAssetError::InvalidRelativeId);
    }
    Dx7Library::parse(&read_sample_path(&browser()?.resolve(id.as_str())?)?)
}
pub fn import(id: &AssetFileId) -> Result<(AssetFileId, CapabilityDescriptor), SampleAssetError> {
    let browser = browser()?;
    let path = browser.resolve(id.as_str())?;
    let bytes = read_sample_path(&path)?;
    let library = Dx7Library::parse(&bytes)?;
    let id = browser.store_validated_file(&path, &bytes, "syx")?;
    let descriptor = library
        .descriptor(
            AssetReference::new(AssetKind::SysEx, id.as_str())
                .map_err(|_| SampleAssetError::InvalidRelativeId)?,
        )
        .map_err(|_| SampleAssetError::MalformedSysEx)?;
    Ok((id, descriptor))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn message(format: u8, payload: &[u8]) -> Vec<u8> {
        let mut out = vec![
            0xf0,
            0x43,
            0,
            format,
            (payload.len() / 128) as u8,
            (payload.len() % 128) as u8,
        ];
        out.extend_from_slice(payload);
        out.push((128 - (payload.iter().map(|x| u32::from(*x)).sum::<u32>() & 127)) as u8 & 127);
        out.push(0xf7);
        out
    }
    #[test]
    fn bank_messages_preserve_all_voices_and_expose_stable_preset_choices() {
        let mut packed = vec![0u8; 4096];
        for (index, voice) in packed.chunks_exact_mut(128).enumerate() {
            voice[110] = index as u8;
            voice[117] = 24;
            voice[118..128].copy_from_slice(b"DUPLICATE ");
        }
        let mut bytes = message(9, &packed);
        bytes.extend(message(9, &packed));
        let library = Dx7Library::parse(&bytes).unwrap();
        assert_eq!(library.presets.len(), 64);
        for (index, preset) in library.presets.iter().enumerate() {
            assert_eq!(
                preset.id,
                format!("message-{}.voice-{}", index / 32, index % 32)
            );
            assert_eq!(preset.name, "DUPLICATE");
            assert_eq!(preset.data[134], (index % 32) as u8);
        }
        let reference = AssetReference::new(AssetKind::SysEx, "test-bank.syx").unwrap();
        let descriptor = library.descriptor(reference).unwrap();
        let preset = descriptor
            .parameter(&ParameterId::new(PRESET).unwrap())
            .unwrap();
        assert_eq!(preset.choices().len(), 64);
        assert_eq!(
            preset.patch_interaction(),
            PatchInteraction::StructuralChoice
        );
        let mut malformed = packed;
        malformed[110] = 32;
        assert_eq!(
            Dx7Library::parse(&message(9, &malformed)).err(),
            Some(SampleAssetError::MalformedSysEx)
        );
    }

    #[test]
    fn single_and_multiple_messages_preserve_authored_names_and_duplicate_identity() {
        let default = Dx7Library::bundled();
        let mut bytes = message(0, &default.presets[0].data);
        bytes.extend(message(0, &default.presets[0].data));
        let library = Dx7Library::parse(&bytes).unwrap();
        assert_eq!(library.presets.len(), 2);
        assert_eq!(library.presets[0].name, "E.PIANO 1");
        assert_eq!(library.presets[0].name, library.presets[1].name);
        assert_ne!(library.presets[0].id, library.presets[1].id);
        assert_eq!(library.presets[0].data, default.presets[0].data);
    }
    #[test]
    fn rejects_truncation_checksum_status_and_unsafe_parameter_values() {
        let default = Dx7Library::bundled();
        let bytes = message(0, &default.presets[0].data);
        for n in 0..bytes.len() {
            assert!(Dx7Library::parse(&bytes[..n]).is_err());
        }
        for index in [0, 1, 2, 3, 4, 5, 6, bytes.len() - 2, bytes.len() - 1] {
            let mut bad = bytes.clone();
            bad[index] ^= 0x80;
            assert!(Dx7Library::parse(&bad).is_err());
        }
        let mut bad = default.presets[0].data;
        bad[134] = 32;
        assert!(Dx7Library::parse(&message(0, &bad)).is_err());
    }
}
