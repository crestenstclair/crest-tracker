# Embedded audio source provenance

`sources.json` pins the selected repositories and nested dependencies. Each
component includes its original license and a `SHA256SUMS` manifest. The source
fetcher is `scripts/vendor_audio_sources.py`; normal builds use the vendored
files without fetching source. `scripts/audio_notices.py` collects redistribution
notices in `assets/licenses/UPSTREAM_AUDIO.txt`, included by the Tauri bundle.
The separately vendored RustySynth records its small local change in
`../rustysynth/PROVENANCE.md`.

Crest wraps complete upstream instruments and processors. Rust owns capability
metadata, asset import, per-note envelopes, prepared graph handoff, and typed
failures. C++ wrappers adapt processing boundaries and instance ownership. They
do not add synthesis or effect algorithms. Original source notices remain in
place; `build.rs` and `build_support` stage adaptations into Cargo's
output directory rather than altering the pinned source inputs.

## Build adaptations

- Mutable and DaisySP objects receive the zero-initialized storage their
  firmware initialization expects. Native rate/block requirements are adapted
  by r8brain with prepared buffers and contiguous circular-buffer spans; the
  source block calls and converter clocks are unchanged. A frozen scalar
  buffering witness checks output and clocks across wraps and resets. Per-instance PRNG state is selected around
  native calls. On Darwin, a pthread key allocated during preparation selects
  that state without C++ thread-local storage allocation on first render;
  other targets use constant-initialized pointer TLS. Mutable's staged PRNG
  accesses the selected instance, isolating preparation from rendering.
  Elements' modulation-offset metadata admits 0–0.5: its internal triangle
  LFO adds up to 0.5 before the upstream approximate cosine oscillator, so
  larger offsets can leave that oscillator's supported domain and diverge.
  The upstream signal algorithm is unchanged.
  The native adapter build disables GCC lifetime dead-store elimination to
  preserve zeroed storage before embedded DSP constructors; optimized instance
  independence and finite-output witnesses cover this initialization contract.
  Rings' staged modal resonator borrows the existing SVF histories into bounded
  stack arrays for SIMD, retains ordered summation, and caches pickup weights
  only while the interpolated position is unchanged. Filter histories return
  to their original owners after each block; exact-zero shortcuts never truncate
  a tail. Elements uses the same filter scratch, reuses identical center-pickup
  weights only when position is stationary, preserves each oscillator and
  bowed-feedback recurrence, and uses four partial sums for modal accumulation.
  Its unchanged coefficients are reused only after both alternating higher-mode
  update phases have observed them. The numerical witness compares against the
  retained upstream resonators without relaxing its tolerance, including rapid
  parameter changes with active histories.
- Airwindows/mda DSP is isolated behind a small SDK compatibility boundary;
  no VST2 SDK or foreign editor is distributed. mda ePiano's constructor-owned
  sample crossfades use private sample storage. ButterComp2's local static
  noise counters become instance fields. mda Piano's diagnostic print is
  excluded. The Dynamics adapter initializes and resets the three envelope
  histories omitted by its upstream constructor and suspend hook. These changes
  preserve the original signal algorithms.
- The selected DaisySP analog and synthetic snare ports failed finite-output
  checks at admitted frequencies. Their catalog roles use the original Mutable
  Plaits AnalogSnareDrum and SyntheticSnareDrum with explicit Mutable identities.
  The synthetic port diverges even at 2495.4 Hz with otherwise default controls;
  the original retains its own filter and resonance semantics.
  The other selected DaisySP algorithms remain the MIT main-library versions;
  the separate LGPL subtree is not included in the build.
  Staged SVF and modal implementations cache unchanged filter coefficients;
  AnalogBassDrum and SyntheticBassDrum compute unchanged tone/decay powers in
  their setters. The SVF caches its resonance-only damping bound even while
  the frequency changes. String and ModalVoice cache parameter-only excitation
  and damping calculations while preserving delay/filter history, interpolation
  phase, and every random draw. The staged SVF methods are inline so each caller
  can eliminate unused output calculations. Modal coefficient preparation is
  separate from sample processing; unchanged controls do not compare every
  mode's coefficients again each sample.
  The original calculations run on first use and after parameter changes. Gain
  and filter history still advance every sample. The native witness compares
  these paths against retained upstream sources across changes, reset, sample
  rates, and modal resolutions using the production optimization level.
