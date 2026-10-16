//! A read-only, asynchronous local file chooser. Choosing a path does not open,
//! create, replace or remove any document; the application owns that operation.
use serde::Serialize;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};

const ENTRY_LIMIT: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Mode {
    Open,
    Save,
    NewFolder,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct Entry {
    name: String,
    path: String,
    directory: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct View {
    directory: String,
    parent: Option<String>,
    filename: String,
    entries: Vec<Entry>,
    loading: bool,
    error: Option<String>,
    overwrite: Option<String>,
}

enum Reply {
    Directory {
        directory: String,
        parent: Option<String>,
        filename: Option<String>,
        entries: Vec<Entry>,
        warning: Option<String>,
    },
    Overwrite(PathBuf),
    Chosen(PathBuf),
}

/// One chooser token owns one operation at a time. Dropping the chooser cancels
/// delivery of its read-only worker result; no worker can mutate app state.
pub(super) struct FileDialog {
    token: u64,
    version: u64,
    title: String,
    mode: Mode,
    extensions: Vec<String>,
    view: View,
    pending: Option<Receiver<Result<Reply, String>>>,
    completed: bool,
    configuration_error: Option<String>,
}

impl FileDialog {
    pub fn new(
        token: u64,
        title: String,
        mode: Mode,
        initial: PathBuf,
        extensions: Vec<String>,
    ) -> Self {
        let normalized = normalize_extensions(extensions);
        let mut dialog = Self {
            token,
            version: 1,
            title,
            mode,
            extensions: normalized.as_ref().cloned().unwrap_or_default(),
            view: View::default(),
            pending: None,
            completed: false,
            configuration_error: None,
        };
        match normalized {
            Ok(extensions) => dialog.start(move || {
                let (directory, filename) = initial_location(initial)?;
                list_directory(&directory, Some(filename), &extensions)
            }),
            Err(error) => {
                dialog.view.error = Some(error.clone());
                dialog.configuration_error = Some(error);
            }
        }
        dialog
    }

    /// Relative directories resolve against the last successfully listed
    /// canonical directory. Failure preserves that directory and its entries.
    pub fn navigate(&mut self, token: u64, directory: String) -> Result<(), String> {
        self.admit(token)?;
        let base = PathBuf::from(&self.view.directory);
        let extensions = self.extensions.clone();
        let clear_filename = self.mode == Mode::Open;
        self.start(move || {
            let path = resolve_input(&base, &directory)?;
            list_directory(&path, clear_filename.then(String::new), &extensions)
        });
        Ok(())
    }

    /// Save appends the first allowed extension when none was supplied. An
    /// existing file requires a second call with the exact projected overwrite
    /// path and `overwrite = true`; aliases cannot confirm another target.
    pub fn choose(&mut self, token: u64, path: String, overwrite: bool) -> Result<(), String> {
        self.admit(token)?;
        let expected = if overwrite {
            if self.mode != Mode::Save || self.view.overwrite.as_deref() != Some(path.as_str()) {
                return Err("Overwrite confirmation must use the exact prompted path".into());
            }
            Some(PathBuf::from(&path))
        } else {
            None
        };
        let base = PathBuf::from(&self.view.directory);
        let mode = self.mode;
        let extensions = self.extensions.clone();
        self.start(move || {
            let path = resolve_input(&base, &path)?;
            choose_path(path, mode, &extensions, expected.as_deref())
        });
        Ok(())
    }

    /// Applies at most one completed worker response, without blocking. A
    /// validated choice is delivered once; idle polls do not change version.
    pub fn poll(&mut self) -> Option<PathBuf> {
        let result = match self.pending.as_ref()?.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => {
                Err("File browser worker ended before returning a result; try again".into())
            }
        };
        self.pending = None;
        let previous = self.view.clone();
        self.view.loading = false;
        let chosen = match result {
            Ok(reply) => self.apply(reply),
            Err(error) => {
                self.view.error = Some(error);
                None
            }
        };
        self.changed_since(previous);
        chosen
    }

    pub fn projection(&self) -> Value {
        serde_json::json!({
            "token": self.token,
            "version": self.version,
            "title": self.title,
            "mode": self.mode,
            "directory": self.view.directory,
            "parent": self.view.parent,
            "filename": self.view.filename,
            "entries": self.view.entries,
            "loading": self.view.loading,
            "error": self.view.error,
            "overwrite": self.view.overwrite,
        })
    }

    fn admit(&self, token: u64) -> Result<(), String> {
        if token != self.token {
            return Err("This file browser request is stale; use the current dialog".into());
        }
        if self.completed {
            return Err("This file selection has already completed".into());
        }
        if let Some(error) = &self.configuration_error {
            return Err(error.clone());
        }
        if self.pending.is_some() {
            return Err("File browser is still loading; wait for the current operation".into());
        }
        Ok(())
    }

    fn start(&mut self, work: impl FnOnce() -> Result<Reply, String> + Send + 'static) {
        let previous = self.view.clone();
        self.view.error = None;
        self.view.overwrite = None;
        let (send, receive) = mpsc::sync_channel(1);
        match std::thread::Builder::new()
            .name("crest-file-dialog".into())
            .spawn(move || {
                let _ = send.send(work());
            }) {
            Ok(_) => {
                self.pending = Some(receive);
                self.view.loading = true;
            }
            Err(error) => {
                self.view.loading = false;
                self.view.error = Some(format!("Cannot start file browser: {error}"));
            }
        }
        self.changed_since(previous);
    }

    fn apply(&mut self, reply: Reply) -> Option<PathBuf> {
        match reply {
            Reply::Directory {
                directory,
                parent,
                filename,
                entries,
                warning,
            } => {
                self.view.directory = directory;
                self.view.parent = parent;
                if let Some(filename) = filename {
                    self.view.filename = filename;
                }
                self.view.entries = entries;
                self.view.error = warning;
                None
            }
            Reply::Overwrite(path) => {
                self.view.filename = filename(&path).expect("worker validated UTF-8 filename");
                self.view.overwrite =
                    Some(display_path(&path).expect("worker validated UTF-8 path"));
                None
            }
            Reply::Chosen(path) => {
                self.view.filename = filename(&path).expect("worker validated UTF-8 filename");
                self.completed = true;
                Some(path)
            }
        }
    }

    fn changed_since(&mut self, previous: View) {
        if previous != self.view {
            self.version = self.version.saturating_add(1);
        }
    }
}

fn normalize_extensions(extensions: Vec<String>) -> Result<Vec<String>, String> {
    let mut result = Vec::new();
    for extension in extensions {
        let extension = extension.trim_start_matches('.').to_ascii_lowercase();
        if extension.is_empty()
            || extension
                .chars()
                .any(|c| c == '.' || c == '/' || c == '\\' || c.is_control())
        {
            return Err("File extensions must be nonempty suffixes without path separators".into());
        }
        if !result.contains(&extension) {
            result.push(extension);
        }
    }
    Ok(result)
}

fn display_path(path: &Path) -> Result<String, String> {
    path.to_str().map(str::to_owned).ok_or_else(|| {
        "This path contains non-UTF-8 characters; choose a folder with a UTF-8 path".into()
    })
}

fn filename(path: &Path) -> Result<String, String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .ok_or_else(|| "Enter a nonempty UTF-8 filename or folder name".into())
}

