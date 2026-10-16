//! Spread independent converters' FFT work without changing their filter or latency.
use std::path::{Path, PathBuf};

pub fn stage(output: &Path) -> PathBuf {
    let destination = output.join("r8brain-source");
    copy_tree(Path::new("vendor/audio/r8brain"), &destination);
    destination
}

fn replace(text: &mut String, before: &str, after: &str, count: usize) {
    assert_eq!(
        text.matches(before).count(),
        count,
        "r8brain adaptation no longer matches: {before}"
    );
    *text = text.replace(before, after);
}

fn copy_tree(source: &Path, destination: &Path) {
    std::fs::create_dir_all(destination).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let source = entry.unwrap().path();
        let target = destination.join(source.file_name().unwrap());
        if source.is_dir() {
            copy_tree(&source, &target);
            continue;
        }
        let mut bytes = std::fs::read(&source).unwrap();
        if source.file_name().unwrap() == "CDSPResampler.h" {
            let mut text = String::from_utf8(bytes).unwrap().replace("\r\n", "\n");
            replace(&mut text, "const EDSPFilterPhaseResponse ReqPhase = fprLinearPhase )",
                "const EDSPFilterPhaseResponse ReqPhase = fprLinearPhase,\n\t\tconst unsigned int BlockPhase = 0 )", 1);
            for factors in ["num, den", "i, 1", "2, 1", "num, 1", "1, downf"] {
                replace(
                    &mut text,
                    &format!("), {factors}, LatencyFrac ));"),
                    &format!("), {factors}, LatencyFrac, true, BlockPhase ));"),
                    1,
                );
            }
            // Only the 24-bit front end is used by Crest. Other upstream front
            // ends keep the default phase and their original public signatures.
            let (before, tail) = text.split_once("class CDSPResampler24 :").unwrap();
            let mut tail = tail.to_owned();
            replace(&mut tail, "const int aMaxInLen, const double ReqTransBand = 2.0 )",
                "const int aMaxInLen, const double ReqTransBand = 2.0,\n\t\tconst unsigned int BlockPhase = 0 )", 1);
            replace(
                &mut tail,
                "180.15, fprLinearPhase )",
                "180.15, fprLinearPhase, BlockPhase )",
                1,
            );
            bytes = format!("{before}class CDSPResampler24 :{tail}").into_bytes();
        } else if source.file_name().unwrap() == "CDSPBlockConvolver.h" {
            let mut text = String::from_utf8(bytes).unwrap().replace("\r\n", "\n");
            replace(
                &mut text,
                "const bool aDoConsumeLatency = true )",
                "const bool aDoConsumeLatency = true,\n\t\tconst unsigned int aBlockPhase = 0 )",
                1,
            );
            replace(
                &mut text,
                "\t\tclear();",
                r###"        // Start within a zero-padded FFT block. Retain the original latency
        // consumption, so returned counts, signal alignment, and interpolator
        // history match phase zero. Whole input/output sample alignment also
        // preserves integer up/downsampling. No priming DSP runs on reset.
        const int alignment = UpFactor * DownFactor;
        const int positions = (InputLen - InputDelay - 1) / alignment + 1;
        InputPhase = DoConsumeLatency ? int((static_cast<unsigned long long>(
            aBlockPhase) * positions) >> 32) * alignment : 0;
        clear();"###,
                1,
            );
            replace(&mut text, "int InputDelay;", "int InputPhase; ///< Prepared zero-padding offset; not signal delay.\n\tint InputDelay;", 1);
            replace(&mut text,
                "memset( CurInput, 0, (size_t) InputDelay * sizeof( CurInput[ 0 ]));\n\n\t\tInDataLeft = InputLen - InputDelay;",
                "memset( CurInput, 0, (size_t) ((InputDelay + InputPhase) /\n            (UpShift >= 0 ? UpFactor : 1)) * sizeof( CurInput[ 0 ]));\n\n\t\tInDataLeft = InputLen - InputDelay - InputPhase;", 1);
            replace(
                &mut text,
                "\t\t\trealfft_t* CurInputFFT = (*fftin) -> forward( CurInput );",
                r###"            // Exact silence needs no FFT, including the retained overlap.
            // Nonzero input still follows the unmodified upstream algorithm.
            bool silent = true;
            for (int i = 0; i < ilu + PrevInputLen; ++i) {
                if (CurInput[i] != 0.0) { silent = false; break; }
            }
            if (silent) {
                memset(CurInput, 0, size_t(BlockLen2) * sizeof(double));
            } else {
            realfft_t* CurInputFFT = (*fftin) -> forward( CurInput );"###,
                1,
            );
            replace(
                &mut text,
                "\t\t\t(*fftout) -> inverse( CurInputFFT );",
                "\t\t\t(*fftout) -> inverse( CurInputFFT );\n            }",
                1,
            );
            bytes = text.into_bytes();
        }
        if std::fs::read(&target).ok().as_deref() != Some(&bytes) {
            std::fs::write(&target, bytes).unwrap();
        }
    }
}
