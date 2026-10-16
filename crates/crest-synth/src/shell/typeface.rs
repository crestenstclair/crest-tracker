//! The authored typeface as a typed, verifiable asset.
//!
//! The four vendored Azeret Mono faces are the only faces the product ever
//! renders: the projection page binds one `@font-face` per authored weight
//! and the webview window serves exactly the vendored bytes. This module
//! owns the vocabulary around that asset — the family name, the per-weight
//! face files, and the typed failure an unavailable or unreadable face
//! raises.
//!
//! An unavailable face is a typed error naming the file. It is never
//! absorbed by a system font stack, never substituted, and never
//! synthesized: a substituted face would let the product claim the authored
//! design while not rendering it.
//!
//! Realizes `valueObject.Shell.AuthoredTypeface` over `asset.AzeretMonoTypeface`.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::shell::tokens::{FontWeight, ALL_WEIGHTS};

/// The authored family, as the design file names it.
pub const AUTHORED_FAMILY: &str = "Azeret Mono";

/// The vendored directory holding the four static faces, relative to the
/// repository root.
pub const VENDOR_DIR: &str = "vendor/azeret-mono";

/// Why the authored typeface could not be installed.
///
/// Every variant names the face at fault, so the failure the shell surfaces is
/// actionable rather than merely red.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TypefaceError {
    /// The face file does not exist or could not be read.
    FaceUnavailable {
        /// The weight whose face is missing.
        weight: FontWeight,
        /// The path that was read.
        path: PathBuf,
        /// The underlying reason, as reported by the filesystem.
        reason: String,
    },
    /// The face file exists but carries no usable font data.
    FaceUnreadable {
        /// The weight whose face is unreadable.
        weight: FontWeight,
        /// The path that was read.
        path: PathBuf,
    },
}

impl fmt::Display for TypefaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FaceUnavailable {
                weight,
                path,
                reason,
            } => write!(
                f,
                "{AUTHORED_FAMILY} {} is unavailable at {}: {reason}",
                weight_face_name(*weight),
                path.display()
            ),
            Self::FaceUnreadable { weight, path } => write!(
                f,
                "{AUTHORED_FAMILY} {} at {} carries no usable font data",
                weight_face_name(*weight),
                path.display()
            ),
        }
    }
}

impl std::error::Error for TypefaceError {}

/// The four authored faces, read and ready to register.
///
/// Constructing this value is the only way to install the typeface, so a
/// missing face is caught before any frame is painted rather than after.
#[derive(Clone, Debug)]
pub struct AuthoredTypeface {
    faces: BTreeMap<FontWeight, Vec<u8>>,
}

impl AuthoredTypeface {
    /// Reads the four faces from the vendored directory beside the crate.
    ///
    /// The faces are read at runtime rather than embedded with `include_bytes!`
    /// so the unavailable-face path stays reachable and testable. Embedding
    /// would turn a missing file into a compile error and make the typed
    /// failure impossible to prove.
    pub fn load() -> Result<Self, TypefaceError> {
        Self::load_from_dir(default_vendor_dir())
    }

    /// Reads the four faces from an explicit directory.
    ///
    /// Point this at a nonexistent path to exercise the failure path without
    /// mutating the vendored files.
    pub fn load_from_dir(dir: impl AsRef<Path>) -> Result<Self, TypefaceError> {
        let dir = dir.as_ref();
        let mut faces = BTreeMap::new();
        for weight in ALL_WEIGHTS {
            let path = dir.join(face_file_name(weight));
            let bytes = std::fs::read(&path).map_err(|error| TypefaceError::FaceUnavailable {
                weight,
                path: path.clone(),
                reason: error.to_string(),
            })?;
            if bytes.is_empty() {
                return Err(TypefaceError::FaceUnreadable { weight, path });
            }
            faces.insert(weight, bytes);
        }
        Ok(Self { faces })
    }

    /// Returns the weights this value carries.
    pub fn registered_weights(&self) -> Vec<FontWeight> {
        self.faces.keys().copied().collect()
    }
}

/// Returns the per-weight display name of an authored face, e.g.
/// `Azeret Mono SemiBold`.
pub fn family_name(weight: FontWeight) -> String {
    format!("{AUTHORED_FAMILY} {}", weight_face_name(weight))
}

/// Returns the vendored file name for an authored weight.
fn face_file_name(weight: FontWeight) -> String {
    format!("AzeretMono-{}.ttf", weight_face_name(weight))
}

/// Returns the upstream face name for an authored weight.
const fn weight_face_name(weight: FontWeight) -> &'static str {
    match weight {
        FontWeight::Regular => "Regular",
        FontWeight::Medium => "Medium",
        FontWeight::SemiBold => "SemiBold",
        FontWeight::Bold => "Bold",
    }
}