fn resolve_input(base: &Path, input: &str) -> Result<PathBuf, String> {
    if input.is_empty() || input.contains('\0') {
        return Err("Enter a nonempty path without NUL characters".into());
    }
    let path = PathBuf::from(input);
    if path.is_absolute() {
        return Ok(path);
    }
    if base.as_os_str().is_empty() {
        return Err("Enter a full absolute path until a directory has loaded".into());
    }
    Ok(base.join(path))
}

fn canonical_directory(path: &Path) -> Result<PathBuf, String> {
    let canonical = fs::canonicalize(path)
        .map_err(|error| format!("Cannot open directory {}: {error}", path.display()))?;
    if !canonical.is_dir() {
        return Err(format!("{} is not a directory", path.display()));
    }
    display_path(&canonical)?;
    Ok(canonical)
}

fn initial_location(initial: PathBuf) -> Result<(PathBuf, String), String> {
    let initial = if initial.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        initial
    };
    if fs::metadata(&initial).is_ok_and(|metadata| metadata.is_dir()) {
        return Ok((canonical_directory(&initial)?, String::new()));
    }
    let parent = initial
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    Ok((canonical_directory(parent)?, filename(&initial)?))
}

fn allowed_extension(path: &Path, extensions: &[String]) -> bool {
    extensions.is_empty()
        || path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| {
                extensions
                    .iter()
                    .any(|extension| value.eq_ignore_ascii_case(extension))
            })
}

