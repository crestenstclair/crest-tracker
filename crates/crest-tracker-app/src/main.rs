mod app;
mod feedback;
mod library;
mod midi_clock;
mod midi_output;
mod persistence;
mod preferences;
mod recovery;
mod synth;
mod webview_wire;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.is_empty() {
        return synth::run(app::App::new());
    }
    if arguments.len() == 1 {
        match arguments[0].to_str() {
            Some("--synth-inventory") => return synth::write_inventory(),
            Some("--help") => {
                println!("Usage: crest-tracker-app [--synth-inventory | --help]");
                return Ok(());
            }
            _ => {}
        }
    }
    Err("Unknown arguments. Usage: crest-tracker-app [--synth-inventory | --help]".into())
}
