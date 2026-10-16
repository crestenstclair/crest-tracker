//! Worker-side resource manifests and portable directory packages. Native
//! decoding, asset identity and exact relinking remain owned by Synth.
pub mod repair;

use crate::persistence::Document;
use crest_synth::adapter::portable_assets::{self, PortableAsset};
use crest_synth::control::saved_session::{SavedSession, SoundDefinition};
use crest_synth::synth::AssetReference;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

const MAX_PACKAGE_BYTES: u64 = 4 * 1024 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resource {
    pub reference: AssetReference,
    pub sha256: Option<String>,
    pub bytes: u64,
    pub file: Option<String>,
    // A verified library-relative import identity; never a machine root.
    // Retained pins need this alias even after their installed Patch changes.
    #[serde(default)]
    pub installed: Option<AssetReference>,
}

fn key(reference: &AssetReference) -> (crest_synth::synth::AssetKind, &str) {
    (reference.kind(), reference.locator())
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn validate(resources: &[Resource]) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    let mut total = 0_u64;
    for item in resources {
        if !seen.insert(key(&item.reference)) || item.reference.locator().is_empty() {
            return Err("Duplicate or invalid resource identity".into());
        }
        if item.installed.as_ref().is_some_and(|reference| {
            reference.kind() != item.reference.kind() || reference.locator().is_empty()
        }) {
            return Err("Invalid installed resource identity".into());
        }
        match &item.sha256 {
            Some(hash)
                if hash.len() == 64
                    && hash
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) =>
            {
                if item.bytes == 0 || item.bytes > crest_synth::synth::MAX_SAMPLE_SOURCE_BYTES {
                    return Err("Resource exceeds the native source admission budget".into());
                }
                if item
                    .file
                    .as_ref()
                    .is_some_and(|path| path != &format!("assets/{hash}"))
                {
                    return Err("Collected resource path must match its content identity".into());
                }
            }
            None if item.bytes == 0 && item.file.is_none() => {}
            _ => return Err("Invalid resource digest or built-in declaration".into()),
        }
        total = total
            .checked_add(item.bytes)
            .ok_or("Resource size overflow")?;
        if total > MAX_PACKAGE_BYTES {
            return Err("Resources exceed the 4 GiB package admission budget".into());
        }
    }
    Ok(())
}

fn session(document: &Document) -> Result<SavedSession, String> {
    let registry = crest_synth::adapter::production_instruments::production_capability_registry()
        .map_err(|e| e.to_string())?;
    SavedSession::from_json(&document.synth.to_string(), &registry).map_err(|e| e.to_string())
}
fn inventory(document: &Document, saved: &SavedSession) -> Result<Vec<AssetReference>, String> {
    let mut references = saved.asset_references();
    for pinned in &document.pinned_sounds {
        let definition = SoundDefinition::from_json(&pinned.definition.to_string())
            .map_err(|e| e.to_string())?;
        for reference in definition.asset_references() {
            if !references.contains(&reference) {
                references.push(reference);
            }
        }
    }
    Ok(references)
}
fn verify(item: &Resource, source: &PortableAsset) -> Result<(), String> {
    let valid = match source {
        PortableAsset::Builtin => item.sha256.is_none() && item.bytes == 0,
        PortableAsset::Bytes(bytes) => {
            item.bytes == bytes.len() as u64
                && item.sha256.as_deref() == Some(digest(bytes).as_str())
        }
    };
    if valid {
        Ok(())
    } else {
        Err(format!(
            "Resource changed: {}. Locate the original content or explicitly replace the sound.",
            item.reference.locator()
        ))
    }
}
fn read_native(item: &Resource, all: &[Resource]) -> Result<PortableAsset, String> {
    let mut candidates = vec![
        item.installed.as_ref().unwrap_or(&item.reference),
        &item.reference,
    ];
    if item.sha256.is_some() {
        candidates.extend(
            all.iter()
                .filter(|other| {
                    other.sha256 == item.sha256 && other.reference.kind() == item.reference.kind()
                })
                .map(|other| other.installed.as_ref().unwrap_or(&other.reference)),
        );
    }
    let mut error = String::new();
    for reference in candidates {
        match portable_assets::read(reference).and_then(|source| {
            verify(item, &source)?;
            Ok(source)
        }) {
            Ok(source) => return Ok(source),
            Err(failure) => error = failure,
        }
    }
    Err(format!(
        "Unresolved resource {}: {error}",
        item.reference.locator()
    ))
}

