//! Preserve STK delay history and mesh equations while reducing memory traffic.
use std::path::{Path, PathBuf};

pub fn stage(output: &Path) -> PathBuf {
    let destination = output.join("stk-source");
    // Relative includes must all resolve to the same staged Stk.h. STK's
    // upstream rate-dependent calculations read the prepared processor's rate;
    // its global setter/observer list is never mutated by the host adapter.
    for entry in std::fs::read_dir("vendor/audio/stk/include").unwrap() {
        let source = entry.unwrap().path();
        if source.is_file() {
            let mut header = std::fs::read_to_string(&source).unwrap();
            if source.file_name().unwrap() == "Stk.h" {
                replace(
                    &mut header,
                    "namespace stk {",
                    "double crest_stk_sample_rate(double fallback) noexcept;\n\nnamespace stk {",
                );
                replace(
                    &mut header,
                    "return srate_;",
                    "return ::crest_stk_sample_rate(srate_);",
                );
            }
            write(&destination.join(source.file_name().unwrap()), &header);
        }
    }
    let mut header = std::fs::read_to_string("vendor/audio/stk/include/BandedWG.h").unwrap();
    replace(
        &mut header,
        "#include \"DelayL.h\"",
        "#include \"banded_delay.h\"",
    );
    replace(&mut header, "DelayL   delay_", "BandedDelay delay_");
    replace(
        &mut header,
        "  bool doPluck_;",
        "  bool doPluck_;\n  bool cleared_ = true;",
    );
    write(&destination.join("BandedWG.h"), &header);
    let mut banded = std::fs::read_to_string("vendor/audio/stk/src/BandedWG.cpp").unwrap();
    replace(
        &mut banded,
        "void BandedWG :: clear( void )\n{",
        "void BandedWG :: clear( void )\n{\n  cleared_ = true;",
    );
    replace(
        &mut banded,
        "  StkFloat radius;",
        "  cleared_ = true;\n  StkFloat radius;",
    );
    replace(
        &mut banded,
        "void BandedWG :: pluck( StkFloat amplitude )\n{",
        "void BandedWG :: pluck( StkFloat amplitude )\n{\n  cleared_ = false;",
    );
    replace(
        &mut banded,
        "StkFloat BandedWG :: tick( unsigned int )\n{",
        r"StkFloat BandedWG :: tick( unsigned int )
{
  // A cleared plucked model has no excitation or history. Absolute circular
  // indices are unobservable: its integral delays chase the write pointer,
  // and the next note retunes before plucking. Bowed models still advance
  // their envelope and velocity every sample. Never truncate a nonzero tail.
  if (doPluck_ && cleared_) { lastFrame_[0] = 0.0; return 0.0; }
  cleared_ = false;",
    );
    write(&destination.join("BandedWG.cpp"), &banded);

    let mut mesh = std::fs::read_to_string("vendor/audio/stk/src/Mesh2D.cpp").unwrap();
    for (tick, suffix) in [("tick0", ""), ("tick1", "1")] {
        let function = mesh
            .find(&format!("StkFloat Mesh2D :: {tick}( void )"))
            .unwrap();
        let start = function
            + mesh[function..]
                .find("  // Update junction velocities.")
                .unwrap();
        let end = start
            + mesh[start..]
                .find("      // Update positive-going waves.")
                .unwrap();
        // The outgoing pass reads only this junction's velocity and the
        // original buffers. Its writes all target the alternate buffers.
        let fused = format!(
            r"  // Update each junction and its outgoing alternate-buffer waves.
  for (x=0; x<NX_-1; x++) {{
    for (y=0; y<NY_-1; y++) {{
      StkFloat vxy = ( vxp{suffix}_[x][y] + vxm{suffix}_[x+1][y] +
                      vyp{suffix}_[x][y] + vym{suffix}_[x][y+1] ) * VSCALE;
      v_[x][y] = vxy;
"
        );
        mesh.replace_range(start..end, &fused);
    }
    write(&destination.join("Mesh2D.cpp"), &mesh);
    destination
}

fn replace(text: &mut String, before: &str, after: &str) {
    assert_eq!(
        text.matches(before).count(),
        1,
        "STK adaptation no longer matches: {before}"
    );
    *text = text.replace(before, after);
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    if std::fs::read_to_string(path).ok().as_deref() != Some(text) {
        std::fs::write(path, text).unwrap();
    }
}