fn list_entry(entry: fs::DirEntry, extensions: &[String]) -> Result<Option<Entry>, &'static str> {
    let path = entry.path();
    let name = filename(&path).map_err(|_| "non-UTF-8")?;
    let metadata = fs::metadata(&path).map_err(|_| "unreadable")?;
    let directory = metadata.is_dir();
    if !directory && (!metadata.is_file() || !allowed_extension(&path, extensions)) {
        return Ok(None);
    }
    Ok(Some(Entry {
        name,
        path: display_path(&path).map_err(|_| "non-UTF-8")?,
        directory,
    }))
}

fn list_directory(
    path: &Path,
    filename: Option<String>,
    extensions: &[String],
) -> Result<Reply, String> {
    let directory = canonical_directory(path)?;
    let listing = fs::read_dir(&directory).map_err(|error| {
        format!(
            "Cannot list {}: {error}. Enter a full path or another directory.",
            directory.display()
        )
    })?;
    let mut entries = Vec::new();
    let mut skipped_names = 0;
    let mut unreadable = 0;
    let mut limited = false;
    for (index, entry) in listing.enumerate() {
        if index == ENTRY_LIMIT {
            limited = true;
            break;
        }
        match entry
            .map_err(|_| "unreadable")
            .and_then(|entry| list_entry(entry, extensions))
        {
            Ok(Some(entry)) => entries.push(entry),
            Ok(None) => {}
            Err("non-UTF-8") => skipped_names += 1,
            Err(_) => unreadable += 1,
        }
    }
    entries.sort_by(|left, right| {
        right
            .directory
            .cmp(&left.directory)
            .then_with(|| left.name.cmp(&right.name))
    });
    let mut warnings = Vec::new();
    if limited {
        warnings.push("Directory exceeds 1024 entries; listing is incomplete. Enter a full path or open a smaller folder.".into());
    }
    if skipped_names > 0 {
        warnings.push(format!("Skipped {skipped_names} non-UTF-8 names. Rename those entries outside this browser to select them."));
    }
    if unreadable > 0 {
        warnings.push(format!(
            "Skipped {unreadable} unreadable entries. Check permissions or enter a full path."
        ));
    }
    Ok(Reply::Directory {
        directory: display_path(&directory)?,
        parent: directory.parent().map(display_path).transpose()?,
        filename,
        entries,
        warning: (!warnings.is_empty()).then(|| warnings.join(" ")),
    })
}

fn require_extension(path: &Path, extensions: &[String]) -> Result<(), String> {
    if allowed_extension(path, extensions) {
        return Ok(());
    }
    Err(format!(
        "Choose a file with one of these extensions: {}",
        extensions.join(", ")
    ))
}

fn canonical_new_path(path: &Path) -> Result<PathBuf, String> {
    let name = filename(path)?;
    let parent = path
        .parent()
        .ok_or("Choose a path inside an existing directory")?;
    let canonical = canonical_directory(parent)?.join(name);
    display_path(&canonical)?;
    Ok(canonical)
}