fn read_collected(root: &Path, item: &Resource) -> Result<PortableAsset, String> {
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let path = root.join(item.file.as_ref().ok_or("No collected resource path")?);
    let path = path.canonicalize().map_err(|e| e.to_string())?;
    if !path.starts_with(&root) {
        return Err("Collected resource escapes its package".into());
    }
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    if file.metadata().map_err(|e| e.to_string())?.len() != item.bytes {
        return Err("Collected resource size mismatch".into());
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(item.bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let source = PortableAsset::Bytes(bytes);
    verify(item, &source)?;
    Ok(source)
}
fn write_blob(root: &Path, hash: &str, bytes: &[u8]) -> Result<(), String> {
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let assets = root.join("assets");
    fs::create_dir_all(&assets).map_err(|e| e.to_string())?;
    File::open(&root)
        .and_then(|directory| directory.sync_all())
        .map_err(|e| e.to_string())?;
    if !assets
        .canonicalize()
        .map_err(|e| e.to_string())?
        .starts_with(&root)
    {
        return Err("Asset folder escapes package".into());
    }
    let path = assets.join(hash);
    match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut file) => {
            if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) {
                let _ = fs::remove_file(path);
                return Err(error.to_string());
            }
            File::open(assets)
                .and_then(|directory| directory.sync_all())
                .map_err(|e| e.to_string())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            if fs::symlink_metadata(&path)
                .map_err(|e| e.to_string())?
                .file_type()
                .is_symlink()
            {
                return Err("Resource destination is a symlink".into());
            }
            if fs::metadata(&path).map_err(|e| e.to_string())?.len() != bytes.len() as u64 {
                return Err("Existing collected resource has different size".into());
            }
            let existing = fs::read(&path).map_err(|e| e.to_string())?;
            if existing == bytes {
                Ok(())
            } else {
                Err("Existing collected resource has different content".into())
            }
        }
        Err(error) => Err(error.to_string()),
    }
}

/// Normal saves hash exact referenced content. Collection writes immutable
/// blobs first; the caller atomically publishes the document last.
pub fn prepare_save(document: &mut Document, collect: Option<&Path>) -> Result<(), String> {
    let saved = session(document)?;
    let references = inventory(document, &saved)?;
    let mut resources = Vec::new();
    for reference in references {
        let existing = document
            .resources
            .iter()
            .find(|item| item.reference == reference);
        let source = match existing {
            Some(item) => read_native(item, &document.resources)?,
            None => portable_assets::read(&reference)?,
        };
        let (sha256, bytes, file) = match source {
            PortableAsset::Builtin => (None, 0, None),
            PortableAsset::Bytes(bytes) => {
                let hash = digest(&bytes);
                let path = if let Some(root) = collect {
                    write_blob(root, &hash, &bytes)?;
                    Some(format!("assets/{hash}"))
                } else {
                    None
                };
                (Some(hash), bytes.len() as u64, path)
            }
        };
        resources.push(Resource {
            reference,
            sha256,
            bytes,
            file,
            installed: existing.and_then(|item| item.installed.clone()),
        });
        validate(&resources)?;
    }
    document.resources = resources;
    Ok(())
}

/// Verify all bytes before imports; native importers admit each complete
/// resource under its hash. Failed preparation never publishes a sound graph.
pub fn prepare_open(document: &mut Document, path: &Path) -> Result<(), String> {
    let saved = session(document)?;
    let references = inventory(document, &saved)?;
    if document.resources.is_empty() && !references.is_empty() {
        prepare_save(document, None)?;
    }
    validate(&document.resources)?;
    let expected: BTreeSet<_> = references.iter().map(key).collect();
    let actual: BTreeSet<_> = document
        .resources
        .iter()
        .map(|item| key(&item.reference))
        .collect();
    if expected != actual {
        return Err(
            "Resource manifest does not match the Song and pinned sound definitions".into(),
        );
    }
    let root = path.parent().unwrap_or(Path::new("."));
    let read = |item: &Resource| {
        if item.file.is_some() {
            read_collected(root, item)
        } else {
            read_native(item, &document.resources)
        }
    };
    for item in &document.resources {
        read(item)?;
    }
    let mut replacements = Vec::new();
    for item in &document.resources {
        let resolved = match read(item)? {
            PortableAsset::Builtin => item.reference.clone(),
            PortableAsset::Bytes(bytes) => portable_assets::install(item.reference.kind(), &bytes)?,
        };
        replacements.push((item.reference.clone(), resolved));
    }
    let session_refs = saved.asset_references();
    let session_replacements: Vec<_> = replacements
        .iter()
        .filter(|(old, _)| session_refs.contains(old))
        .cloned()
        .collect();
    let saved = saved
        .with_relinked_assets(&session_replacements)
        .map_err(|e| e.to_string())?;
    document.synth = serde_json::to_value(&saved).map_err(|e| e.to_string())?;
    let mut normalized = Vec::new();
    for reference in inventory(document, &saved)? {
        let (index, (_, resolved)) = replacements
            .iter()
            .enumerate()
            .find(|(_, (old, new))| old == &reference || new == &reference)
            .ok_or("Imported resource has no manifest entry")?;
        let mut item = document.resources[index].clone();
        item.reference = reference;
        item.installed = Some(resolved.clone());
        normalized.push(item);
    }
    document.resources = normalized;
    Ok(())
}

