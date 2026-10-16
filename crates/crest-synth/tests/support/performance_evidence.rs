//! Retain existing scene evidence when the performance suite requests it.
pub fn retain(report: &crest_synth::testing::LiveDemoReport) {
    if let Some(directory) = std::env::var_os("CREST_PERFORMANCE_EVIDENCE_DIR") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).expect("create scene evidence directory");
        std::fs::write(directory.join("live-demo.json"), report.to_json().unwrap())
            .expect("retain complete scene evidence");
    }
}
