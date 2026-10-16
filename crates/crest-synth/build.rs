#[path = "build_support/daisy.rs"]
mod daisy_build;
#[path = "build_support/mutable.rs"]
mod mutable_build;
#[path = "build_support/r8brain.rs"]
mod r8brain_build;
#[path = "build_support/sfizz.rs"]
mod sfizz_build;
#[path = "build_support/stk.rs"]
mod stk_build;
fn main() {
    // Track complete header/source trees as well as embedded inputs. These
    // remain authoritative when Cargo reuses native output for Rust-only edits.
    for input in [
        "native/braids",
        "vendor/braids",
        "native/chorus",
        "vendor/chorus",
        "native/audio",
        "vendor/audio",
        "assets/sfizz-default.sfz",
    ] {
        println!("cargo:rerun-if-changed={input}");
    }
    sfizz_build::build();
    build_audio_catalog();
    println!("cargo:rustc-link-lib=static=crest_sfizz");
    const SOURCES: &[&str] = &[
        "native/braids/crest_braids.cpp",
        "vendor/braids/braids/analog_oscillator.cc",
        "vendor/braids/braids/digital_oscillator.cc",
        "vendor/braids/braids/macro_oscillator.cc",
        "vendor/braids/braids/resources.cc",
        "vendor/braids/stmlib/utils/random.cc",
    ];

    let mut build = native_build();
    build
        .cpp(true)
        .std("c++11")
        .include("vendor/braids")
        .define("stmlib", Some("crest_braids_stmlib"))
        .include("native/braids")
        .flag_if_supported("-fno-exceptions")
        .flag_if_supported("-fno-rtti")
        .warnings(false);
    for source in SOURCES {
        build.file(source);
    }
    build.compile("crest_braids");

    const CHORUS_SOURCES: &[&str] = &[
        "native/chorus/crest_chorus.cpp",
        "vendor/chorus/rings/resources.cc",
    ];
    let mut chorus = native_build();
    chorus
        .cpp(true)
        .std("c++11")
        .include("vendor/chorus")
        .define("stmlib", Some("crest_chorus_stmlib"))
        .define("rings", Some("crest_chorus_rings"))
        .include("native/chorus")
        .define("TEST", None)
        .flag_if_supported("-fno-exceptions")
        .flag_if_supported("-fno-rtti")
        .warnings(false);
    for source in CHORUS_SOURCES {
        chorus.file(source);
    }
    chorus.compile("crest_chorus");

    // Tauri webview-shell scaffolding (mission webview-shell-foundation):
    // generates the context consumed by `tauri::generate_context!` for the
    // window-only, no-frontend-bundle setup declared in `tauri.conf.json`.
    // Extends the existing C++ vendor build; never replaces it.
    tauri_build::build();
}

