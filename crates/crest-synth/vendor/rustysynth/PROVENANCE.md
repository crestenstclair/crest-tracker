# RustySynth

Source: crates.io `rustysynth` 1.3.6; MIT, see LICENSE.txt.
Original archive SHA-256: `e3ca93af923df5fc03beddbf464242620fd24daa9e10f9ecd56eb9571eb7ba38`.
Repository: https://github.com/sinshu/rustysynth
Revision: `8cc11fc0b10422adb54107757f100d3d6ae0ef96`.

Crest changes positive prepared voice-count validation, removing the upstream
8–256 configuration restriction. Upstream already owns a dynamic voice vector.
All resources are prepared before activation and retired off callback.

Crest also removes the absolute `0.001` note-gain and mix-gain culling in
`voice.rs` and `synthesizer.rs`. Positive low-velocity notes must retain their
signal; only zero gain skips rendering/mixing. Upstream envelope-tail retirement
and MIDI velocity dynamics remain unchanged. The production adapter regression
`quiet_soundfont_notes_keep_their_signal_and_velocity_dynamics` covers the quiet
Cello, Steel Drums and Music Box notes present in Corridors of Time.
