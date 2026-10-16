use std::path::{Component, Path, PathBuf};

/// Resolves relative built-in assets without changing persisted identities or cwd.
/// Once inside an app bundle, missing resources remain errors in that bundle.
pub(crate) fn path(relative: &str) -> std::io::Result<PathBuf> {
    path_for_executable(&std::env::current_exe()?, relative)
}

fn path_for_executable(executable: &Path, relative: &str) -> std::io::Result<PathBuf> {
    let relative = Path::new(relative);
    // This adapter resolves built-in identities only. Absolute paths or parent
    // traversal would escape the selected bundle or dependency source root.
    if !relative
        .components()
        .any(|component| matches!(component, Component::Normal(_)))
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "built-in resource path must be relative without parent traversal",
        ));
    }
    if let Some(macos) = executable.parent() {
        if macos.file_name().is_some_and(|name| name == "MacOS") {
            if let Some(contents) = macos.parent() {
                if contents.file_name().is_some_and(|name| name == "Contents")
                    && contents
                        .parent()
                        .and_then(Path::extension)
                        .is_some_and(|extension| extension == "app")
                {
                    return Ok(contents.join("Resources").join(relative));
                }
            }
        }
    }
    Ok(Path::new(env!("CARGO_MANIFEST_DIR")).join(relative))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_assets_resolve_inside_relocated_bundle_even_when_missing() {
        for relative in ["./sf2/HiDef.sf2", "./midi/song.mid"] {
            assert_eq!(
                path_for_executable(
                    Path::new("/Volumes/External Apps/crest-synth.app/Contents/MacOS/crest-synth"),
                    relative,
                )
                .unwrap(),
                Path::new("/Volumes/External Apps/crest-synth.app/Contents/Resources")
                    .join(relative),
            );
        }
    }

    #[test]
    fn unbundled_assets_resolve_from_synth_source_root_independent_of_host_location() {
        for executable in [
            "/checkout/target/release/crest-synth",
            "/another/crest-tracker/target/debug/crest-tracker",
            "/tmp/Contents/MacOS/crest-synth",
        ] {
            assert_eq!(
                path_for_executable(Path::new(executable), "./sf2/HiDef.sf2").unwrap(),
                Path::new(env!("CARGO_MANIFEST_DIR")).join("./sf2/HiDef.sf2"),
            );
        }
    }

    #[test]
    fn built_in_resource_paths_cannot_escape_the_selected_root() {
        for executable in [
            "/another/crest-tracker/target/debug/crest-tracker",
            "/Applications/crest-tracker.app/Contents/MacOS/crest-tracker",
        ] {
            assert_rejects_invalid_built_in_paths(Path::new(executable));
        }
    }

    fn assert_rejects_invalid_built_in_paths(executable: &Path) {
        for invalid in [
            "",
            ".",
            "./",
            "/tmp/sound.sf2",
            "../sound.sf2",
            "sf2/../sound.sf2",
        ] {
            assert_eq!(
                path_for_executable(executable, invalid).unwrap_err().kind(),
                std::io::ErrorKind::InvalidInput,
                "unexpected result for {invalid:?}",
            );
        }
    }
}
