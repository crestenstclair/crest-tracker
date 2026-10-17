mod app;
mod corridors;
mod library;
mod midi_clock;
mod midi_import;
mod midi_output;
mod performance_editor;
mod persistence;
mod preferences;
mod recovery;
mod synth;
mod workspace;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.is_empty() {
        return synth::run(app::App::new());
    }
    if arguments.len() == 1 {
        match arguments[0].to_str() {
            Some("--synth-inventory") => return synth::write_inventory(),
            Some("--demo-corridors") => return synth::run_demo(),
            Some("--help") => {
                println!(
                    "Usage: crest-tracker-app [--demo-corridors | --synth-inventory | --help]"
                );
                return Ok(());
            }
            _ => {}
        }
    }
    Err("Unknown arguments. Usage: crest-tracker-app [--demo-corridors | --synth-inventory | --help]".into())
}
