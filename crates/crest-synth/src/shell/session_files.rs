use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionFileFailureKind {
    Read,
    CreateTemporary,
    WriteTemporary,
    FlushTemporary,
    SyncTemporary,
    AtomicReplace,
    SyncParent,
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("session filesystem operation failed during {kind:?}")]
pub struct SessionFileFailure {
    kind: SessionFileFailureKind,
}

impl SessionFileFailure {
    const fn new(kind: SessionFileFailureKind) -> Self {
        Self { kind }
    }

    pub const fn kind(&self) -> SessionFileFailureKind {
        self.kind
    }

    #[cfg(test)]
    pub(crate) const fn new_for_test(kind: SessionFileFailureKind) -> Self {
        Self::new(kind)
    }
}

pub trait SessionFilePort: Send + Sync {
    fn read(&self, path: &Path) -> Result<Vec<u8>, SessionFileFailure>;
    fn write_atomic(&self, path: &Path, bytes: &[u8]) -> Result<(), SessionFileFailure>;
}

/// Standard-library filesystem adapter. Destination files are never opened
/// for truncation: bytes reach a unique same-directory temporary first, then
/// one atomic rename establishes the new file.
#[derive(Clone, Copy, Debug, Default)]
pub struct StandardSessionFileSystem;

impl SessionFilePort for StandardSessionFileSystem {
    fn read(&self, path: &Path) -> Result<Vec<u8>, SessionFileFailure> {
        let mut file =
            File::open(path).map_err(|_| SessionFileFailure::new(SessionFileFailureKind::Read))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|_| SessionFileFailure::new(SessionFileFailureKind::Read))?;
        Ok(bytes)
    }

    fn write_atomic(&self, path: &Path, bytes: &[u8]) -> Result<(), SessionFileFailure> {
        write_atomic(path, bytes, None)
    }
}

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

fn temporary_path(destination: &Path) -> PathBuf {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("session");
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    parent.join(format!(
        ".{name}.crest-tmp-{}-{sequence}",
        std::process::id()
    ))
}

fn write_atomic(
    destination: &Path,
    bytes: &[u8],
    injected_failure: Option<SessionFileFailureKind>,
) -> Result<(), SessionFileFailure> {
    let temporary = temporary_path(destination);
    if injected_failure == Some(SessionFileFailureKind::CreateTemporary) {
        return Err(SessionFileFailure::new(
            SessionFileFailureKind::CreateTemporary,
        ));
    }
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|_| SessionFileFailure::new(SessionFileFailureKind::CreateTemporary))?;
    let result = (|| {
        if injected_failure == Some(SessionFileFailureKind::WriteTemporary) {
            return Err(SessionFileFailure::new(
                SessionFileFailureKind::WriteTemporary,
            ));
        }
        file.write_all(bytes)
            .map_err(|_| SessionFileFailure::new(SessionFileFailureKind::WriteTemporary))?;
        if injected_failure == Some(SessionFileFailureKind::FlushTemporary) {
            return Err(SessionFileFailure::new(
                SessionFileFailureKind::FlushTemporary,
            ));
        }
        file.flush()
            .map_err(|_| SessionFileFailure::new(SessionFileFailureKind::FlushTemporary))?;
        if injected_failure == Some(SessionFileFailureKind::SyncTemporary) {
            return Err(SessionFileFailure::new(
                SessionFileFailureKind::SyncTemporary,
            ));
        }
        file.sync_all()
            .map_err(|_| SessionFileFailure::new(SessionFileFailureKind::SyncTemporary))?;
        drop(file);
        if injected_failure == Some(SessionFileFailureKind::AtomicReplace) {
            return Err(SessionFileFailure::new(
                SessionFileFailureKind::AtomicReplace,
            ));
        }
        fs::rename(&temporary, destination)
            .map_err(|_| SessionFileFailure::new(SessionFileFailureKind::AtomicReplace))?;
        if injected_failure == Some(SessionFileFailureKind::SyncParent) {
            return Err(SessionFileFailure::new(SessionFileFailureKind::SyncParent));
        }
        let parent = destination.parent().unwrap_or_else(|| Path::new("."));
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| SessionFileFailure::new(SessionFileFailureKind::SyncParent))?;
        Ok(())
    })();
    if result.is_err() && temporary.exists() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "crest-session-files-{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        path
    }

    fn temporary_entries(directory: &Path) -> Vec<PathBuf> {
        fs::read_dir(directory)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.contains(".crest-tmp-"))
            })
            .collect()
    }

    #[test]
    fn atomic_write_replaces_only_after_complete_temporary_sync() {
        let directory = directory();
        let destination = directory.join("session.crest");
        fs::write(&destination, b"old").unwrap();
        StandardSessionFileSystem
            .write_atomic(&destination, b"complete-new")
            .unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"complete-new");
        assert!(temporary_entries(&directory).is_empty());
        fs::remove_file(destination).unwrap();
        fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn pre_replace_failures_preserve_destination_and_clean_temporary_files() {
        for kind in [
            SessionFileFailureKind::CreateTemporary,
            SessionFileFailureKind::WriteTemporary,
            SessionFileFailureKind::FlushTemporary,
            SessionFileFailureKind::SyncTemporary,
            SessionFileFailureKind::AtomicReplace,
        ] {
            let directory = directory();
            let destination = directory.join("session.crest");
            fs::write(&destination, b"prior-valid").unwrap();
            assert_eq!(
                write_atomic(&destination, b"candidate", Some(kind)),
                Err(SessionFileFailure::new(kind))
            );
            assert_eq!(fs::read(&destination).unwrap(), b"prior-valid");
            assert!(temporary_entries(&directory).is_empty());
            fs::remove_file(destination).unwrap();
            fs::remove_dir(directory).unwrap();
        }
    }

    #[test]
    fn read_failure_is_explicit_and_never_fabricates_bytes() {
        let directory = directory();
        let missing = directory.join("missing.crest");
        assert_eq!(
            StandardSessionFileSystem.read(&missing),
            Err(SessionFileFailure::new(SessionFileFailureKind::Read))
        );
        fs::remove_dir(directory).unwrap();
    }
}
