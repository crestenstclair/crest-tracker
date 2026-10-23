//! Open any imported Song through the production file dialog and play to its end.
use super::*;
use crate::persistence::Document;
use crest_tracker::composition::Fx;
use std::path::{Path, PathBuf};

pub(super) enum Checkpoint {
    Browser,
    Loaded {
        path: PathBuf,
        performance: String,
    },
    Finished {
        last_attack: Beat,
        expected_tracks: [bool; 16],
        timeout_seconds: u64,
    },
}

impl Scene {
    pub fn song(path: &Path) -> Result<Self, String> {
        let path = path.canonicalize().map_err(|e| e.to_string())?;
        let document = Document::read(&path)?;
        let mut end = Beat::ZERO;
        let mut last_attack = Beat::ZERO;
        let mut notes = 0_usize;
        let mut expected_tracks = [false; 16];
        let mut slowest = document.song.tempo.bpm();
        // Derive shared phrase and chain placement
        // extents arithmetically so a large repeat count cannot stall analysis.
        for track in &document.song.tracks {
            if track.looped {
                return Err("Song demo requires finite, non-looping tracks".into());
            }
            if track.muted {
                continue;
            }
            let mut at = Beat::ZERO;
            for usage in track.slots.iter().flatten() {
                if usage.chain.special().is_some() {
                    at = document
                        .song
                        .after_measures(at, usage.repeats)
                        .map_err(|e| e.to_string())?;
                    continue;
                }
                let chain =
                    &document.song.chains[&usage.chain.pattern().map_err(|e| e.to_string())?];
                let mut chain_length = Beat::ZERO;
                let mut chain_attack = None;
                let mut chain_notes = 0_usize;
                for phrase_use in &chain.phrases {
                    let phrase = &document.song.phrases[&phrase_use.phrase];
                    let length = phrase.length().map_err(|e| e.to_string())?;
                    let last_repeat = length
                        .checked_scale(u64::from(phrase_use.repeats - 1), 1)
                        .map_err(|e| e.to_string())?;
                    for event in &phrase.events {
                        if event.note.is_some() {
                            chain_notes = chain_notes
                                .checked_add(phrase_use.repeats as usize)
                                .ok_or("Note count overflow")?;
                            let attack = chain_length
                                .checked_add(last_repeat)
                                .and_then(|at| at.checked_add(event.at))
                                .map_err(|e| e.to_string())?;
                            chain_attack = Some(
                                chain_attack.map_or(attack, |previous: Beat| previous.max(attack)),
                            );
                            // Import patches route source channels to matching mixer tracks.
                            expected_tracks[usize::from(event.target.channel().get() - 1)] = true;
                        }
                        for fx in event.fx.iter().flatten() {
                            match fx {
                                Fx::Tempo { tempo } => slowest = slowest.min(tempo.bpm()),
                                Fx::Jump { .. } => {
                                    return Err(
                                        "Song demo requires linear phrases without Jump commands"
                                            .into(),
                                    )
                                }
                                _ => {}
                            }
                        }
                    }
                    chain_length = chain_length
                        .checked_add(
                            length
                                .checked_scale(u64::from(phrase_use.repeats), 1)
                                .map_err(|e| e.to_string())?,
                        )
                        .map_err(|e| e.to_string())?;
                }
                if let Some(attack) = chain_attack {
                    last_attack = last_attack.max(
                        at.checked_add(
                            chain_length
                                .checked_scale(u64::from(usage.repeats - 1), 1)
                                .map_err(|e| e.to_string())?,
                        )
                        .and_then(|at| at.checked_add(attack))
                        .map_err(|e| e.to_string())?,
                    );
                }
                notes = notes
                    .checked_add(
                        chain_notes
                            .checked_mul(usage.repeats as usize)
                            .ok_or("Note count overflow")?,
                    )
                    .ok_or("Note count overflow")?;
                at = at
                    .checked_add(
                        chain_length
                            .checked_scale(u64::from(usage.repeats), 1)
                            .map_err(|e| e.to_string())?,
                    )
                    .map_err(|e| e.to_string())?;
            }
            end = end.max(at);
        }
        if notes == 0 {
            return Err("Song demo requires audible note events".into());
        }
        let seconds =
            ((end.numerator() as f64 / end.denominator() as f64) * 60.0 / slowest).ceil() as u64;
        use Check::*;
        use Event::*;
        let mut scene = Self::script(
            "imported-song",
            vec![
                ("Application ready", Wait, Ready),
                ("Open main menu", Activate("menu"), Panel("files")),
                ("Open Song File", Activate("files"), Panel("open")),
                (
                    "Open file browser",
                    Activate("open"),
                    Song(Checkpoint::Browser),
                ),
                ("Enter absolute Song path", FileName, Panel("text_submit")),
                (
                    "Load validated composite Song",
                    FilePath(path.clone()),
                    Song(Checkpoint::Loaded {
                        path,
                        performance: crate::midi_plan::performance_digest(&document.song)?,
                    }),
                ),
                (
                    "Play imported performance",
                    Action(SemanticAction::PreviewStart),
                    FullPlayback,
                ),
                (
                    "Complete Song and release voices",
                    Wait,
                    Song(Checkpoint::Finished {
                        last_attack,
                        expected_tracks,
                        timeout_seconds: seconds.saturating_add(60),
                    }),
                ),
            ],
        );
        scene.playback_timeout = seconds.saturating_add(60);
        scene.report.notes = notes;
        scene.report.instruments = document.installed_patches.len();
        Ok(scene)
    }
}

pub(super) fn check(
    runtime: &Runtime,
    report: &Report,
    checkpoint: &Checkpoint,
    audio: AudioObservationSnapshot,
) -> Result<bool, String> {
    let app = runtime.app.borrow();
    Ok(match checkpoint {
        Checkpoint::Browser => app.file_dialog["mode"] == "open",
        Checkpoint::Loaded { path, performance } => {
            if runtime.loading.is_some() || runtime.pending_open.is_some() {
                return Ok(false);
            }
            if app.path.as_ref() != Some(path) {
                return Err(format!("Song did not load: {}", app.status));
            }
            if &crate::midi_plan::performance_digest(app.editor.song())? != performance {
                return Err("Loaded Song changed performance data".into());
            }
            !app.dirty()
        }
        Checkpoint::Finished {
            last_attack,
            expected_tracks,
            ..
        } => {
            if app.playing || audio.active_notes() != 0 {
                return Ok(false);
            }
            if report.last_beat.is_none_or(|beat| beat < *last_attack)
                || expected_tracks
                    .iter()
                    .zip(report.audible_tracks)
                    .any(|(expected, audible)| *expected && !audible)
                || report.clipped_samples != 0
                || report.output_peak > 1.0
            {
                return Err(format!(
                    "Incomplete or clipped Song playback: {}",
                    serde_json::to_string(report).map_err(|e| e.to_string())?
                ));
            }
            true
        }
    })
}