fn choose_path(
    mut path: PathBuf,
    mode: Mode,
    extensions: &[String],
    overwrite: Option<&Path>,
) -> Result<Reply, String> {
    if mode == Mode::Save && path.extension().is_none() {
        if let Some(extension) = extensions.first() {
            path.set_extension(extension);
        }
    }
    if mode != Mode::NewFolder {
        require_extension(&path, extensions)?;
    }
    let canonical = canonical_new_path(&path)?;
    if overwrite.is_some_and(|expected| expected != canonical) {
        return Err(
            "Overwrite target changed; choose the file again to review its exact path".into(),
        );
    }
    let exists = match fs::symlink_metadata(&canonical) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(format!("Cannot inspect {}: {error}", canonical.display())),
    };
    if mode == Mode::NewFolder {
        return if exists {
            Err("Collect requires a new, absent folder; choose another name".into())
        } else {
            Ok(Reply::Chosen(canonical))
        };
    }
    if !exists {
        return if mode == Mode::Open {
            Err(format!("File does not exist: {}", canonical.display()))
        } else {
            Ok(Reply::Chosen(canonical))
        };
    }
    let target = fs::canonicalize(&canonical)
        .map_err(|error| format!("Cannot resolve {}: {error}", canonical.display()))?;
    display_path(&target)?;
    filename(&target)?;
    if !fs::metadata(&target)
        .map_err(|error| error.to_string())?
        .is_file()
    {
        return Err(
            "Choose a regular file; directories and special files cannot be opened or replaced"
                .into(),
        );
    }
    require_extension(&target, extensions)?;
    if mode == Mode::Open {
        return Ok(Reply::Chosen(target));
    }
    match overwrite {
        Some(expected) if expected == target => Ok(Reply::Chosen(target)),
        Some(_) => {
            Err("Overwrite target changed; choose the file again to review its exact path".into())
        }
        None => Ok(Reply::Overwrite(target)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("crest-file-dialog-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&path).unwrap();
            Self(path.canonicalize().unwrap())
        }
        fn dialog(&self, mode: Mode) -> FileDialog {
            FileDialog::new(
                73,
                "Choose document".into(),
                mode,
                self.0.clone(),
                vec![".JSON".into()],
            )
        }
        fn file(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            fs::write(&path, b"original document").unwrap();
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn settle(dialog: &mut FileDialog) -> Option<PathBuf> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let chosen = dialog.poll();
            if !dialog.projection()["loading"].as_bool().unwrap() {
                return chosen;
            }
            assert!(Instant::now() < deadline, "file browser worker timed out");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    fn names(dialog: &FileDialog) -> Vec<String> {
        dialog.projection()["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["name"].as_str().unwrap().into())
            .collect()
    }

    #[test]
    fn projection_is_exact_initial_file_lists_parent_and_filters_case_insensitively() {
        let fixture = Fixture::new();
        fixture.file("keep.JSON");
        fixture.file("omit.txt");
        fs::create_dir(fixture.0.join("folder.txt")).unwrap();
        let initial = fixture.0.join("new.json");
        let mut dialog = FileDialog::new(
            73,
            "Save song".into(),
            Mode::Save,
            initial,
            vec!["json".into()],
        );
        assert!(dialog.projection()["loading"].as_bool().unwrap());
        assert_eq!(settle(&mut dialog), None);
        let view = dialog.projection();
        let mut keys: Vec<_> = view
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "directory",
                "entries",
                "error",
                "filename",
                "loading",
                "mode",
                "overwrite",
                "parent",
                "title",
                "token",
                "version"
            ]
        );
        assert_eq!(view["directory"], fixture.0.to_str().unwrap());
        assert_eq!(
            view["parent"],
            fixture.0.parent().unwrap().to_str().unwrap()
        );
        assert_eq!(view["filename"], "new.json");
        assert_eq!(view["mode"], "save");
        assert_eq!(names(&dialog), ["folder.txt", "keep.JSON"]);
        assert_eq!(view["entries"][0]["directory"], true);
        let before = dialog.projection();
        assert_eq!(dialog.poll(), None);
        assert_eq!(dialog.projection(), before);
    }

    #[test]
    fn one_worker_stale_tokens_and_failed_navigation_preserve_loaded_directory() {
        let fixture = Fixture::new();
        let mut dialog = fixture.dialog(Mode::Open);
        let loading = dialog.projection();
        assert!(dialog
            .navigate(72, fixture.0.to_str().unwrap().into())
            .unwrap_err()
            .contains("stale"));
        assert!(dialog
            .choose(73, "song.json".into(), false)
            .unwrap_err()
            .contains("loading"));
        assert_eq!(dialog.projection(), loading);
        settle(&mut dialog);
        let previous = dialog.projection();
        dialog.navigate(73, "absent-folder".into()).unwrap();
        settle(&mut dialog);
        let failed = dialog.projection();
        assert_eq!(failed["directory"], previous["directory"]);
        assert_eq!(failed["entries"], previous["entries"]);
        assert!(failed["error"]
            .as_str()
            .unwrap()
            .contains("Cannot open directory"));
        assert!(failed["version"].as_u64().unwrap() > previous["version"].as_u64().unwrap());
        let idle = dialog.projection();
        assert!(dialog.choose(1, "song.json".into(), false).is_err());
        assert_eq!(dialog.projection(), idle);
    }

    #[test]
    fn open_requires_existing_regular_file_with_allowed_extension() {
        let fixture = Fixture::new();
        let original = fixture.file("keep.JSON");
        fixture.file("wrong.txt");
        fs::create_dir(fixture.0.join("folder.json")).unwrap();
        let mut dialog = fixture.dialog(Mode::Open);
        settle(&mut dialog);
        for path in ["missing.json", "folder.json", "wrong.txt"] {
            dialog.choose(73, path.into(), false).unwrap();
            assert_eq!(settle(&mut dialog), None);
            assert!(dialog.projection()["error"].is_string());
        }
        dialog.choose(73, "keep.JSON".into(), false).unwrap();
        assert_eq!(settle(&mut dialog), Some(original.clone()));
        assert_eq!(fs::read(original).unwrap(), b"original document");
        assert!(dialog
            .choose(73, "keep.JSON".into(), false)
            .unwrap_err()
            .contains("completed"));
        assert_eq!(dialog.poll(), None);
    }

    #[test]
    fn save_appends_extension_and_requires_exact_existing_path_confirmation() {
        let fixture = Fixture::new();
        let original = fixture.file("existing.json");
        let mut dialog = fixture.dialog(Mode::Save);
        settle(&mut dialog);
        assert!(dialog.choose(73, "existing.json".into(), true).is_err());
        dialog.choose(73, "existing".into(), false).unwrap();
        assert_eq!(settle(&mut dialog), None);
        let prompt = dialog.projection()["overwrite"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(prompt, original.to_str().unwrap());
        let before = dialog.projection();
        assert!(dialog.choose(73, "existing.json".into(), true).is_err());
        assert!(dialog
            .choose(
                73,
                fixture.0.join("other.json").to_str().unwrap().into(),
                true
            )
            .is_err());
        assert_eq!(dialog.projection(), before);
        dialog.choose(73, prompt, true).unwrap();
        assert_eq!(settle(&mut dialog), Some(original.clone()));
        assert_eq!(fs::read(original).unwrap(), b"original document");
        let mut absent = fixture.dialog(Mode::Save);
        settle(&mut absent);
        absent.choose(73, "new-name".into(), false).unwrap();
        let new_path = fixture.0.join("new-name.json");
        assert_eq!(settle(&mut absent), Some(new_path.clone()));
        assert!(!new_path.exists());
    }

    #[test]
    fn browsing_another_directory_clears_overwrite_and_preserves_save_filename() {
        let fixture = Fixture::new();
        fixture.file("existing.json");
        fs::create_dir(fixture.0.join("elsewhere")).unwrap();
        let mut dialog = fixture.dialog(Mode::Save);
        settle(&mut dialog);
        dialog.choose(73, "existing.json".into(), false).unwrap();
        settle(&mut dialog);
        let prompt = dialog.projection()["overwrite"]
            .as_str()
            .unwrap()
            .to_owned();
        dialog.navigate(73, "elsewhere".into()).unwrap();
        assert!(dialog.projection()["overwrite"].is_null());
        settle(&mut dialog);
        assert_eq!(dialog.projection()["filename"], "existing.json");
        assert!(dialog.choose(73, prompt, true).is_err());
        dialog.choose(73, "existing.json".into(), false).unwrap();
        assert_eq!(
            settle(&mut dialog),
            Some(fixture.0.join("elsewhere/existing.json"))
        );
        assert!(!fixture.0.join("elsewhere/existing.json").exists());
    }

    #[test]
    fn new_folder_rejects_every_existing_entry_and_never_creates_the_choice() {
        let fixture = Fixture::new();
        fixture.file("file");
        fs::create_dir(fixture.0.join("folder")).unwrap();
        let mut dialog = fixture.dialog(Mode::NewFolder);
        settle(&mut dialog);
        assert_eq!(dialog.projection()["mode"], "new_folder");
        for name in ["file", "folder"] {
            dialog.choose(73, name.into(), false).unwrap();
            assert_eq!(settle(&mut dialog), None);
            assert!(dialog.projection()["error"]
                .as_str()
                .unwrap()
                .contains("new, absent folder"));
            assert!(dialog.projection()["overwrite"].is_null());
        }
        assert!(dialog.choose(73, "file".into(), true).is_err());
        dialog.choose(73, "collected-song".into(), false).unwrap();
        assert_eq!(settle(&mut dialog), Some(fixture.0.join("collected-song")));
        assert!(!fixture.0.join("collected-song").exists());
    }

    #[test]
    fn bounded_listing_still_accepts_an_unlisted_full_path() {
        let fixture = Fixture::new();
        for index in 0..ENTRY_LIMIT + 3 {
            fixture.file(&format!("file-{index}.json"));
        }
        let mut dialog = fixture.dialog(Mode::Open);
        settle(&mut dialog);
        let view = dialog.projection();
        assert!(view["entries"].as_array().unwrap().len() <= ENTRY_LIMIT);
        let error = view["error"].as_str().unwrap();
        assert!(error.contains("1024"));
        assert!(error.contains("full path"));
        let visible: std::collections::BTreeSet<_> = names(&dialog).into_iter().collect();
        let missing = (0..ENTRY_LIMIT + 3)
            .map(|index| format!("file-{index}.json"))
            .find(|name| !visible.contains(name))
            .unwrap();
        let full_path = fixture.0.join(missing);
        dialog
            .choose(73, full_path.to_str().unwrap().into(), false)
            .unwrap();
        assert_eq!(settle(&mut dialog), Some(full_path));
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_return_canonical_targets_and_dangling_links_are_not_new_folders() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let original = fixture.file("original.json");
        symlink(&original, fixture.0.join("alias.json")).unwrap();
        symlink(fixture.0.join("missing"), fixture.0.join("dangling")).unwrap();
        fs::create_dir(fixture.0.join("real-folder")).unwrap();
        symlink(
            fixture.0.join("real-folder"),
            fixture.0.join("folder-alias"),
        )
        .unwrap();
        let mut dialog = fixture.dialog(Mode::Open);
        settle(&mut dialog);
        dialog.choose(73, "alias.json".into(), false).unwrap();
        assert_eq!(settle(&mut dialog), Some(original.clone()));
        let mut save = fixture.dialog(Mode::Save);
        settle(&mut save);
        save.choose(73, "alias.json".into(), false).unwrap();
        settle(&mut save);
        assert_eq!(save.projection()["overwrite"], original.to_str().unwrap());
        assert!(save
            .choose(
                73,
                fixture.0.join("alias.json").to_str().unwrap().into(),
                true
            )
            .is_err());
        save.choose(73, original.to_str().unwrap().into(), true)
            .unwrap();
        assert_eq!(settle(&mut save), Some(original));
        let mut folder = fixture.dialog(Mode::NewFolder);
        settle(&mut folder);
        folder.choose(73, "dangling".into(), false).unwrap();
        assert_eq!(settle(&mut folder), None);
        folder.navigate(73, "folder-alias".into()).unwrap();
        settle(&mut folder);
        assert_eq!(
            folder.projection()["directory"],
            fixture.0.join("real-folder").to_str().unwrap()
        );
        folder.choose(73, "new".into(), false).unwrap();
        assert_eq!(settle(&mut folder), Some(fixture.0.join("real-folder/new")));
    }

    #[cfg(unix)]
    #[test]
    fn replacing_prompted_file_with_symlink_cannot_redirect_overwrite() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let original = fixture.file("original.json");
        let other = fixture.file("other.json");
        let mut dialog = fixture.dialog(Mode::Save);
        settle(&mut dialog);
        dialog.choose(73, "original.json".into(), false).unwrap();
        settle(&mut dialog);
        let prompt = dialog.projection()["overwrite"]
            .as_str()
            .unwrap()
            .to_owned();
        fs::remove_file(&original).unwrap();
        symlink(&other, &original).unwrap();
        dialog.choose(73, prompt, true).unwrap();
        assert_eq!(settle(&mut dialog), None);
        assert!(dialog.projection()["error"]
            .as_str()
            .unwrap()
            .contains("target changed"));
        assert_eq!(fs::read(other).unwrap(), b"original document");
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_paths_fail_conversion_without_lossy_projection() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;
        let path = PathBuf::from(OsString::from_vec(b"/tmp/bad-\xff.json".to_vec()));
        assert!(filename(&path).is_err());
        assert!(display_path(&path).unwrap_err().contains("non-UTF-8"));
    }

    #[cfg(unix)]
    #[test]
    #[cfg_attr(
        target_os = "macos",
        ignore = "APFS rejects non-UTF-8 filename creation; filesystem witness requires a supporting Unix filesystem"
    )]
    fn non_utf8_names_are_skipped_with_visible_warning() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;
        let fixture = Fixture::new();
        fs::write(
            fixture
                .0
                .join(OsString::from_vec(b"bad-\xff.json".to_vec())),
            b"keep",
        )
        .unwrap();
        fixture.file("good.json");
        let mut dialog = fixture.dialog(Mode::Open);
        settle(&mut dialog);
        assert_eq!(names(&dialog), ["good.json"]);
        assert!(dialog.projection()["error"]
            .as_str()
            .unwrap()
            .contains("non-UTF-8"));
    }

    #[test]
    fn dropping_pending_dialog_discards_result_without_mutating_files() {
        let fixture = Fixture::new();
        let original = fixture.file("original.json");
        let mut dialog = fixture.dialog(Mode::Open);
        settle(&mut dialog);
        let (release, blocked) = mpsc::sync_channel(1);
        let (finished, done) = mpsc::sync_channel(1);
        let returned_path = original.clone();
        dialog.start(move || {
            blocked.recv().unwrap();
            let result = choose_path(returned_path, Mode::Open, &["json".into()], None);
            finished.send(()).unwrap();
            result
        });
        drop(dialog);
        release.send(()).unwrap();
        done.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(fs::read(original).unwrap(), b"original document");
        assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 1);
        let mut replacement = fixture.dialog(Mode::Open);
        settle(&mut replacement);
        assert_eq!(replacement.poll(), None);
    }
}
