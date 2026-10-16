use std::{
    path::{Path, PathBuf},
    process::Command,
};

fn write_changed(path: &Path, bytes: &[u8]) {
    if std::fs::read(path).ok().as_deref() != Some(bytes) {
        std::fs::write(path, bytes).unwrap();
    }
}
fn adapted(source: &Path) -> Vec<u8> {
    let bytes = std::fs::read(source).unwrap();
    let edits:&[(&str,&str,&str)]=&[
        ("src/sfizz/parser/ParserListener.h","    // low-level parsing","    // The embedding host can reject a file before the parser opens it.\n    virtual bool onParseFile(const std::string&) { return true; }\n    // low-level parsing"),
        ("src/sfizz/parser/Parser.cpp","    if (!reader) {\n        auto fileReader", "    if (!reader) {\n        if (_listener && !_listener->onParseFile(fullPath.string())) {\n            emitError(makeErrorRange(), \"File rejected by asset boundary\"); return;\n        }\n        auto fileReader"),
        ("src/sfizz/Synth.cpp","static constexpr bool loaderParsesPermissively = true;","static constexpr bool loaderParsesPermissively = false;"),
        ("src/sfizz/Synth.cpp","    impl.finalizeSfzLoad();\n    return true;","    const size_t expectedRegions = impl.layers_.size();\n    impl.finalizeSfzLoad();\n    return impl.layers_.size() == expectedRegions;"),
        ("external/st_audiofile/src/st_audiofile.c","    // Try WV\n    {\n        af->wv =\n            WavpackOpenRawDecoder", "    // WavPack memory input must contain a complete file header.\n    // Raw Matroska payload inference is unsafe for arbitrary imported bytes.\n    if (length < 32 || length > INT32_MAX || memcmp(memory, \"wvpk\", 4)) { free(af); return NULL; }\n    {\n        af->wv =\n            WavpackOpenRawDecoder"),
        ("external/st_audiofile/src/st_audiofile.c","#include <stdlib.h>","#include <stdlib.h>\n#include <string.h>"),
        ("src/sfizz/FilePool.cpp","    auto reader = createAudioReaderFromMemory(data.data(), data.size(), fileId.isReverse());\n    auto fileInformation = getReaderInformation(reader.get());", "    std::error_code error;\n    auto reader = createAudioReaderFromMemory(data.data(), data.size(), fileId.isReverse(), &error);\n    if (error) return {};\n    auto fileInformation = getReaderInformation(reader.get());\n    if (!fileInformation || reader->frames() == 0) return {};"),
        ("src/sfizz/Synth.cpp","#include \"Config.h\"","#include \"Config.h\"\n#include <stdexcept>"),
        ("src/sfizz/Synth.cpp","    filePool.loadFromRam(id, data);","    if (!filePool.loadFromRam(id, data)) throw std::runtime_error(\"Invalid embedded SFZ sample\");"),
        ("CMakeLists.txt","add_subdirectory(clients)","# Crest embeds only the synthesis library."),
        ("src/sfizz/SynthMessagingHelper.hpp","std::vector<unsigned> indices;","absl::InlinedVector<unsigned, maxIndices> indices;"),
        ("src/sfizz/SynthMessagingHelper.hpp","#include <type_traits>","#include <type_traits>\n#include <absl/container/inlined_vector.h>"),
        ("cmake/SfizzConfig.cmake","set(CMAKE_OSX_DEPLOYMENT_TARGET \"10.13\")","# The embedding build supplies the deployment target."),
        ("src/CMakeLists.txt","configure_file(${PROJECT_SOURCE_DIR}/scripts/doxygen/Doxyfile.in ${PROJECT_SOURCE_DIR}/Doxyfile @ONLY)","# Documentation is not part of the embedded build."),
        ("src/Config.h.in","constexpr bool loadInRam { false };","constexpr bool loadInRam { true };"),
        ("src/sfizz/FilePool.cpp","void sfz::FilePool::setRamLoading(bool loadInRam) noexcept\n{","void sfz::FilePool::setRamLoading(bool loadInRam) noexcept\n{\n    loadInRam = true; // Resident ownership is mandatory in Crest."),
        ("src/sfizz/FilePool.cpp","void sfz::FilePool::triggerGarbageCollection() noexcept\n{","void sfz::FilePool::triggerGarbageCollection() noexcept\n{\n    if (loadInRam) return; // Resident data retires with its prepared graph."),
        ("external/atomic_queue/include/atomic_queue/atomic_queue.h","Base::template do_pop_any(","Base::do_pop_any("),
        ("external/atomic_queue/include/atomic_queue/atomic_queue.h","Base::template do_push_any(","Base::do_push_any("),
        ("src/sfizz/FilePool.h","std::thread dispatchThread { &FilePool::dispatchingJob, this };","std::thread dispatchThread;"),
        ("src/sfizz/FilePool.h","std::thread garbageThread { &FilePool::garbageJob, this };","std::thread garbageThread;"),
        ("src/sfizz/FilePool.cpp","threadPool(globalThreadPool())","threadPool()"),
        // Crest owns callback observations and measures acceptance outside the
        // renderer. sfizz's unused per-stage profiler otherwise reads the host
        // clock for every voice stage. Keep its disabled durations at zero;
        // unrelated clock users and every synthesis operation remain intact.
        ("src/sfizz/utility/Timing.h","(highResNow() - creationTime).count()","0.0"),
        ("src/sfizz/utility/Timing.h","const TimePoint creationTime { highResNow() };","const TimePoint creationTime {};"),
        ("src/sfizz/FilePool.cpp","garbageThread.join();","if (garbageThread.joinable()) garbageThread.join();"),
        ("src/sfizz/FilePool.cpp","dispatchThread.join();","if (dispatchThread.joinable()) dispatchThread.join();"),
        // At delay zero, insertion only updates the already-normalized first
        // event. Avoid touching every controller's separate allocation on each
        // block when there is no delayed event to collapse.
        ("src/sfizz/MidiState.h","    int activeNotes { 0 };","    bool eventsNeedFlush { false };\n    int activeNotes { 0 };"),
        ("src/sfizz/MidiState.cpp","void sfz::MidiState::flushEvents() noexcept\n{","void sfz::MidiState::flushEvents() noexcept\n{\n    if (!eventsNeedFlush) return;\n    eventsNeedFlush = false;"),
        ("src/sfizz/MidiState.cpp","void sfz::MidiState::insertEventInVector(EventVector& events, int delay, float value)\n{","void sfz::MidiState::insertEventInVector(EventVector& events, int delay, float value)\n{\n    eventsNeedFlush |= delay != 0;"),
        ("src/sfizz/MidiState.cpp","void sfz::MidiState::resetEventStates() noexcept\n{","void sfz::MidiState::resetEventStates() noexcept\n{\n    eventsNeedFlush = false;"),
];
    let mut text = None;
    for (path, from, to) in edits {
        if source.strip_prefix("vendor/audio/sfizz").ok() == Some(Path::new(path)) {
            let data = text.get_or_insert_with(|| String::from_utf8(bytes.clone()).unwrap());
            assert!(
                data.contains(from),
                "sfizz ownership adaptation no longer matches {}",
                source.display()
            );
            *data = data.replace(from, to);
        }
    }
    text.map(String::into_bytes).unwrap_or(bytes)
}
fn copy_tree(source: &Path, destination: &Path) {
    std::fs::create_dir_all(destination).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let path = entry.unwrap().path();
        let target = destination.join(path.file_name().unwrap());
        if path.is_dir() {
            copy_tree(&path, &target);
        } else {
            write_changed(&target, &adapted(&path));
        }
    }
}
fn run(command: &mut Command) {
    let result = command
        .output()
        .expect("sfizz requires CMake and a native C++ toolchain");
    if !result.status.success() {
        panic!(
            "sfizz build failed:\n{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
}
fn objects(directory: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            objects(&path, out);
        } else if matches!(path.extension().and_then(|s| s.to_str()), Some("o" | "obj")) {
            out.push(path);
        }
    }
}
pub fn build() {
    println!("cargo:rerun-if-env-changed=CMAKE");
    let output = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let source = output.join("sfizz-source");
    let binary = output.join("sfizz-build");
    copy_tree(Path::new("vendor/audio/sfizz"), &source);
    // Library-only build, with immutable resident samples. These are ownership
    // and deployment adaptations, not modifications to sfizz synthesis DSP.
    let cmake = std::env::var_os("CMAKE").unwrap_or_else(|| "cmake".into());
    let mut configure = Command::new(&cmake);
    configure
        .arg("-S")
        .arg(&source)
        .arg("-B")
        .arg(&binary)
        .args([
            "-DCMAKE_BUILD_TYPE=Release",
            "-DCMAKE_CXX_STANDARD=17",
            "-DCMAKE_POSITION_INDEPENDENT_CODE=ON",
            "-DCMAKE_POLICY_VERSION_MINIMUM=3.5",
            "-DENABLE_LTO=OFF",
            "-DSFIZZ_SHARED=OFF",
            "-DSFIZZ_JACK=OFF",
            "-DSFIZZ_RENDER=OFF",
            "-DSFIZZ_USE_SNDFILE=OFF",
            "-DSFIZZ_TESTS=OFF",
            "-DSFIZZ_GIT_SUBMODULE_CHECK=OFF",
        ]);
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        configure.arg(format!(
            "-DCMAKE_OSX_DEPLOYMENT_TARGET={}",
            std::env::var("MACOSX_DEPLOYMENT_TARGET").unwrap_or_else(|_| "11.0".into())
        ));
    }
    run(&mut configure);
    run(Command::new(&cmake)
        .arg("--build")
        .arg(&binary)
        .args(["--target", "sfizz_static", "--parallel"])
        .arg(std::env::var("NUM_JOBS").unwrap_or_else(|_| "2".into())));
    let mut files = Vec::new();
    objects(&binary, &mut files);
    files.sort();
    // One archive avoids platform-dependent static dependency ordering while
    // retaining only the objects built by the library target and its deps.
    let mut archive = cc::Build::new();
    archive.cpp(true).warnings(false).cargo_metadata(false);
    for file in files {
        archive.object(file);
    }
    archive.compile("crest_sfizz");
}