- STK's staged sample-rate accessor selects the prepared instance's rate through
  Crest's native context; outside a context it retains the upstream global-rate
  API. The host never changes STK's global rate. Voices render directly at device
  rate using the original rate-dependent equations. Setup/retirement of the
  global observer list is serialized off callback. Raw waves are embedded and
  loaded during preparation. BandedWG reserves delay capacity for the lowest
  note and downward bend across its presets at the prepared rate; Shakers'
  selectable materials are warmed there. Mandolin's
  admitted damping range avoids the upstream invalid loop-gain endpoint.
  BandedWG's staged delay type privately wraps DelayL's original scalar tick
  and clears only the contiguous circular interval written since the previous
  clear. A cleared, unexcited plucked model returns exact zero until excitation;
  bowed state and nonzero tails always advance. Delay changes and cached
  outputs retain upstream semantics.
  Mesh2D fuses its junction/outgoing-wave passes because all outgoing writes
  target alternate buffers. Native witnesses require bit-identical samples
  across presets, pitch bends, delay wraps/growth, mesh dimensions, and resets.
  A separate witness compares interleaved instances at different rates with
  upstream global-rate rendering and requires bit-identical output.
- r8brain retains its upstream 24-bit filter design and double-precision DSP.
  Staged convolution starts at a prepared zero-padded block offset to spread
  independent voices' FFT work, retaining the original latency consumption and
  sample alignment. All-zero input and overlap skip the FFT. Native witnesses
  compare output, counts, latency, and reset against the retained upstream
  sources across integer and fractional rates. Generator adapters preserve the
  input converter clock while omitting unused channels and duplicate mono
  output conversion; stereo effects still convert both input/output channels.
- MSFA is Google's original Apache-2.0 DX7 core. The bundled electric-piano
  voice comes from its `synth_unit.cc`. Crest supplies validated SysEx framing,
  checksums, stable bank/voice identities, and preset selection. No GPL Dexed
  code or cartridge manager is used. Staged headers include their required
  integer/size declarations, and staged DX7 calls explicitly select MSFA's
  original min/max helpers to avoid libstdc++ overload ambiguity.
- NAM compiles the current full core and its selected dependencies with
  `EIGEN_MPL2_ONLY`. The bundled model is the upstream test LSTM, not a branded
  amp capture. FFTConvolver uses Ooura/AudioFFT, not FFTW. Its initial impulse
  is explicitly named as transparent. The adapter includes r8brain before
  FFTConvolver so shared SSE intrinsics retain global scope on x86.
  Signalsmith uses its portable backend.

## Maintained sfizz library build

Crest builds the BSD core as a static library, without JACK, libsndfile, plugin
clients, UI, tests, docs, or LTO. The compiler and macOS deployment target match
the surrounding native build. Selected decoder/dependency licenses and Faust
exceptions remain separate grants in the notices file.

The staged implementation forces resident loading, removes file-pool worker
creation and callback garbage-collection locking, guards absent worker joins,
and uses inline OSC message-index storage. A compatibility correction updates
atomic_queue syntax for current Clang.
MIDI block normalization skips its controller-table scan when every event is
already a current value at delay zero. Delayed events retain upstream ordering
and collapse semantics; a native witness covers controls, pitch, aftertouch,
and reset against the staged library.
The unused stage profiler reports disabled zero durations instead of reading
the clock at every voice stage. Other clock users remain upstream; Crest's
callback observations and external timing witnesses remain authoritative.

Import enables strict parsing, rejects unknown opcodes and discarded regions,
checks sample-decoder failures, and rejects invalid embedded samples. An
include callback validates containment before the parser reads each included
file. The bundler uses that parser, confines sample/include references to the
selected folder, enforces source budgets, and embeds samples. The WavPack
adapter checks a complete `wvpk` header before calling its raw decoder; invalid
bytes must not enter the raw Matroska decoding path.

The upstream repository is archived. These staged changes, their validation,
and future dependency maintenance are Crest's responsibility. Current host
voices own separate sfizz instances and duplicate sample residency.

## Verification scope

`scripts/check_native_audio.py` links the same native archives as Cargo and
exercises initialization, controls, model choices, MIDI, reset/release, variable
blocks, multiple sample rates, and interleaved instance independence. It tracks
C++ allocation/destruction and, on macOS, interposes common C heap and pthread
locking functions from a separate interposer library. Linux wraps those calls
from the linked native archives; calls internal to shared system libraries are
outside that link-time instrumentation. Counter self-tests must
pass, and first rendering runs on a fresh thread after control-thread
preparation. This is a regression witness for exercised paths, not proof
for every imported model, library, parameter combination, platform, or driver.
Rust tests cover capability ports, lifecycle correlation, graph activation,
parameter ownership, asset validation, persistence, and rendering.

Engine grants do not license arbitrary imported models, samples, presets, or
IRs. STK retains its upstream patent qualifications. Eigen MPL-covered source
is retained under `nam/Dependencies/eigen`; include the applicable source-access
information when distributing a binary. The notice generator packages the actual
source as `assets/licenses/EIGEN_SOURCE.tar.gz`, included by the Tauri bundle.
See the component notices for the
actual terms rather than assigning one license to the entire catalog.
