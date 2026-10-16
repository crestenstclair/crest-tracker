//! Content-addressed Custom chords, independent of Song-local revision IDs.
use super::*;
use crest_tracker::chord_bank::ChordRevision;
use std::io::Read;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishedChord {
    pub digest: String,
    pub chord: ChordRevision,
}

impl PublishedChord {
    fn validate(&self) -> Result<(), String> {
        self.chord.validate().map_err(|error| error.to_string())?;
        if self.digest
            != digest(&serde_json::to_value(&self.chord).map_err(|error| error.to_string())?)?
        {
            return Err("Chord library digest mismatch".into());
        }
        Ok(())
    }
}

pub fn publish(root: &Path, chord: ChordRevision) -> Result<PublishedChord, String> {
    let entry = PublishedChord {
        digest: digest(&serde_json::to_value(&chord).map_err(|error| error.to_string())?)?,
        chord,
    };
    entry.validate()?;
    fs::create_dir_all(root).map_err(|error| error.to_string())?;
    let path = root.join(format!("{}.crest-chord", entry.digest));
    let temporary = root.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| error.to_string())?;
        let bytes = serde_json::to_vec_pretty(&entry).map_err(|error| error.to_string())?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|error| error.to_string())?;
        drop(file);
        match fs::hard_link(&temporary, &path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if read(&path)?.chord != entry.chord {
                    return Err("Existing chord revision differs".into());
                }
            }
            Err(error) => return Err(error.to_string()),
        }
        fs::File::open(root)
            .and_then(|file| file.sync_all())
            .map_err(|error| error.to_string())?;
        Ok(entry)
    })();
    let _ = fs::remove_file(temporary);
    result
}

fn read(path: &Path) -> Result<PublishedChord, String> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|error| error.to_string())?
        .take(65_537)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > 65_536 {
        return Err("Chord revision exceeds admission budget".into());
    }
    let entry: PublishedChord =
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    entry.validate()?;
    if path.file_stem().and_then(|stem| stem.to_str()) != Some(entry.digest.as_str()) {
        return Err("Chord filename does not match its revision".into());
    }
    Ok(entry)
}

pub fn list(root: &Path) -> Result<Vec<PublishedChord>, String> {
    let directory = match fs::read_dir(root) {
        Ok(directory) => directory,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let mut entries = Vec::new();
    for entry in directory {
        let entry = entry.map_err(|error| error.to_string())?;
        if entry
            .path()
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("crest-chord")
        {
            continue;
        }
        entries.push(read(&entry.path())?);
    }
    entries.sort_by(|a, b| {
        a.chord
            .name()
            .cmp(b.chord.name())
            .then(a.digest.cmp(&b.digest))
    });
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crest_tracker::{
        chord_bank::ChordDefinition,
        harmony::{Chord, Pitch},
    };
    #[test]
    fn shared_library_is_immutable_validated_and_independent_of_song_ids() {
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let chord = ChordRevision::new(
            "Minor".into(),
            ChordDefinition::new(Chord::new(vec![0, 3, 7]).unwrap(), 1, true, 1).unwrap(),
            Pitch::new(60).unwrap(),
        )
        .unwrap();
        let first = publish(&root, chord.clone()).unwrap();
        assert_eq!(publish(&root, chord.clone()).unwrap().digest, first.digest);
        assert_eq!(list(&root).unwrap()[0].chord, chord);
        let path = root.join(format!("{}.crest-chord", first.digest));
        let mut corrupt = first;
        corrupt.digest = "bad".into();
        fs::write(&path, serde_json::to_vec(&corrupt).unwrap()).unwrap();
        assert!(list(&root).is_err());
        assert!(publish(&root, chord).is_err());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }
}
