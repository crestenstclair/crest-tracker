mod app;
mod corridors;
mod library;
mod midi_clock;
mod midi_import;
mod midi_output;
mod midi_plan;
mod persistence;
mod preferences;
mod recovery;
mod synth;
mod workspace;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _performance = crest_synth::shell::performance::start_from_env()?;
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.is_empty() {
        return synth::run(app::App::new());
    }
    if arguments.len() == 2 && arguments[0] == "--demo" {
        return synth::run_named_demo(arguments[1].to_str().ok_or("Demo name must be UTF-8")?);
    }
    if arguments.len() == 2 && arguments[0] == "--demo-song" {
        return synth::run_song_demo(std::path::Path::new(&arguments[1]));
    }
    if arguments.len() == 3 && arguments[0] == "--demo-stress" {
        return synth::run_stress_demo(
            std::path::Path::new(&arguments[2]),
            arguments[1].to_str().ok_or("Scene must be UTF-8")?,
        );
    }
    if arguments.len() == 2 && arguments[0] == "--demo-mixer" {
        return synth::run_mixer_demo(std::path::Path::new(&arguments[1]));
    }
    if arguments.len() == 2 && arguments[0] == "--midi-inspect" {
        let report = midi_plan::inspect_file(std::path::Path::new(&arguments[1]))?;
        serde_json::to_writer_pretty(std::io::stdout().lock(), &report)?;
        println!();
        return Ok(());
    }
    if arguments.len() == 4 && arguments[0] == "--midi-import" {
        let report = midi_plan::import_file(
            std::path::Path::new(&arguments[1]),
            std::path::Path::new(&arguments[2]),
            std::path::Path::new(&arguments[3]),
        )?;
        serde_json::to_writer_pretty(std::io::stdout().lock(), &report)?;
        println!();
        return Ok(());
    }
    if arguments.len() == 1 {
        match arguments[0].to_str() {
            Some("--synth-inventory") => return synth::write_inventory(),
            Some("--demo-corridors") => return synth::run_demo(),
            Some("--demo-piano") => return synth::run_piano_demo(),
            Some("--demo-cursors") => return synth::run_cursor_demo(),
            Some("--list-demos") => {
                println!(
                    "drums\neuclidean\nrandom\nchords\nfx\nnavigation\nrest-kill\npiano\ncursors\ncorridors\ntransport"
                );
                return Ok(());
            }
            Some("--help") => {
                println!("{USAGE}");
                return Ok(());
            }
            _ => {}
        }
    }
    Err(format!("Unknown arguments.\n{USAGE}").into())
}

const USAGE: &str = "Usage:
  crest-tracker-app                         Open Tracker
  crest-tracker-app --list-demos            List event scenes
  crest-tracker-app --demo NAME             Run an isolated visible scene
  crest-tracker-app --demo-song SONG        Open and play an imported .crest-song
  crest-tracker-app --demo-stress SCENE SONG  Stress mixer, edit, or navigation during playback
  crest-tracker-app --demo-mixer SONG       Adjust Mixer during imported Song playback
  crest-tracker-app --midi-inspect MIDI      Write analysis, catalog and plan schema JSON
  crest-tracker-app --midi-import MIDI PLAN OUTPUT
                                           Create a portable .crest-song with sibling assets/
  crest-tracker-app --synth-inventory       Write production capability inventory
Legacy aliases: --demo-corridors, --demo-piano, --demo-cursors";
