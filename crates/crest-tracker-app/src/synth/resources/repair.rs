//! Retained load candidates. Repair never mutates the active document or an
//! existing library revision; graph activation remains the host's transaction.
use super::*;
use std::path::PathBuf;

pub struct Candidate {
    pub document: Document,
    pub path: PathBuf,
    pub replaced: bool,
    pub error: String,
}

impl Candidate {
    pub fn projection(&self) -> serde_json::Value {
        serde_json::json!({
            "name": self.document.song.name,
            "error": self.error,
            "resources": self.document.resources.iter().enumerate().map(|(index, item)| {
                serde_json::json!({"index":index,"name":item.reference.locator(),
                    "sha256":item.sha256,"kind":item.reference.kind()})
            }).collect::<Vec<_>>()
        })
    }
}

/// Inventories a legacy Song even when some source files are missing. Entries
/// without an expected digest require explicit replacement; they cannot prove
/// that a located file is the original content.
pub fn retain(mut document: Document, path: PathBuf, error: String) -> Result<Candidate, String> {
    if document.resources.is_empty() {
        let saved = session(&document)?;
        document.resources = inventory(&document, &saved)?
            .into_iter()
            .map(|reference| {
                let source = portable_assets::read(&reference).ok();
                let (sha256, bytes) = match source {
                    Some(PortableAsset::Bytes(bytes)) => (Some(digest(&bytes)), bytes.len() as u64),
                    _ => (None, 0),
                };
                Resource {
                    reference,
                    sha256,
                    bytes,
                    file: None,
                    installed: None,
                }
            })
            .collect();
    }
    Ok(Candidate {
        document,
        path,
        replaced: false,
        error,
    })
}

/// Imports only validated native content. Locate verifies the saved digest;
/// Replace deliberately creates new definitions/revisions in this candidate.
pub fn repair(
    original: &Document,
    index: usize,
    selected: &Path,
    replace: bool,
) -> Result<Document, String> {
    let item = original
        .resources
        .get(index)
        .ok_or("Resource selection is stale")?;
    let bytes = portable_assets::read_selected(item.reference.kind(), selected)?;
    if !replace {
        if item.sha256.is_none() {
            return Err("No original content digest is available; use explicit Replace".into());
        }
        verify(item, &PortableAsset::Bytes(bytes.clone()))?;
    }
    let installed = portable_assets::install(item.reference.kind(), &bytes)?;
    let hash = digest(&bytes);
    let mut candidate = original.clone();
    let aliases: Vec<_> = original
        .resources
        .iter()
        .enumerate()
        .filter(|(i, other)| {
            *i == index
                || (item.sha256.is_some()
                    && item.sha256 == other.sha256
                    && item.reference.kind() == other.reference.kind())
        })
        .map(|(i, other)| (i, other.reference.clone()))
        .collect();
    for (index, reference) in aliases {
        if replace {
            replace_references(&mut candidate, &reference, &installed)?;
            let resource = &mut candidate.resources[index];
            resource.reference = installed.clone();
            resource.sha256 = Some(hash.clone());
            resource.bytes = bytes.len() as u64;
        }
    }
    // A replacement may converge with content already present in the manifest.
    // Every matching alias now has verified bytes, including an earlier row
    // that deduplication would otherwise retain with a missing package path.
    let source = PortableAsset::Bytes(bytes);
    for resource in &mut candidate.resources {
        if resource.reference.kind() != installed.kind()
            || resource.sha256.as_deref() != Some(hash.as_str())
        {
            continue;
        }
        verify(resource, &source)?;
        resource.installed = Some(installed.clone());
        resource.file = None;
    }
    // Replacing two aliases with the same bytes can converge on one identity.
    let mut seen = BTreeSet::new();
    candidate
        .resources
        .retain(|item| seen.insert((item.reference.kind(), item.reference.locator().to_owned())));
    candidate.validate()?;
    Ok(candidate)
}

fn replace_references(
    document: &mut Document,
    old: &AssetReference,
    new: &AssetReference,
) -> Result<(), String> {
    let saved = session(document)?;
    if saved.asset_references().contains(old) {
        document.synth = serde_json::to_value(
            saved
                .with_relinked_assets(&[(old.clone(), new.clone())])
                .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    }
    let mut revisions = Vec::new();
    for pinned in &mut document.pinned_sounds {
        let definition = SoundDefinition::from_json(&pinned.definition.to_string())
            .map_err(|e| e.to_string())?;
        if !definition.asset_references().contains(old) {
            continue;
        }
        let definition = definition
            .with_relinked_assets(&[(old.clone(), new.clone())])
            .map_err(|e| e.to_string())?;
        let next =
            pinned.with_definition(serde_json::to_value(definition).map_err(|e| e.to_string())?)?;
        revisions.push((
            pinned.resource.clone(),
            pinned.revision.clone(),
            next.revision.clone(),
        ));
        *pinned = next;
    }
    for instrument in document.song.instruments.values_mut() {
        if let Some(reference) = &mut instrument.resource {
            if let Some((_, _, revision)) = revisions.iter().find(|(resource, revision, _)| {
                *resource == reference.resource && *revision == reference.revision
            }) {
                reference.revision = revision.clone();
            }
        }
    }
    Ok(())
}