fn build_audio_catalog() {
    let generated = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let r8brain_sources = r8brain_build::stage(&generated);
    let mutable_sources = mutable_build::stage(&generated);
    let stk_sources = stk_build::stage(&generated);
    let msfa_sources = stage_msfa(&generated);
    std::fs::create_dir_all(generated.join("stmlib/utils")).unwrap();
    // Select each instance's original Mutable generator through the prepared
    // native scope; no first-use C++ TLS allocation occurs on Darwin audio.
    let random_header = std::fs::read_to_string("vendor/audio/stmlib/utils/random.h")
        .unwrap()
        .replace(
            "#include \"stmlib/stmlib.h\"",
            "#include \"stmlib/stmlib.h\"\n#include \"processor_state.h\"",
        )
        .replace("static uint32_t rng_state_;", "")
        .replace("rng_state_", "crest_processor_state().mutable_state");
    std::fs::write(generated.join("stmlib/utils/random.h"), random_header).unwrap();
    // mda EPiano crossfades its sample table at construction. Give each
    // prepared instance its own table, preserving the upstream sample DSP.
    let epiano_header = std::fs::read_to_string("vendor/audio/mda/plugins/mdaEPiano.h")
        .unwrap()
        .replace(
            "short *waves;",
            "std::vector<short> wave_storage_; short *waves;",
        );
    std::fs::write(generated.join("mdaEPiano.h"), epiano_header).unwrap();
    let epiano_source = std::fs::read_to_string("vendor/audio/mda/plugins/mdaEPiano.cpp").unwrap()
        .replace("waves = epianoData;", "wave_storage_.assign(std::begin(epianoData), std::end(epianoData)); waves = wave_storage_.data();");
    std::fs::write(generated.join("crest_mda_epiano.cpp"), epiano_source).unwrap();
    let default_sfz = std::fs::read_to_string("assets/sfizz-default.sfz").unwrap();
    std::fs::write(
        generated.join("crest_sample_default.h"),
        format!("static const char crest_sample_default[] = R\"SFZ({default_sfz})SFZ\";"),
    )
    .unwrap();
    let nam_default = std::fs::read_to_string("vendor/audio/nam/example_models/lstm.nam").unwrap();
    std::fs::write(generated.join("crest_nam_default.h"),format!("// Upstream NAM example_models/lstm.nam, retained under the vendored MIT license.\nstatic const char crest_nam_default[] = R\"NAM({nam_default})NAM\";")).unwrap();
    let msfa_source =
        std::fs::read_to_string("vendor/audio/msfa/app/src/main/jni/synth_unit.cc").unwrap();
    let default_patch = msfa_source
        .split("char epiano[] = {")
        .nth(1)
        .unwrap()
        .split("};")
        .next()
        .unwrap();
    std::fs::write(generated.join("crest_msfa_default.h"),format!("// Apache-2.0, Copyright 2012 Google Inc. Extracted unchanged from synth_unit.cc.\nstatic const char crest_msfa_default[] = {{{default_patch}}};")).unwrap();
    std::fs::create_dir_all(generated.join("signalsmith-linear")).unwrap();
    for header in ["linear.h", "stft.h", "fft.h", "approx.h"] {
        std::fs::copy(
            std::path::Path::new("vendor/audio/linear").join(header),
            generated.join("signalsmith-linear").join(header),
        )
        .unwrap();
    }
    let mut build = native_build();
    build
        .include(generated.join("sfizz-source/src"))
        .include("vendor/audio/sfizz/external/filesystem/include")
        .include("vendor/audio/sfizz/external/abseil-cpp")
        .include("vendor/audio/sfizz/external/simde");
    build
        .include("vendor/audio/mda/plugins")
        .include(&msfa_sources);
    build
        .include(&generated)
        .include("vendor/audio")
        .include(&mutable_sources)
        .include("vendor/audio/mutable")
        .define("TEST", None);
    build
        .cpp(true)
        .std("c++20")
        // Embedded DSP assumes zeroed static storage before its constructors.
        // GCC lifetime DSE otherwise erases ZeroInitialized's pre-constructor
        // stores, leaving histories indeterminate in optimized heap voices.
        .flag_if_supported("-fno-lifetime-dse")
        .include("vendor/audio/nam/Dependencies/eigen")
        .include("vendor/audio/nam/Dependencies/nlohmann")
        .define("NAM_SAMPLE_FLOAT", None)
        .define("EIGEN_MPL2_ONLY", None)
        .include("native/audio")
        .include(&r8brain_sources)
        .include(&stk_sources)
        .include("vendor/audio/stk/include")
        .warnings(false);
    for entry in std::fs::read_dir("native/audio").unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|s| s.to_str()) == Some("cpp")
            && path.file_name().and_then(|s| s.to_str()) != Some("stk_waves.cpp")
        {
            build.file(path);
        }
    }
    // The upstream compressor's local static dither counters become owned
    // fields. The signal algorithm is unchanged; instances no longer interfere.
    let butter = generated.join("crest_butter");
    std::fs::create_dir_all(&butter).unwrap();
    for name in ["ButterComp2.h", "ButterComp2.cpp", "ButterComp2Proc.cpp"] {
        let mut source = std::fs::read_to_string(format!(
            "vendor/audio/airwindows/plugins/WinVST/ButterComp2/{name}"
        ))
        .unwrap();
        if name.ends_with(".h") {
            source = source.replace(
                "private:",
                "private:\n    int noisesourceL = 0, noisesourceR = 850010;",
            );
        }
        if name.ends_with("Proc.cpp") {
            source = source
                .replace("static int noisesourceL = 0;", "")
                .replace("static int noisesourceR = 850010;", "");
        }
        std::fs::write(butter.join(name), source).unwrap();
    }
    let daisy = daisy_build::stage(&generated);
    build.include(&daisy);
    for folder in std::fs::read_dir(&daisy).unwrap() {
        let path = folder.unwrap().path();
        if path.is_dir() {
            build.include(path);
        }
    }
    build.compile("crest_audio");
    let mut mutable = native_build();
    mutable.include("native/audio");
    mutable
        .cpp(true)
        .std("c++17")
        .warnings(false)
        .define("TEST", None)
        .include(&generated)
        .include("vendor/audio")
        .include(&mutable_sources)
        .include("vendor/audio/mutable")
        .flag("-include")
        .flag("cstdio");
    for module in ["plaits", "rings", "elements", "clouds", "warps"] {
        mutable_build::add_sources(
            &mut mutable,
            &std::path::Path::new("vendor/audio/mutable")
                .join(module)
                .join("dsp"),
            &mutable_sources,
        );
        mutable.file(format!("vendor/audio/mutable/{module}/resources.cc"));
    }
    for source in [
        "tides2/poly_slope_generator.cc",
        "tides2/ramp_generator.cc",
        "tides2/resources.cc",
        "peaks/drums/bass_drum.cc",
        "peaks/drums/snare_drum.cc",
        "peaks/drums/fm_drum.cc",
        "peaks/drums/high_hat.cc",
        "peaks/resources.cc",
    ] {
        let path = std::path::Path::new("vendor/audio/mutable").join(source);
        if path.exists() {
            mutable.file(path);
        }
    }
    for source in ["dsp/units.cc", "dsp/atan.cc"] {
        let path = std::path::Path::new("vendor/audio/stmlib").join(source);
        if path.exists() {
            mutable.file(path);
        }
    }
    mutable.compile("crest_mutable");
    let mut dsp = native_build();
    dsp.cpp(true)
        .std("c++17")
        .warnings(false)
        .flag("-include")
        .flag("native/audio/random.h")
        .include(&daisy);
    for folder in std::fs::read_dir(&daisy).unwrap() {
        let path = folder.unwrap().path();
        if path.is_dir() {
            dsp.include(&path);
        }
    }
    for source in [
        "Drums/analogbassdrum.cpp",
        "Drums/synthbassdrum.cpp",
        "Drums/hihat.cpp",
        "PhysicalModeling/stringvoice.cpp",
        "PhysicalModeling/modalvoice.cpp",
        "PhysicalModeling/KarplusString.cpp",
        "PhysicalModeling/resonator.cpp",
        "Effects/flanger.cpp",
        "Effects/phaser.cpp",
        "Effects/tremolo.cpp",
        "Effects/autowah.cpp",
        "Dynamics/limiter.cpp",
        "Synthesis/oscillator.cpp",
        "Filters/svf.cpp",
        "Utility/dcblock.cpp",
        "Dynamics/crossfade.cpp",
    ] {
        dsp.file(daisy.join(source));
    }
    dsp.compile("crest_daisy");
    let mut stk = native_build();
    // Keep STK's resident FileRead implementation beside its callers so
    // one-pass ELF archive extraction does not leave a backwards dependency.
    stk.file("native/audio/stk_waves.cpp");
    stk.cpp(true)
        .std("c++17")
        .include(&stk_sources)
        .include("native/audio")
        .include("vendor/audio/stk/include")
        .warnings(false)
        .flag("-include")
        .flag("native/audio/random.h");
    for source in std::fs::read_to_string("native/audio/stk_sources.txt")
        .unwrap()
        .lines()
    {
        let name = std::path::Path::new(source).file_name().unwrap();
        if name == "BandedWG.cpp" || name == "Mesh2D.cpp" {
            stk.file(stk_sources.join(name));
        } else {
            stk.file(source);
        }
    }
    stk.compile("crest_stk");
    let mut msfa = native_build();
    msfa.cpp(true)
        .std("c++17")
        .warnings(false)
        .include(&msfa_sources);
    for source in [
        "dx7note",
        "env",
        "pitchenv",
        "fm_core",
        "fm_op_kernel",
        "freqlut",
        "exp2",
        "sin",
        "lfo",
        "patch",
    ] {
        msfa.file(msfa_sources.join(format!("{source}.cc")));
    }
    msfa.compile("crest_msfa");
    let mut nam = native_build();
    nam.cpp(true)
        .std("c++20")
        .warnings(false)
        .define("NAM_SAMPLE_FLOAT", None)
        .define("EIGEN_MPL2_ONLY", None)
        .include("vendor/audio/nam/Dependencies/eigen")
        .include("vendor/audio/nam/Dependencies/nlohmann")
        .include("vendor/audio/nam/Dependencies/AudioDSPTools");
    add_sources(
        &mut nam,
        std::path::Path::new("vendor/audio/nam/NAM"),
        "cpp",
    );
    nam.compile("crest_nam");
    let mut convolution = native_build();
    convolution.cpp(true).std("c++17").warnings(false);
    for source in ["FFTConvolver.cpp", "AudioFFT.cpp", "Utilities.cpp"] {
        convolution.file(std::path::Path::new("vendor/audio/fftconvolver").join(source));
    }
    convolution.compile("crest_convolution");
}