/// Returns the vendored directory beside the crate manifest.
fn default_vendor_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(VENDOR_DIR)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use sha2::{Digest, Sha256};

    use super::*;
    use crate::shell::tokens::ALL_TYPE_STYLES;

    /// The vendored directory, resolved beside the crate manifest.
    fn vendor_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(VENDOR_DIR)
    }

    /// Every face the manifest lists still hashes to the value it was vendored
    /// with. A silently replaced face fails here rather than shipping.
    #[test]
    fn every_vendored_file_matches_its_recorded_hash() {
        let root = vendor_root();
        let manifest = std::fs::read_to_string(root.join("SHA256SUMS")).expect("SHA256SUMS");
        let mut listed = BTreeSet::new();
        for line in manifest.lines().filter(|line| !line.trim().is_empty()) {
            let (expected, name) = line.split_once("  ").expect("`<hash>  <path>` per line");
            let bytes = std::fs::read(root.join(name))
                .unwrap_or_else(|_| panic!("{name} is listed in SHA256SUMS but absent"));
            let actual = format!("{:x}", Sha256::digest(bytes));
            assert_eq!(actual, expected, "{name} does not match its recorded hash");
            listed.insert(name.to_owned());
        }
        assert!(
            listed.contains("AzeretMono[wght].ttf"),
            "the upstream variable source must stay in the manifest as the provenance record"
        );
        for weight in ALL_WEIGHTS {
            assert!(
                listed.contains(&face_file_name(weight)),
                "{} is registered but unhashed",
                face_file_name(weight)
            );
        }
    }

    /// The manifest is complete. A face added to the directory without being
    /// hashed fails here, so the previous test cannot be satisfied by omission.
    #[test]
    fn the_manifest_covers_every_vendored_file() {
        let root = vendor_root();
        let manifest = std::fs::read_to_string(root.join("SHA256SUMS")).expect("SHA256SUMS");
        let mut expected: BTreeSet<String> = manifest
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                line.split_once("  ")
                    .expect("`<hash>  <path>`")
                    .1
                    .to_owned()
            })
            .collect();
        // The manifest cannot hash itself, and the provenance record is prose.
        expected.insert("SHA256SUMS".to_owned());
        expected.insert("PROVENANCE.md".to_owned());

        let actual: BTreeSet<String> = std::fs::read_dir(&root)
            .expect("vendored directory")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(
            actual, expected,
            "the vendored directory and its manifest disagree"
        );
    }

    /// The license ships verbatim, as `NFR-006` and `C-007` require.
    #[test]
    fn the_open_font_license_ships_verbatim() {
        let license = std::fs::read_to_string(vendor_root().join("OFL.txt")).expect("OFL.txt");
        assert!(
            license.contains("SIL OPEN FONT LICENSE Version 1.1"),
            "the license header is not the verbatim OFL 1.1"
        );
        assert!(
            license
                .contains("Permission is hereby granted, free of charge, to any person obtaining"),
            "the OFL permission grant is missing"
        );
        assert!(
            license.contains("Copyright 2021 The Azeret Project Authors"),
            "the upstream copyright line is missing"
        );
    }

    /// The provenance record names the upstream source and the reproducible
    /// derivation for the weights not shipped upstream as static faces.
    #[test]
    fn the_provenance_record_names_the_upstream_source_and_derivation() {
        let provenance =
            std::fs::read_to_string(vendor_root().join("PROVENANCE.md")).expect("PROVENANCE.md");
        assert!(
            provenance.contains("github.com/google/fonts/tree/main/ofl/azeretmono"),
            "the upstream source is not recorded"
        );
        assert!(
            provenance.contains("SIL Open Font License 1.1"),
            "the license is not recorded"
        );
        assert!(
            provenance.contains("varLib.instancer"),
            "the derivation procedure for the static weights is not recorded"
        );
        for weight in ALL_WEIGHTS {
            let numeric = weight.numeric().to_string();
            assert!(
                provenance.contains(&numeric),
                "the derivation does not name the wght axis value {numeric}"
            );
        }
    }

    #[test]
    fn all_four_authored_weights_register() {
        let typeface = AuthoredTypeface::load().expect("vendored faces are present");
        assert_eq!(
            typeface.registered_weights(),
            vec![
                FontWeight::Regular,
                FontWeight::Medium,
                FontWeight::SemiBold,
                FontWeight::Bold,
            ]
        );
        for weight in ALL_WEIGHTS {
            assert!(
                family_name(weight).starts_with(AUTHORED_FAMILY),
                "{} does not carry the authored family",
                family_name(weight)
            );
        }
    }

    /// Every authored type style is set in a weight whose face is vendored,
    /// so no style can resolve to a face the product does not ship.
    #[test]
    fn every_type_style_resolves_to_a_vendored_weight() {
        let typeface = AuthoredTypeface::load().expect("vendored faces are present");
        let weights = typeface.registered_weights();
        for style in ALL_TYPE_STYLES {
            assert!(
                weights.contains(&style.metrics().weight),
                "{} is set in a weight with no vendored face",
                style.canonical_name()
            );
        }
    }

    /// The negative assertion is the one that matters. A happy-path-only test
    /// would have passed before this mission, when no typeface existed at all.
    #[test]
    fn an_unavailable_face_is_a_typed_error_naming_it() {
        let missing = Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor/no-such-typeface");
        let error = AuthoredTypeface::load_from_dir(&missing)
            .expect_err("a missing directory must not succeed");
        match error {
            TypefaceError::FaceUnavailable { weight, path, .. } => {
                assert_eq!(weight, FontWeight::Regular);
                assert!(path.ends_with("AzeretMono-Regular.ttf"));
            }
            other => panic!("expected FaceUnavailable, got {other:?}"),
        }
    }

    #[test]
    fn an_unavailable_face_names_the_family_in_its_message() {
        let missing = Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor/no-such-typeface");
        let message = AuthoredTypeface::load_from_dir(&missing)
            .expect_err("a missing directory must not succeed")
            .to_string();
        assert!(message.contains(AUTHORED_FAMILY), "{message}");
        assert!(message.contains("Regular"), "{message}");
    }

    #[test]
    fn an_empty_face_is_unreadable_rather_than_silently_accepted() {
        let dir = std::env::temp_dir().join("crest-typeface-empty-face");
        std::fs::create_dir_all(&dir).expect("temp dir");
        for weight in ALL_WEIGHTS {
            std::fs::write(dir.join(face_file_name(weight)), b"").expect("write empty face");
        }
        let error =
            AuthoredTypeface::load_from_dir(&dir).expect_err("an empty face must not succeed");
        assert!(
            matches!(error, TypefaceError::FaceUnreadable { .. }),
            "{error:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
