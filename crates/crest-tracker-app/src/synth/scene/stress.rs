//! Corpus stress tours use public UI events and verify committed document data.
use super::*;
use crate::persistence::Document;
use crest_tracker::composition::{Event as NoteEvent, PhraseForm, PhraseId};
use std::path::Path;

pub(super) enum Checkpoint {
    Settled,
    Phrase(PhraseId),
    Events(PhraseId, Vec<NoteEvent>),
}

pub(super) fn check(app: &App, checkpoint: &Checkpoint) -> Result<bool, String> {
    if !app.playing {
        return Err(format!("Stress tour stopped playback: {}", app.status));
    }
    if !app.requests.is_empty() {
        return Ok(false);
    }
    match checkpoint {
        Checkpoint::Settled => Ok(true),
        Checkpoint::Phrase(id) => Ok(app
            .selected_phrase()
            .is_some_and(|(selected, _)| selected == *id)),
        Checkpoint::Events(id, expected) => {
            if app.editor.song().phrases[id].events != *expected {
                return Err(format!(
                    "Stress edit/undo produced unexpected note data: {}",
                    app.status
                ));
            }
            Ok(true)
        }
    }
}

impl Scene {
    pub fn stress(path: &Path, name: &str) -> Result<Self, String> {
        if name == "mixer" {
            let mut scene = Self::mixer(path)?;
            scene.report.scene = "stress-mixer";
            scene.steps.pop();
            scene.finish_stress();
            return Ok(scene);
        }
        if !matches!(name, "edit" | "navigation") {
            return Err("Stress scene must be mixer, edit, or navigation".into());
        }
        let document = Document::read(path)?;
        let (track, slot, phrase_index, phrase_id) = document
            .song
            .tracks
            .iter()
            .enumerate()
            .flat_map(|(track, value)| {
                value
                    .slots
                    .iter()
                    .enumerate()
                    .map(move |(slot, usage)| (track, slot, usage))
            })
            .filter_map(|(track, slot, usage)| {
                usage
                    .as_ref()
                    .and_then(|usage| usage.chain.pattern().ok())
                    .map(|chain| (track, slot, chain))
            })
            .flat_map(|(track, slot, chain)| {
                document.song.chains[&chain]
                    .phrases
                    .iter()
                    .enumerate()
                    .map(move |(index, usage)| (track, slot, index, usage.phrase))
            })
            .find(|(_, _, _, phrase)| {
                document.song.phrases[phrase]
                    .events
                    .iter()
                    .any(|event| event.note.is_some())
            })
            .ok_or("Stress tour needs a placed phrase containing notes")?;
        if !matches!(
            document.song.phrases[&phrase_id].form,
            PhraseForm::PianoRoll
        ) {
            return Err("Corpus stress tour expects imported Piano Roll phrases".into());
        }
        let mut scene = Self::song(path)?;
        scene.report.scene = if name == "edit" {
            "stress-edit"
        } else {
            "stress-navigation"
        };
        scene.steps.pop();
        if name == "navigation" {
            scene.navigation_tour();
        }
        use Direction::*;
        use SemanticAction::*;
        scene.push_stress(
            "Choose sounding track",
            Event::Repeat(Navigate(Right), track),
            Checkpoint::Settled,
        );
        scene.push_stress(
            "Choose sounding chain slot",
            Event::Repeat(Navigate(Down), slot),
            Checkpoint::Settled,
        );
        scene.push(
            "Open Chain during playback",
            Event::Action(NavigatePage(Right)),
            Check::View(View::Chain),
        );
        scene.push_stress(
            "Choose sounding phrase",
            Event::Repeat(Navigate(Down), phrase_index),
            Checkpoint::Phrase(phrase_id),
        );
        scene.push(
            "Open Piano Roll during playback",
            Event::Action(NavigatePage(Right)),
            Check::Piano(piano::Checkpoint::Roll),
        );
        if name == "edit" {
            let baseline = &document.song.phrases[&phrase_id].events;
            for velocity in [64, 48, 80, 96] {
                // A corpus phrase may already have the requested velocity.
                // Undo/redo must exercise an actual edit, not an intentional no-op.
                let velocity = if baseline
                    .iter()
                    .filter_map(|event| event.note.as_ref())
                    .all(|note| note.velocity == velocity)
                {
                    63
                } else {
                    velocity
                };
                let mut expected = baseline.clone();
                for event in &mut expected {
                    if let Some(note) = &mut event.note {
                        note.velocity = velocity;
                    }
                }
                scene.push(
                    "Open note tools",
                    Event::Action(OpenPopover),
                    Check::Panel("notes_all"),
                );
                scene.push(
                    "Select all phrase notes",
                    Event::Activate("notes_all"),
                    Check::Panel("piano_inspect"),
                );
                scene.push(
                    "Inspect selected notes",
                    Event::Activate("piano_inspect"),
                    Check::Panel("piano_velocity_value"),
                );
                scene.push(
                    "Enter exact velocity",
                    Event::Activate("piano_velocity_value"),
                    Check::Panel("text_submit"),
                );
                let value = match velocity {
                    63 => "63",
                    64 => "64",
                    48 => "48",
                    80 => "80",
                    _ => "96",
                };
                scene.push(
                    "Preview velocity edit",
                    Event::Submit("piano_velocity_value", value),
                    Check::Panel("piano_apply"),
                );
                scene.push_stress(
                    "Commit multi-note edit while playing",
                    Event::Activate("piano_apply"),
                    Checkpoint::Settled,
                );
                scene.push_stress(
                    "Confirm shared edit when required",
                    Event::AcceptSharedEdit,
                    Checkpoint::Events(phrase_id, expected.clone()),
                );
                scene.push(
                    "Open history tools",
                    Event::Action(OpenPopover),
                    Check::Panel("undo"),
                );
                scene.push_stress(
                    "Undo restores all note data",
                    Event::Activate("undo"),
                    Checkpoint::Events(phrase_id, baseline.clone()),
                );
                scene.push_stress(
                    "Redo restores committed edit",
                    Event::Activate("redo"),
                    Checkpoint::Events(phrase_id, expected),
                );
                scene.push_stress(
                    "Undo leaves source phrase intact",
                    Event::Activate("undo"),
                    Checkpoint::Events(phrase_id, baseline.clone()),
                );
                scene.push(
                    "Open quantize preview",
                    Event::Activate("piano_quantize"),
                    Check::Panel("piano_quantize_grid"),
                );
                scene.push(
                    "Choose quantize grid",
                    Event::Activate("piano_quantize_grid"),
                    Check::Panel("piano_triplet"),
                );
                scene.push(
                    "Triplet quantization",
                    Event::Activate("piano_triplet"),
                    Check::Panel("piano_time:8"),
                );
                scene.push(
                    "Eighth-note triplet preview",
                    Event::Activate("piano_time:8"),
                    Check::Panel("piano_time_apply"),
                );
                scene.push(
                    "Apply quantize grid to preview",
                    Event::Activate("piano_time_apply"),
                    Check::Panel("piano_close"),
                );
                scene.push_stress(
                    "Cancel preview preserves phrase",
                    Event::Activate("piano_close"),
                    Checkpoint::Events(phrase_id, baseline.clone()),
                );
            }
        } else {
            for _ in 0..4 {
                scene.push_stress(
                    "Scan Piano Roll forward",
                    Event::Repeat(Navigate(Right), 12),
                    Checkpoint::Settled,
                );
                scene.push_stress(
                    "Scan Piano Roll pitches",
                    Event::Repeat(Navigate(Up), 8),
                    Checkpoint::Settled,
                );
                scene.push_stress(
                    "Scan Piano Roll back",
                    Event::Repeat(Navigate(Left), 12),
                    Checkpoint::Settled,
                );
                scene.push_stress(
                    "Scan Piano Roll pitches back",
                    Event::Repeat(Navigate(Down), 8),
                    Checkpoint::Settled,
                );
                scene.push(
                    "Open phrase settings",
                    Event::Action(NavigatePage(Up)),
                    Check::Panel("phrase_form"),
                );
                scene.push(
                    "Return from phrase settings",
                    Event::Activate("back"),
                    Check::Piano(piano::Checkpoint::Roll),
                );
            }
        }
        scene.finish_stress();
        Ok(scene)
    }