/// Keep pinned MSFA sources intact while making their declarations portable.
fn stage_msfa(output: &std::path::Path) -> std::path::PathBuf {
    let staged = output.join("msfa-source");
    std::fs::create_dir_all(&staged).unwrap();
    for entry in std::fs::read_dir("vendor/audio/msfa/app/src/main/jni").unwrap() {
        let source = entry.unwrap().path();
        if !matches!(
            source.extension().and_then(|s| s.to_str()),
            Some("h" | "cc")
        ) {
            continue;
        }
        let mut text = std::fs::read_to_string(&source).unwrap();
        match source.file_name().and_then(|s| s.to_str()).unwrap() {
            "aligned_buf.h" => {
                text = text.replace(
                    "#define __ALIGNED_BUF_H",
                    "#define __ALIGNED_BUF_H\n#include <stddef.h>\n#include <stdint.h>",
                );
            }
            "dx7note.cc" => {
                // Select the original helpers, not libstdc++ overloads pulled
                // into scope by the upstream `using namespace std` directive.
                text = text.replace("min(", "::min(").replace("max(", "::max(");
            }
            _ => {}
        }
        std::fs::write(staged.join(source.file_name().unwrap()), text).unwrap();
    }
    staged
}

fn add_sources(build: &mut cc::Build, path: &std::path::Path, extension: &str) {
    for entry in std::fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            add_sources(build, &path, extension);
        } else if path.extension().and_then(|s| s.to_str()) == Some(extension) {
            build.file(path);
        }
    }
}

fn native_build() -> cc::Build {
    let mut build = cc::Build::new();
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let target = std::env::var("MACOSX_DEPLOYMENT_TARGET").unwrap_or_else(|_| "11.0".into());
        build.flag(format!("-mmacosx-version-min={target}"));
    }
    build
}