/// A published definition binds its file content before its revision is hashed.
/// Import identities are library-relative hashes; future reads verify them.
pub fn freeze_definition(definition: SoundDefinition) -> Result<SoundDefinition, String> {
    let mut replacements = Vec::new();
    for reference in definition.asset_references() {
        let resolved = match portable_assets::read(&reference)? {
            PortableAsset::Builtin => reference.clone(),
            PortableAsset::Bytes(bytes) => portable_assets::install(reference.kind(), &bytes)?,
        };
        replacements.push((reference, resolved));
    }
    definition
        .with_relinked_assets(&replacements)
        .map_err(|e| e.to_string())
}

pub fn resolve_definition(
    definition: SoundDefinition,
    resources: &[Resource],
) -> Result<SoundDefinition, String> {
    let mut replacements = Vec::new();
    for reference in definition.asset_references() {
        if let Some(item) = resources.iter().find(|item| item.reference == reference) {
            let resolved = match read_native(item, resources)? {
                PortableAsset::Builtin => reference.clone(),
                PortableAsset::Bytes(bytes) => portable_assets::install(reference.kind(), &bytes)?,
            };
            replacements.push((reference, resolved));
        } else {
            match portable_assets::read(&reference)? {
                PortableAsset::Builtin => {},
                PortableAsset::Bytes(_) if portable_assets::is_content_addressed(&reference) => {},
                PortableAsset::Bytes(_) => return Err("This legacy sound revision has unpinned file content; open its original Song and publish a new revision first".into()),
            }
        }
    }
    definition
        .with_relinked_assets(&replacements)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crest_synth::synth::AssetKind;

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir()
                .join(format!("crest-resource-lifecycle-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(root.join("samples")).unwrap();
            fs::write(root.join("samples/default.wav"), sample_bytes(0)).unwrap();
            Self(root)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Production catalogs use OnceLock. A fresh child per scenario isolates
    /// both catalog state and environment without touching a user's libraries.
    fn isolated_samples(case: &str, test: impl FnOnce(&Path)) {
        const CASE: &str = "CREST_RESOURCE_TEST_CASE";
        const ROOT: &str = "CREST_RESOURCE_TEST_ROOT";
        if std::env::var(CASE).ok().as_deref() == Some(case) {
            let root =
                std::path::PathBuf::from(std::env::var_os(ROOT).expect("child fixture root"));
            test(&root);
            return;
        }
        let fixture = Fixture::new();
        let module = module_path!().split_once("::").unwrap().1;
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                &format!("{module}::{case}"),
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CASE, case)
            .env(ROOT, &fixture.0)
            .env("CREST_SAMPLE_LIBRARY_ROOT", fixture.0.join("samples"))
            .env("CREST_SAMPLE_DEFAULT_ASSET", "default.wav")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "isolated {case} failed:\n{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            String::from_utf8_lossy(&result.stdout).contains("1 passed"),
            "child test must run"
        );
    }

    fn sample_bytes(level: i16) -> Vec<u8> {
        let samples = 128_u32;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + samples * 2).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&48_000_u32.to_le_bytes());
        bytes.extend_from_slice(&96_000_u32.to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(samples * 2).to_le_bytes());
        for _ in 0..samples {
            bytes.extend_from_slice(&level.to_le_bytes());
        }
        bytes
    }

    fn sample_document(locator: &str) -> Document {
        use crest_synth::adapter::sample_capability::SampleCapability;
        use crest_synth::control::{AppEvent, AppState};
        use crest_synth::kernel::{MidiChannel, PatchId};
        use crest_synth::mixer::{global_parameters::GlobalParameters, patch_output::PatchOutput};
        use crest_synth::synth::{
            AssetFileId, CapabilityRegistry, InstrumentCapabilityProvider, Patch,
        };
        use crest_tracker::composition::{InstrumentId, InstrumentUse, Song};
        let provider = SampleCapability::new(AssetFileId::new(locator).unwrap()).unwrap();
        let registry = CapabilityRegistry::new(vec![provider.descriptor()]).unwrap();
        let patch = Patch::new(
            PatchId::new(1).unwrap(),
            "Lifecycle sample".into(),
            provider.default_config().unwrap(),
            MidiChannel::new(3).unwrap(),
            PatchOutput::default(),
        );
        let mut state = AppState::new(registry, GlobalParameters::new(-3.0).unwrap());
        state.apply(AppEvent::InstallPatches(vec![patch])).unwrap();
        let mut song = Song::default();
        let instrument = InstrumentId::new(1).unwrap();
        song.instruments.insert(
            instrument,
            InstrumentUse {
                name: "Sample".into(),
                resource: None,
            },
        );
        Document::new(
            song,
            serde_json::to_value(SavedSession::capture(&state)).unwrap(),
            [(instrument, 1)].into_iter().collect(),
        )
    }

    fn definition(document: &Document) -> SoundDefinition {
        session(document)
            .unwrap()
            .sound_definition(crest_synth::kernel::PatchId::new(1).unwrap())
            .unwrap()
    }

    fn save_document(
        path: &Path,
        document: Document,
        mode: crate::persistence::SaveMode,
    ) -> Document {
        let worker = crate::persistence::SaveWorker::new();
        worker.submit(path.to_owned(), document, mode).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            if let Some(result) = worker.poll() {
                let result = result.unwrap();
                result.result.unwrap();
                return result.document;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "save worker did not complete"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    fn load_document(path: &Path) -> Document {
        match finish_load(crate::persistence::LoadWorker::start(
            path.to_owned(),
            false,
        )) {
            crate::persistence::Loaded::Song(document) => document,
            crate::persistence::Loaded::Resources { error, .. } => {
                panic!("Song resources could not load: {error}")
            }
            crate::persistence::Loaded::Synth(_) | crate::persistence::Loaded::Imported(_) => {
                panic!("Song worker returned a Synth document")
            }
        }
    }

    fn finish_load(worker: crate::persistence::LoadWorker) -> crate::persistence::Loaded {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            if let Some(result) = worker.poll() {
                return result.unwrap();
            }
            assert!(
                std::time::Instant::now() < deadline,
                "load worker did not complete"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    fn assert_sample_graph_prepares(document: &Document) {
        use crest_synth::adapter::{
            production_instruments, sample_preparer::SamplePreparer,
            wav_sample_decoder::WavSampleDecoder,
        };
        use crest_synth::synth::{EffectCapabilityRegistry, InstrumentPreparer};
        let saved = session(document).unwrap();
        let catalog = production_instruments::production_sample_catalog()
            .unwrap()
            .unwrap();
        let preparers: Vec<Box<dyn InstrumentPreparer>> = vec![Box::new(
            SamplePreparer::new(catalog, std::sync::Arc::new(WavSampleDecoder)).unwrap(),
        )];
        let prepared = saved
            .prepare_restore(
                production_instruments::production_capability_registry().unwrap(),
                EffectCapabilityRegistry::default(),
                &preparers,
                &[],
                crest_synth::real_time::GraphRevision::INITIAL,
                48_000.0,
                64,
            )
            .unwrap();
        assert_eq!(SavedSession::capture(prepared.state()), saved);
    }

    struct DamagedPackage {
        document: Document,
        path: std::path::PathBuf,
        song_bytes: Vec<u8>,
        library_files: Vec<(std::path::PathBuf, Vec<u8>)>,
    }

    impl DamagedPackage {
        fn new(root: &Path) -> Self {
            use crate::persistence::{LoadWorker, Loaded, SaveMode};
            use crest_tracker::composition::{InstrumentId, ResourceRevision};
            let bytes = sample_bytes(1024);
            fs::write(root.join("samples/active.wav"), &bytes).unwrap();
            fs::write(root.join("samples/inactive.wav"), &bytes).unwrap();
            fs::write(root.join("located.wav"), &bytes).unwrap();
            fs::write(root.join("different.wav"), sample_bytes(8192)).unwrap();
            let mut document = sample_document("active.wav");
            let mut library_files = Vec::new();
            for locator in ["active.wav", "inactive.wav"] {
                let pinned = crate::library::publish(
                    &root.join("sounds"),
                    None,
                    serde_json::to_value(definition(&sample_document(locator))).unwrap(),
                )
                .unwrap();
                let path = root
                    .join("sounds")
                    .join(&pinned.resource)
                    .join(format!("{}.crest-sound", pinned.revision));
                library_files.push((path.clone(), fs::read(path).unwrap()));
                document.pinned_sounds.push(pinned);
            }
            let active = &document.pinned_sounds[0];
            document
                .song
                .instruments
                .get_mut(&InstrumentId::new(1).unwrap())
                .unwrap()
                .resource = Some(ResourceRevision {
                resource: active.resource.clone(),
                revision: active.revision.clone(),
            });
            let path = root.join("damaged.crest-songpack/song.crest-song");
            save_document(&path, document, SaveMode::Collect);
            // Recollect after import: the active hash and both logical pins
            // share one blob, but all three aliases must be repaired.
            let imported = load_document(&path);
            let imported_reference = session(&imported).unwrap().asset_references()[0].clone();
            let collected = save_document(&path, imported, SaveMode::UpdateCollection);
            assert_eq!(collected.resources.len(), 3);
            fs::remove_file(root.join("samples/active.wav")).unwrap();
            fs::remove_file(root.join("samples/inactive.wav")).unwrap();
            fs::remove_file(root.join("samples").join(imported_reference.locator())).unwrap();
            fs::remove_dir_all(path.parent().unwrap().join("assets")).unwrap();
            let Loaded::Resources { document, error } =
                finish_load(LoadWorker::start(path.clone(), false))
            else {
                panic!("Missing collected bytes must retain a repair candidate")
            };
            assert!(!error.is_empty());
            assert_eq!(
                serde_json::to_value(&document).unwrap(),
                serde_json::to_value(collected).unwrap()
            );
            let song_bytes = fs::read(&path).unwrap();
            Self {
                document,
                path,
                song_bytes,
                library_files,
            }
        }

        fn assert_originals_unchanged(&self) {
            assert_eq!(fs::read(&self.path).unwrap(), self.song_bytes);
            assert_eq!(
                serde_json::to_value(&self.document).unwrap(),
                serde_json::to_value(Document::read(&self.path).unwrap()).unwrap()
            );
            for (path, bytes) in &self.library_files {
                assert_eq!(&fs::read(path).unwrap(), bytes);
            }
        }
    }

    #[test]
    fn locate_repairs_every_package_alias_only_with_original_bytes() {
        isolated_samples(
            "locate_repairs_every_package_alias_only_with_original_bytes",
            |root| {
                let fixture = DamagedPackage::new(root);
                let original = &fixture.document;
                let index = original
                    .resources
                    .iter()
                    .position(|item| item.reference.locator() == "inactive.wav")
                    .unwrap();
                let error = repair::repair(original, index, &root.join("different.wav"), false)
                    .unwrap_err();
                assert!(error.contains("Resource changed:"), "{error}");
                fixture.assert_originals_unchanged();
                assert!(prepare_open(&mut original.clone(), &fixture.path).is_err());

                let mut repaired =
                    repair::repair(original, index, &root.join("located.wav"), false).unwrap();
                assert_eq!(repaired.synth, original.synth);
                assert_eq!(
                    serde_json::to_value(&repaired.pinned_sounds).unwrap(),
                    serde_json::to_value(&original.pinned_sounds).unwrap()
                );
                assert!(repaired
                    .resources
                    .iter()
                    .all(|item| item.file.is_none() && item.installed.is_some()));
                prepare_open(&mut repaired, &fixture.path).unwrap();
                assert_sample_graph_prepares(&repaired);
                let imported = session(&repaired).unwrap().asset_references()[0].clone();
                assert_eq!(
                    portable_assets::read(&imported).unwrap(),
                    PortableAsset::Bytes(sample_bytes(1024))
                );
                for pin in &repaired.pinned_sounds {
                    let resolved = resolve_definition(
                        SoundDefinition::from_json(&pin.definition.to_string()).unwrap(),
                        &repaired.resources,
                    )
                    .unwrap();
                    assert_eq!(resolved.asset_references(), vec![imported.clone()]);
                }
                let saved_path = root.join("located.crest-song");
                save_document(&saved_path, repaired, crate::persistence::SaveMode::Normal);
                fs::remove_file(root.join("located.wav")).unwrap();
                assert_sample_graph_prepares(&load_document(&saved_path));
                fixture.assert_originals_unchanged();
            },
        );
    }

    #[test]
    fn replace_creates_current_and_inactive_pin_revisions_without_rewriting_originals() {
        isolated_samples(
            "replace_creates_current_and_inactive_pin_revisions_without_rewriting_originals",
            |root| {
                let fixture = DamagedPackage::new(root);
                let original = &fixture.document;
                let index = original
                    .resources
                    .iter()
                    .position(|item| item.reference.locator() == "inactive.wav")
                    .unwrap();
                let mut repaired =
                    repair::repair(original, index, &root.join("different.wav"), true).unwrap();
                assert_ne!(repaired.synth, original.synth);
                assert_eq!(repaired.resources.len(), 1);
                let replacement = &repaired.resources[0];
                assert_eq!(replacement.sha256, Some(digest(&sample_bytes(8192))));
                assert!(replacement.file.is_none());
                let reference = replacement.reference.clone();
                assert_eq!(
                    session(&repaired).unwrap().asset_references(),
                    vec![reference.clone()]
                );
                assert_eq!(repaired.pinned_sounds.len(), original.pinned_sounds.len());
                for (before, after) in original.pinned_sounds.iter().zip(&repaired.pinned_sounds) {
                    after.validate().unwrap();
                    assert_eq!(before.resource, after.resource);
                    assert_ne!(before.revision, after.revision);
                    let definition =
                        SoundDefinition::from_json(&after.definition.to_string()).unwrap();
                    assert_eq!(definition.asset_references(), vec![reference.clone()]);
                    let unpublished = root
                        .join("sounds")
                        .join(&after.resource)
                        .join(format!("{}.crest-sound", after.revision));
                    assert!(
                        !unpublished.exists(),
                        "Replace must not publish a library revision"
                    );
                }
                let instrument = repaired.song.instruments.values().next().unwrap();
                let active = instrument.resource.as_ref().unwrap();
                assert_eq!(active.resource, repaired.pinned_sounds[0].resource);
                assert_eq!(active.revision, repaired.pinned_sounds[0].revision);
                prepare_open(&mut repaired, &fixture.path).unwrap();
                assert_sample_graph_prepares(&repaired);
                assert_eq!(
                    portable_assets::read(&reference).unwrap(),
                    PortableAsset::Bytes(sample_bytes(8192))
                );
                let saved_path = root.join("replaced.crest-song");
                save_document(&saved_path, repaired, crate::persistence::SaveMode::Normal);
                fs::remove_file(root.join("different.wav")).unwrap();
                assert_sample_graph_prepares(&load_document(&saved_path));
                fixture.assert_originals_unchanged();
            },
        );
    }

    #[test]
    fn replace_with_an_existing_identity_keeps_the_verified_import() {
        isolated_samples(
            "replace_with_an_existing_identity_keeps_the_verified_import",
            |root| {
                let bytes = sample_bytes(8192);
                let current = portable_assets::install(AssetKind::Sample, &bytes).unwrap();
                fs::write(root.join("samples/missing.wav"), sample_bytes(1024)).unwrap();
                fs::write(root.join("selected.wav"), &bytes).unwrap();
                let mut document = sample_document(current.locator());
                document.pinned_sounds.push(
                    crate::library::publish(
                        &root.join("sounds"),
                        None,
                        serde_json::to_value(definition(&sample_document("missing.wav"))).unwrap(),
                    )
                    .unwrap(),
                );
                let path = root.join("converged.crest-songpack/song.crest-song");
                let original =
                    save_document(&path, document, crate::persistence::SaveMode::Collect);
                fs::remove_dir_all(path.parent().unwrap().join("assets")).unwrap();
                fs::remove_file(root.join("samples/missing.wav")).unwrap();
                fs::remove_file(root.join("samples").join(current.locator())).unwrap();
                let index = original
                    .resources
                    .iter()
                    .position(|item| item.reference.locator() == "missing.wav")
                    .unwrap();
                let mut repaired =
                    repair::repair(&original, index, &root.join("selected.wav"), true).unwrap();
                assert_eq!(repaired.resources.len(), 1);
                assert_eq!(repaired.resources[0].reference, current);
                assert!(repaired.resources[0].file.is_none());
                prepare_open(&mut repaired, &path).unwrap();
                assert_sample_graph_prepares(&repaired);
                assert_eq!(
                    portable_assets::read(&current).unwrap(),
                    PortableAsset::Bytes(bytes)
                );
            },
        );
    }

    #[test]
    fn missing_legacy_digest_requires_explicit_replace() {
        isolated_samples("missing_legacy_digest_requires_explicit_replace", |root| {
            let original = sample_document("missing.wav");
            let candidate = repair::retain(
                original,
                root.join("legacy.crest-song"),
                "missing source".into(),
            )
            .unwrap();
            assert_eq!(candidate.document.resources.len(), 1);
            assert!(candidate.document.resources[0].sha256.is_none());
            fs::write(root.join("selected.wav"), sample_bytes(1024)).unwrap();
            let error = repair::repair(&candidate.document, 0, &root.join("selected.wav"), false)
                .unwrap_err();
            assert!(error.contains("No original content digest"), "{error}");
            fs::write(root.join("invalid.wav"), b"not a WAV file").unwrap();
            assert!(
                repair::repair(&candidate.document, 0, &root.join("invalid.wav"), true).is_err()
            );
            let mut replaced =
                repair::repair(&candidate.document, 0, &root.join("selected.wav"), true).unwrap();
            prepare_open(&mut replaced, &candidate.path).unwrap();
            assert_sample_graph_prepares(&replaced);
            assert!(candidate.document.resources[0].sha256.is_none());
            assert_eq!(
                definition(&candidate.document).asset_references()[0].locator(),
                "missing.wav"
            );
        });
    }

    #[test]
    fn unused_pin_alias_survives_collect_import_normal_save_and_reopen_without_original() {
        isolated_samples(
            "unused_pin_alias_survives_collect_import_normal_save_and_reopen_without_original",
            |root| {
                use crate::persistence::SaveMode;
                let original_bytes = sample_bytes(1024);
                fs::write(root.join("samples/original.wav"), &original_bytes).unwrap();
                fs::write(root.join("samples/replacement.wav"), sample_bytes(2048)).unwrap();
                let mut document = sample_document("original.wav");
                // Legacy pins keep their logical references; importing a package
                // must not rewrite their hashed, immutable definition.
                let pinned = crate::library::publish(
                    &root.join("sounds"),
                    None,
                    serde_json::to_value(definition(&document)).unwrap(),
                )
                .unwrap();
                document.pinned_sounds.push(pinned.clone());
                let package = root.join("source.crest-songpack");
                let collected = save_document(
                    &package.join("song.crest-song"),
                    document,
                    SaveMode::Collect,
                );
                let logical = AssetReference::new(AssetKind::Sample, "original.wav").unwrap();
                assert_eq!(collected.resources[0].sha256, Some(digest(&original_bytes)));
                assert!(collected.resources[0].file.is_some());
                fs::remove_file(root.join("samples/original.wav")).unwrap();
                let moved = root.join("moved.crest-songpack");
                fs::rename(&package, &moved).unwrap();
                let mut opened = load_document(&moved.join("song.crest-song"));
                assert_sample_graph_prepares(&opened);
                assert_eq!(opened.pinned_sounds[0].definition, pinned.definition);
                let imported = opened
                    .resources
                    .iter()
                    .find(|item| item.reference == logical)
                    .unwrap()
                    .installed
                    .clone()
                    .unwrap();
                assert_ne!(imported, logical);
                assert_eq!(
                    portable_assets::read(&imported).unwrap(),
                    PortableAsset::Bytes(original_bytes.clone())
                );
                opened.synth = sample_document("replacement.wav").synth;
                let normal_path = root.join("normal.crest-song");
                save_document(&normal_path, opened, SaveMode::Normal);
                fs::remove_dir_all(&moved).unwrap();
                let serialized = Document::read(&normal_path).unwrap();
                let unused = serialized
                    .resources
                    .iter()
                    .find(|item| item.reference == logical)
                    .unwrap();
                assert_eq!(unused.installed.as_ref(), Some(&imported));
                assert!(unused.file.is_none());
                let reopened = load_document(&normal_path);
                assert_sample_graph_prepares(&reopened);
                let definition =
                    SoundDefinition::from_json(&reopened.pinned_sounds[0].definition.to_string())
                        .unwrap();
                let resolved = resolve_definition(definition, &reopened.resources).unwrap();
                assert_eq!(resolved.asset_references(), vec![imported]);
                assert_eq!(reopened.pinned_sounds[0].revision, pinned.revision);
                assert_eq!(reopened.pinned_sounds[0].definition, pinned.definition);
            },
        );
    }

    #[test]
    fn frozen_publication_keeps_original_bytes_and_rejects_tampered_import() {
        isolated_samples(
            "frozen_publication_keeps_original_bytes_and_rejects_tampered_import",
            |root| {
                let bytes = sample_bytes(1024);
                let changed = sample_bytes(8192);
                fs::write(root.join("samples/original.wav"), &bytes).unwrap();
                let original = definition(&sample_document("original.wav"));
                let frozen = freeze_definition(original.clone()).unwrap();
                assert_eq!(frozen.instrument().values(), original.instrument().values());
                let reference = frozen.asset_references()[0].clone();
                assert!(portable_assets::is_content_addressed(&reference));
                let published = crate::library::publish(
                    &root.join("sounds"),
                    None,
                    serde_json::to_value(&frozen).unwrap(),
                )
                .unwrap();
                let path = root
                    .join("sounds")
                    .join(&published.resource)
                    .join(format!("{}.crest-sound", published.revision));
                fs::write(root.join("samples/original.wav"), &changed).unwrap();
                let pinned = crate::library::read(&path).unwrap();
                let restored = resolve_definition(
                    SoundDefinition::from_json(&pinned.definition.to_string()).unwrap(),
                    &[],
                )
                .unwrap();
                assert_eq!(restored, frozen);
                assert_eq!(
                    portable_assets::read(&reference).unwrap(),
                    PortableAsset::Bytes(bytes)
                );
                let revised = crate::library::publish(
                    &root.join("sounds"),
                    Some(&published.resource),
                    serde_json::to_value(freeze_definition(original).unwrap()).unwrap(),
                )
                .unwrap();
                assert_ne!(published.revision, revised.revision);
                assert_eq!(
                    crate::library::read(&path).unwrap().definition,
                    pinned.definition
                );
                fs::write(root.join("samples").join(reference.locator()), &changed).unwrap();
                assert!(resolve_definition(frozen, &[]).is_err());
                assert_eq!(
                    crate::library::read(&path).unwrap().revision,
                    published.revision
                );
            },
        );
    }

    #[test]
    fn hash_looking_legacy_filenames_cannot_claim_immutable_revision_content() {
        isolated_samples(
            "hash_looking_legacy_filenames_cannot_claim_immutable_revision_content",
            |root| {
                let bytes = sample_bytes(1024);
                let hash = digest(&bytes);
                fs::create_dir(root.join("samples/Imported")).unwrap();
                for filename in [
                    format!("{hash}edited.wav"),
                    format!("{hash}-x.wav"),
                    format!("{}.wav", hash.to_uppercase()),
                    format!("{hash}.wav_backup"),
                ] {
                    let locator = format!("Imported/{filename}");
                    fs::write(root.join("samples").join(&locator), &bytes).unwrap();
                    let definition = definition(&sample_document(&locator));
                    assert!(
                        resolve_definition(definition, &[]).is_err(),
                        "not an immutable import: {locator}"
                    );
                }
            },
        );
    }

    fn resource(bytes: &[u8]) -> Resource {
        let hash = digest(bytes);
        Resource {
            reference: AssetReference::new(AssetKind::Sample, "source.wav").unwrap(),
            file: Some(format!("assets/{hash}")),
            sha256: Some(hash),
            bytes: bytes.len() as u64,
            installed: None,
        }
    }
    #[test]
    fn manifest_rejects_traversal_duplicates_digest_and_budget_errors() {
        let item = resource(b"sample");
        validate(std::slice::from_ref(&item)).unwrap();
        assert!(validate(&[item.clone(), item.clone()]).is_err());
        let mut bad = item.clone();
        bad.file = Some("../secret".into());
        assert!(validate(&[bad]).is_err());
        let mut bad = item.clone();
        bad.sha256 = Some("bad".into());
        assert!(validate(&[bad]).is_err());
        let mut bad = item;
        bad.bytes = u64::MAX;
        assert!(validate(&[bad]).is_err());
    }
    #[test]
    fn collected_blobs_are_verified_immutable_and_relocatable() {
        let root = std::env::temp_dir().join(format!("crest-resources-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let item = resource(b"original content");
        write_blob(&root, item.sha256.as_ref().unwrap(), b"original content").unwrap();
        assert!(write_blob(&root, item.sha256.as_ref().unwrap(), b"changed").is_err());
        read_collected(&root, &item).unwrap();
        let moved = root.with_extension("moved");
        fs::rename(&root, &moved).unwrap();
        read_collected(&moved, &item).unwrap();
        fs::write(moved.join(item.file.as_ref().unwrap()), b"changed content!").unwrap();
        assert!(read_collected(&moved, &item).is_err());
        fs::remove_dir_all(moved).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn collected_symlinks_cannot_escape_package() {
        let root = std::env::temp_dir().join(format!("crest-resources-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("assets")).unwrap();
        let outside = root.with_extension("outside");
        fs::write(&outside, b"data").unwrap();
        let item = resource(b"data");
        std::os::unix::fs::symlink(&outside, root.join(item.file.as_ref().unwrap())).unwrap();
        assert!(read_collected(&root, &item).is_err());
        assert!(write_blob(&root, item.sha256.as_ref().unwrap(), b"data").is_err());
        fs::remove_dir_all(root).unwrap();
        fs::remove_file(outside).unwrap();
    }
}