    fn navigation_tour(&mut self) {
        use Direction::*;
        use SemanticAction::*;
        for _ in 0..3 {
            for (direction, count) in [(Right, 15), (Left, 15), (Down, 16), (Up, 16)] {
                self.push_stress(
                    "Scan Song grid with queued repeats",
                    Event::Repeat(Navigate(direction), count),
                    Checkpoint::Settled,
                );
            }
            self.push(
                "Open global settings during playback",
                Event::Action(OpenMidiSettings),
                Check::Panel("settings_music"),
            );
            for tab in [
                "settings_clock",
                "settings_devices",
                "settings_files",
                "settings_defaults",
                "settings_music",
            ] {
                self.push(
                    "Switch settings tabs during playback",
                    Event::Activate(tab),
                    Check::Panel(tab),
                );
            }
            self.push(
                "Return from global settings",
                Event::Action(NavigatePage(Down)),
                Check::Playing,
            );
            self.push(
                "Open main menu",
                Event::Activate("menu"),
                Check::Panel("home"),
            );
            self.push(
                "Open instruments during playback",
                Event::Activate("instruments"),
                Check::Synth,
            );
            self.push_stress(
                "Navigate native patch controls",
                Event::Repeat(Navigate(Down), 6),
                Checkpoint::Settled,
            );
            self.push_stress(
                "Navigate native patch controls back",
                Event::Repeat(Navigate(Up), 6),
                Checkpoint::Settled,
            );
            self.push(
                "Return from instruments",
                Event::Action(NavigatePage(Down)),
                Check::Panel("home"),
            );
            self.push("Return to Song", Event::Activate("home"), Check::Playing);
        }
    }
    fn push(&mut self, label: &'static str, event: Event, check: Check) {
        self.steps.push(Step {
            label,
            event,
            check,
        });
    }
    fn push_stress(&mut self, label: &'static str, event: Event, check: Checkpoint) {
        self.push(label, event, Check::Stress(check));
    }
    fn finish_stress(&mut self) {
        self.push(
            "Stop stress playback and release voices",
            Event::Action(SemanticAction::PreviewStart),
            Check::Stopped,
        );
    }
}
