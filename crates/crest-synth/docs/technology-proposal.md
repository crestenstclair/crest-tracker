# Proposed synthesizers, samplers, and effects

Research date: 2026-09-06. This report recommends upstream technologies; it is
not an implementation plan, product authority, or automatic context for future
tasks. Source, interfaces, and licensing were screened during research.

Implementation now follows this selection in the production capability registries.
See [DESIGN.md](../DESIGN.md) for current behavior and remaining integration
constraints, and [source provenance](../vendor/audio/PROVENANCE.md) for pinned
versions, dependency scope, and maintained adaptations. This report remains a
research decision input, not a second architecture document.

## Selection basis

Choose **self-contained instruments and effects whose native controls make
sense on a controller**. Braids, Plaits, a resonator, a drum voice, or a tape
delay has a coherent musical purpose. Preserve musical breadth by selecting
complete instruments and processors from coherent upstream collections. A
large workstation synth hidden behind macros does not meet this requirement,
but rejecting it should prompt a search for replacements for its musical roles.

For licensing, this report evaluates **embedding and distributing the selected
code without requiring Cresten's own application code to become open source**.
This is a selection assumption, not a declaration of Cresten's distribution
license. Commercial use alone is insufficient: GPL permits commercial use,
but its distribution obligations differ from MIT, BSD, Apache, MPL, and LGPL.

Use complete existing Rust, C++, or C DSP. Crest supplies wrappers, asset
import, preparation, and controller integration. Do not assemble oscillator
and filter primitives into a new Crest synthesizer. There are no arbitrary
caps on instruments, effects, Patches, tracks, parameters, or voices. Native
controls can occupy sensible pages; hardware resource budgets are configurable.

## Recommended portfolio

Make **Mutable Instruments the main synthesis and creative-effects family**,
with **DaisySP, mda, and Airwindows** as complementary collections. **STK** adds
another substantial instrument family, with the specific qualifications below.
Keep specialist libraries where these collections do not satisfy the need.

| Source family | Proposed selection | Musical contribution |
| --- | --- | --- |
| **Mutable Instruments** | Braids, current Plaits, Rings, Elements, Tides 2 audio modes, Peaks drums, Clouds, Warps | Analog-style, wavetable, additive, FM, phase-distortion, string-machine, speech, chiptune, physical modeling, percussion, granular, spectral, and cross-modulation sounds. |
| **DaisySP** | Analog and synthetic kick/snare voices, HiHat, StringVoice, ModalVoice, and the named MIT effects | Complete voices with direct controls, plus standard modulation and processing. Several voices are Mutable ports; these are alternative access paths to those algorithms. |
| **mda** | JX10, Piano, ePiano; Leslie, Dynamics, DeEsser, DubDelay, and TalkBox | A conventional subtractive synth, self-contained keyboards, rotary speaker, conventional dynamics, and additional character effects. |
| **Airwindows** | The curated standard-effects selection below | Broad delay, reverb, modulation, EQ, dynamics, saturation, and lo-fi coverage. |
| **Synthesis ToolKit (STK)** | Shakers, ModalBar, VoicForm; wind/string/brass models and simple FM keyboards/organ as qualified additions | Dedicated instruments with native musical controls, not a new modular synthesis environment. Copyright license permits embedding; selected models carry upstream patent notices. |
| **Library instruments** | Full RustySynth; sfizz conditional on maintenance ownership; MSFA for DX7 SysEx, with Plaits six-op as a comparison candidate | SF2, WAV/SFZ, and DX7 library playback without building large editors. TinySoundFont remains the compact SF2 alternative. |
| **Specialists** | NeuralAmpModelerCore, FFTConvolver, Signalsmith Stretch | Neural captures, cabinet/space IRs, and pitch/time processing. |

### Replacements for the musical coverage

These replace musical roles, not proprietary preset formats or every behavior
of the removed application. The broader palette comes from multiple complete,
understandable instruments rather than another workstation synth.

| Earlier candidate / role | Proposed replacement | Remaining distinction |
| --- | --- | --- |
| Surge/Vital/Helm: subtractive and wavetable sounds | Plaits virtual-analog/VCF, waveshaping, and wavetable models; mda JX10 for conventional oscillator/filter/envelope editing | No equivalent of their deep modulation editors or preset formats is proposed. |
| ZynAddSubFX: additive sounds, pads, evolving textures | Plaits additive, chord, string-machine, and swarm models; Elements/Rings and Clouds for resonant and spectral textures | These cover the musical territory without claiming an exact PADsynth engine or Zyn preset import. |
| setBfree: organ and rotary speaker | STK BeeThree for a simple organ timbre; mda Leslie for rotary processing; SF2 organ libraries remain available | BeeThree is an FM organ, not a full tonewheel/drawbar/key-click model. A comparable self-contained permissive tonewheel engine has not been established by this search. |
| libADLMIDI: retro/chiptune/FM | Plaits chiptune and FM models; MSFA for DX7 libraries | OPL emulation and WOPL bank compatibility remain unfilled; DX7 and chiptune synthesis do not reproduce that format. |
| Full Dexed: imported DX7 voices | Apache MSFA; compare Mutable's MIT six-op voice where preset behavior permits | Neither is declared identical to Dexed Mk I. Import validation remains adapter work. |
| Shortcircuit: richer sampling | sfizz for WAV/SFZ playback; Clouds for granular/stretch/freeze treatments | sfizz maintenance and migration compatibility remain conditional. A large sample-mapping editor is not required. |
| sst-effects/Dragonfly: broad effects suite | Airwindows, DaisySP, Mutable effects, mda, and FFTConvolver | Concrete choices below cover standard effects; no dependency on the removed synth is necessary. |

## Embedding license gate

“Eligible” below means the stated code scope supports the evaluation basis
with the listed obligations. It does not cover arbitrary optional dependencies,
bundled content, or plugin SDKs. A Rust wrapper does not change upstream terms.

| Selected source / language | License and scope | Embedding decision and obligations |
| --- | --- | --- |
| [Mutable DSP][mutable] / C++ | Relevant STM32F DSP and resources are MIT. The repository also contains GPLv3 AVR code; hardware designs have separate terms. | **Eligible for the selected DSP subset.** Retain source notices; include only required DSP/resources and MIT stmlib dependencies. Exclude firmware utilities and unrelated GPL tools. |
| [mda][mda-jx10-dsp] / C++ | MIT notices in the inspected processors, controllers, and Piano/ePiano data headers. The current public SDK also carries MIT terms. | **Eligible for the selected source.** Preserve Paul Kellett and applicable Steinberg notices. Reuse DSP and parameter mappings; no plugin host or foreign editor is required. |
| [STK][stk-license] / C++ | Permissive STK license explicitly permits modification, sublicensing, and sale; returning changes is a nonbinding request. | **Copyright license supports embedding.** Retain notices. Some model headers explicitly mention patents; that separate qualification is unresolved here, so those models are not presented as cleared for distribution. |
| [MSFA][msfa-license] / C++ | Apache-2.0 native synthesis core. | **Eligible.** Include the license, preserve notices and any applicable NOTICE, and identify modified files. Do not import GPL Dexed engine or cartridge code. |
| [DaisySP][daisy-license] / C++ | MIT main library; separate DaisySP-LGPL subtree. | **Eligible for the named MIT processors.** Retain notices and omit the LGPL subtree. Older examples referencing relocated ReverbSc or Compressor do not establish MIT eligibility. |
| [RustySynth][rustysynth-license] / Rust | MIT; upstream reports no external dependencies. | **Eligible.** Include copyright and license notices. Use the complete upstream renderer. |
| [TinySoundFont][tsf-license] / C | MIT, including SFZero attribution; C standard library dependency. | **Eligible alternative.** Retain all applicable notices. Its scope is SF2, not SFZ. |
| [sfizz][sfizz-license] / C++ with C API | BSD-2-Clause core; mixed dependencies. Select the library with dr_libs decoding, without optional libsndfile, JACK, or plugin UI. | **Eligible under the documented library dependency terms.** Retain BSD/MIT/Apache and other dependency notices, including generated DSP exceptions. Maintenance remains conditional; see below. |
| [Airwindows][airwindows-license] / C++ | MIT processor source. | **Eligible.** Retain notices. Adapt the DSP boundary without distributing the VST2 SDK or upstream editor wrapper. |
| [NeuralAmpModelerCore][nam-license] / C++ | MIT core, MIT AudioDSPTools and nlohmann JSON; Eigen is primarily MPL-2.0 with additional permissive notices. | **Eligible with Eigen's source obligations.** Retain MIT and dependency notices; make the covered Eigen source, including modifications, available as MPL requires. This is not an entirely MIT dependency stack. |
| [FFTConvolver][fft-source] / C++ | MIT convolver and AudioFFT; use the included Ooura implementation. | **Eligible in this scope.** Retain notices. Exclude the optional GPL FFTW3 backend. |
| [Signalsmith Stretch][stretch-license] / C++ | MIT, including the required Signalsmith Linear dependency. | **Eligible with the portable built-in backend.** Retain both projects' notices; optional FFT backends are outside this selection. |

MIT/BSD generally require preservation of copyright, license, and disclaimer
notices; BSD-3-Clause also restricts endorsement. Apache adds its notice and
modification requirements and an express patent grant. MPL's source obligations
apply to covered files, not unrelated application files; it permits a larger proprietary
application, including static linking. See [Mozilla's MPL FAQ][mpl-faq].
LGPL can also support proprietary embedding, but requires compliance with its
library source and user replacement/relinking provisions. It is not a blanket
ban; the simpler permissive options are preferred here.

For sfizz, upstream [documents the dependency set][sfizz-readme]: dr_libs uses
MIT No Attribution; Abseil uses Apache-2.0; atomic_queue, tuning, and pugixml use
MIT; filesystem, KISS FFT, cephes, and cpuid use BSD-3-Clause; hiir uses WTFPL-2.0.
Generated Faust components carry STK and LGPL-with-permissive-exception terms.
Preserve those actual generated-code notices and exceptions with the chosen
revision; do not relabel the combined library BSD-only.

Engine licenses do not grant rights to redistribute `.nam` models, SysEx banks,
SoundFonts, SFZ/sample libraries, IRs, or trademarks. Import capability and
factory content are separate decisions. Any bundled content needs explicit
redistribution permission. Record selected revisions and their notices when
vendoring so a later dependency change cannot silently change eligibility.

## Synthesizers and controller interaction

The controls below describe native musical concepts, not a universal knob
count. Repeated controls or secondary pages are fine when the instrument
remains understandable. Imported DX7 libraries are intentionally preset-first.

### Mutable Instruments as the foundation

| Instrument | Proposed controller interaction | Reuse boundary / contribution |
| --- | --- | --- |
| **Braids** | Model, timbre, color, pitch/performance controls. | Retain the existing upstream DSP wrapper and its distinct palette. |
| **[Plaits][plaits-engines]** | Model, harmonics, timbre, morph; native LPG/decay controls where applicable. | Include the current upstream model set, not just the original firmware's familiar models. Reuse the complete voice. |
| **[Rings][rings-source]** | Model, structure, brightness, damping, position. | Resonant strings, modal bodies, and related native models; include instrument and audio-resonator roles where routing supports them. |
| **Elements** | Exciter amount/timbre and resonator geometry/material, brightness, damping, position, grouped into native exciter/resonator pages. | Include as a distinct physical-model instrument, with its complete exciter and resonator rather than a newly assembled voice. |
| **[Tides 2][tides-source]** | Frequency, slope, shape, smoothness, shift, and native output mode. | Add the module's audio-range slope synthesis and related-output behavior. Its control-rate utility modes do not imply a new modular-control system. |
| **[Peaks drums][peaks-source]** | Kick: pitch, punch, tone, decay. Snare: pitch, tone, snappiness, decay. FM drum: frequency, FM amount, decay, noise/drive. | Add the complete upstream drum models, not the module's unrelated sequencer and utility functions. |

Plaits is itself a substantial collection. Its [voice registration][plaits-engines]
provides concrete replacements for the larger synths:

| Plaits model family | Musical coverage |
| --- | --- |
| Virtual analog, virtual analog with VCF, waveshaping | Conventional leads/basses and richer oscillator timbres. |
| Wavetable, wave terrain, additive | Scanned textures, harmonic spectra, and evolving tones. |
| Phase distortion, FM, six-operator FM | Digital basses, bells, keys, and metallic sounds. |
| String machine, chords, swarm | Ensemble, chordal, and layered pad textures. |
| Speech, granular/formant, chiptune | Vocal/formant colors and retro digital sounds. |
| String, modal, particle, noise, kick, snare, hi-hat | Physical-model sounds, percussion, and noise textures. |

These are upstream models, not new Crest algorithms or a requirement to create
a separate registry entry for every mode. Mode-specific labels should explain
what harmonics, timbre, and morph actually do.

Mutable **Clouds** also includes granular, time-stretch, looping-delay, and
spectral modes. **Warps** supplies wavefolding, ring modulation, cross-modulation,
and vocoding. Both belong in the selected family, with their native mode and
parameter surfaces. Warps has a built-in carrier; external-carrier modes need
explicit independent input routing. Stereo left/right is not automatically a
pair of independently routed sources.

Select actual MIT DSP files and the required [stmlib subset][stmlib-license].
Hardware rates, block sizes, initialization, and memory assumptions need
adaptation; they do not establish Crest capacity limits. Names here identify
research sources; follow upstream trademark/derivative naming guidance for
product branding. Analog schematics and unrelated sequencers/utilities are
not automatically digital instruments or effects.

### Physical modeling: strongest additions

**Prioritize Elements and Rings**, then expose Plaits/DaisySP's existing string
and modal voices as focused instruments where useful. Their controls describe
how an object is excited and how it resonates, which suits controller editing.
The goal is expressive musical models; source inspection does not establish
acoustic realism or audio quality.

| Candidate | What makes it interesting | Proposed treatment |
| --- | --- | --- |
| **[Elements][elements-source]** | Built-in bow, blow, and strike exciters feed a resonant body. Geometry, brightness, damping, and excitation position support mallet-like attacks, metallic ringing, breathy textures, and sustained bowed sounds. | First choice for a distinctive physical-model instrument. Group the [native controls][elements-patch] into excitation, body, and space pages. Selected DSP is MIT. |
| **Rings** | Modal resonators, sympathetic strings, and string models; internal excitation or incoming audio can drive the body. | Include as both an instrument and an audio-resonator effect when routing supports it. A drum loop driving a resonant body is an especially useful creative application. Selected DSP is MIT. |
| **Plaits / DaisySP StringVoice and ModalVoice** | Complete plucked-string and mallet-excited resonator voices with structure, brightness, damping, and accent controls. | Compact dedicated instruments using existing complete algorithms. DaisySP's versions derive from Mutable; avoid presenting them as unrelated synthesis technologies. Selected DSP is MIT. |
| **STK Shakers / ModalBar** | Physically informed rattles, bells, bamboo, and collision sounds; struck bars with hardness and strike-position controls. | Add percussion beyond kick/snare/hat, using the complete upstream instruments and STK license terms below. |
| **[STK BandedWG][stk-bandedwg]** | Bowed bars, glass harmonica, and Tibetan bowl presets; bow pressure, motion, velocity, and striking mode. | A particularly interesting expressive addition. Its header marks the strike-position control as unimplemented; do not promise it. Preserve STK/author notices and the suite's stated qualifications. |
| **STK Bowed / Clarinet / Flute / Brass / Mandolin** | Bow friction, breath/reed behavior, lip tension, and coupled strings provide instrument-specific performance controls. | Broader acoustic-model palette; retain the explicit patent qualifications below. |
| **[STK Mesh2D][stk-mesh]** | A two-dimensional waveguide mesh with adjustable dimensions, decay, and excitation position; accepts incoming audio as well as impulses. | Experimental resonant-surface/percussion candidate. Its native note-on ignores pitch and note-off is ignored, so it is not a ready-made pitched keyboard instrument. Its header also carries a patent notice. |

These are complete upstream sound generators or processors. Reuse their native
excitation and resonance behavior rather than designing new Crest models.
Elements and Rings expose their own hardware-oriented voice managers; allocate
prepared instances or adapt voice ownership to Crest's configurable resources
without importing the modules' hardware polyphony limits. Audio-excited modes
need explicit graph input routes; live microphone capture is a separate feature.

### Complementary collections: mda and DaisySP

| Instrument | Proposed controller interaction | Recommendation |
| --- | --- | --- |
| **[mda JX10][mda-jx10-controls]** | Oscillator mix/tuning, glide, filter frequency/resonance, filter and amplitude envelopes, LFO/vibrato, noise. | Add for conventional subtractive editing. Its native sections make sensible controller pages without a modulation matrix. |
| **[mda ePiano][mda-epiano-controls]** | Decay/release, hardness, treble, modulation/rate, velocity response, stereo width, tuning, overdrive. | Add a dedicated electric piano with upstream playback and embedded sample data. |
| **[mda Piano][mda-piano-controls]** | Decay/release, hardness, muffling, velocity response, stereo width, tuning. | Add a compact acoustic-piano option alongside richer imported SF2/SFZ libraries. It is not presented as a modern concert-piano library. |
| **[DaisySP drums][daisy-drums]** | Tune, accent, tone, decay; native kick FM/dirtiness, snare snappiness, and hat noisiness controls as applicable. | Include analog and synthetic kick/snare variants and HiHat. Several derive from Mutable; choose the port for its integration fit, not as unrelated new DSP. |
| **[DaisySP StringVoice][daisy-string] and [ModalVoice][daisy-modal]** | Structure, brightness, damping, accent, trigger/sustain, frequency. | Useful complete voices for directly accessible plucked-string and mallet instruments. Their Plaits ancestry means these can be dedicated presentations of existing algorithms. |

The mda sources live in Steinberg's [public SDK examples][mda-suite], but their
musical interfaces are compact instruments and individual effects. Reuse the
existing DSP under Crest's capability ports. A VST host, plugin-browser
workflow, or replacement of Crest's controller UI is unnecessary. mda's
[Piano][mda-piano-data] and [ePiano][mda-epiano-data] data headers carry the same
MIT grant; preserve those notices with the selected data. These are older
algorithms whose sound and behavior still need listening comparisons.

Original mda voice pools and preset counts are implementation details. Preserve
the algorithms while adapting instance/voice storage to configurable resources;
do not copy those constants into Crest's product contract. Current per-note
integration and prepared rendering are documented in `DESIGN.md`.

### STK: a substantial additional instrument family

[The Synthesis ToolKit][stk-readme] contains complete instruments as well as
building blocks. Select its instrument classes; Crest should not use the
building blocks to design its own synths.

| Complete upstream instrument | Controller-ready native concepts | Contribution |
| --- | --- | --- |
| **[Shakers][stk-shakers]** | Instrument, shake energy, decay, object count, resonance. | Maraca, cabasa, tambourine, bells, bamboo, guiro, and environmental percussion. |
| **[ModalBar][stk-modalbar]** | Material/preset, stick hardness/position, direct strike mix, vibrato. | Marimba, vibraphone, agogo, and other struck-bar timbres. |
| **[BandedWG][stk-bandedwg]** | Preset, bow pressure/motion/velocity, striking mode. | Bowed bars, glass harmonica, and Tibetan bowls; the documented strike-position control is unimplemented. |
| **[Mesh2D][stk-mesh]** | Mesh dimensions, decay, excitation position. | Experimental waveguide surface; native note-on ignores pitch and note-off is ignored. |
| **[VoicForm][stk-voicform]** | Phoneme, voiced/noise mix, vibrato, spectral tilt. | A dedicated vowel/formant instrument. |
| **[Clarinet][stk-clarinet] / [Flute][stk-flute]** | Reed stiffness or jet delay, breath/noise, vibrato. | Expressive wind-model voices. |
| **[Brass][stk-brass]** | Lip tension, slide length, vibrato, breath/level. | A dedicated brass waveguide model. |
| **[Bowed][stk-bowed] / [Mandolin][stk-mandolin]** | Bow pressure/position/velocity; or body size, pluck position, sustain, detune. | Bowed and plucked strings with native physical controls. |
| **[BeeThree][stk-beethree]** | Native operator gains, vibrato, level/envelope performance controls. | A compact FM organ; no claim of full drawbar/tonewheel behavior. |
| **[Rhodey][stk-rhodey] / [Wurley][stk-wurley]** | Native FM index, pair balance, modulation, envelope performance controls. | Synthesized electric-piano colors, distinct from mda's sampled ePiano. |

The [STK license][stk-license] explicitly allows proprietary embedding and
sale with notices; its request to return modifications is explicitly nonbinding.
However, the README and several waveguide/FM model headers retain patent
notices. This source review does not establish which claims remain applicable
or expired. The named models remain qualified candidates on that issue; do not
mistake permission to copy code for resolved patent status. Prioritize the
Mutable/mda/DaisySP core while retaining this concrete expansion option.

Use the instrument/DSP classes rather than STK's audio/MIDI drivers, GUI,
networking, or SKINI layer. Prepare required rawwave assets off callback. STK
can perform file access and throw exceptions during setup, and its error paths
can format/log; valid prepared render/control paths need explicit proof.
Upstream still describes the toolkit as alpha despite its long history, so
age is not a quality or production-readiness guarantee.

### DX7 libraries: MSFA instead of full Dexed

Mutable's current [six-op engine][plaits-sixop] and [DX7 patch unpacker][plaits-fm-patch]
provide an additional MIT candidate inside the preferred suite. Evaluate it
alongside MSFA before choosing a second FM dependency. It is not yet a turnkey
SysEx library instrument: the enclosing hardware engine has its own bank/voice
scheduler, and the [voice][plaits-fm-voice] applies brightness/envelope macros
that can alter preset behavior. Do not claim unchanged DX7 envelopes or exact
Dexed fidelity merely because it reads the patch layout. Reuse its native
voice with prepared host-managed instances if selected; hardware scheduling
and bank storage do not define Crest library or polyphony limits.

Full Dexed is GPLv3. Its Apache-licensed MSFA ancestry does not make Dexed's
Mk I engine, JUCE integration, or cartridge handling Apache-licensed. Use the
original [Google MSFA native core][msfa] for this proposal. It already contains
DX7 note synthesis, packed-patch unpacking, and bank SysEx handling.

The user workflow should import `.syx` libraries, browse authored names, select
a preset, and play. Support standard DX7 single-voice and bank messages,
including multiple supported messages in a file. Preserve stable file/bank/
preset identity even when names repeat. These are format adapters around
upstream synthesis, not new FM DSP.

The existing [MSFA MIDI parser][msfa-midi] contains a checksum-validation TODO;
it is not a ready-to-trust file importer. The adapter must validate framing,
message type, lengths, seven-bit payloads, and checksums, reject malformed or
unsupported data visibly, and handle the requested single-voice/multi-message
cases. Do not promise every Yamaha SysEx dialect or imply file import includes
live hardware SysEx transfer.

**Tradeoff:** Google archived this project. It is an older source baseline that
requires maintenance ownership, not an actively supported library. MSFA is also
a different rendering choice from Dexed Mk I; identical patch data does not
establish identical sound.
Compare representative libraries against the chosen upstream renderer before
accepting the integration. If exact Dexed Mk I sound is mandatory, the current
permissive selection does not satisfy it.

## Sampler recommendation

| Capability | Backend before catalog integration | Selected direction |
| --- | --- | --- |
| Sample | `hound` decodes WAV; Crest owns playback, interpolation, transposition, loop handling, and envelopes. | Evaluate sfizz as the complete playback backend for both simple imported samples and SFZ libraries. A new decoder alone would leave the custom sampler DSP intact. |
| SoundFont | `rustysynth` parses SF2; Crest renders prepared regions with its own numeric voice engine. | First evaluate the full upstream RustySynth `Synthesizer`, already available through the installed dependency. |
| Braids / Chorus | Vendored Mutable synthesis / Rings chorus behind wrappers. | Preserve these existing examples of upstream DSP reuse. |
| Reverb / Delay | Crest-owned processors. | Replace through the effect selections below; no replacement is implemented by this report. |

Local evidence: [Sample](../src/adapter/sample_preparer.rs),
[WAV decoder](../src/adapter/wav_sample_decoder.rs),
[SoundFont renderer](../src/adapter/soundfont_voice_engine.rs),
[Reverb](../src/adapter/reverb_preparer.rs),
[Delay](../src/adapter/delay_preparer.rs),
[Braids provenance](../vendor/braids/PROVENANCE.md), and
[Chorus provenance](../vendor/chorus/PROVENANCE.md).

**SoundFont:** prefer [RustySynth][rustysynth] for a file/bank/preset instrument.
[TinySoundFont][tsf] is a compact C alternative if the Rust renderer fails the
required compatibility or integration checks. Neither is a general SFZ sampler.
FluidSynth and OxiSynth are LGPL alternatives, but introduce additional license
obligations without an established need to replace the installed MIT library.

**Sample/SFZ:** [sfizz][sfizz-readme] provides a C API, existing sample playback,
streaming, and support for authored zones and velocity layers. The user can
select a sample or library and edit supported tuning, playback, loop, and
performance controls without a zone-mapping workstation. A one-region SFZ
representation of an imported WAV would be metadata integration; sfizz would
still own the audio rendering. Exact compatibility with current preview,
landmarks, loop crossfades, and saved assets is not yet established.

Its GitHub repository is [archived][sfizz-status], with last push reported as
2025-03-17. Choose it only with explicit maintenance ownership. That is a
material selection condition, not evidence that an unsuitable large sampler
should be substituted. This report does not establish an equally compact,
actively maintained, permissively licensed drop-in replacement.

Use the [opcode support matrix][sfizz-opcodes] to define accepted libraries.
Import referenced samples and included definitions with safe path resolution;
missing assets or unsupported behavior need visible errors. Do not promise
Kontakt or other proprietary formats. Preserve existing asset identity and
saved behavior through any eventual compatible transition.

The [sfizz API][sfizz-api] documents preload changes that lock and require
rendering to stop. Preparation must occur off callback. Full preload can reduce
streaming needs, but does not prove allocation-free, lock-free rendering.
RustySynth and TinySoundFont likewise need native voice/envelope and callback
compatibility checks; none is declared a proven drop-in replacement here.

## Effects: standard coverage from complete upstream processors

Use individual processors as Crest capabilities, with controller labels that
preserve native parameter meanings. Airwindows supplies MIT C++ DSP; DaisySP
supplies the named MIT C++ processors; mda and Mutable add further complete
processors. The list covers standard roles before
adding specialty processors; it is not a mandatory launch inventory.

| Effect role | Selected upstream processor | Native controls / intended controller surface |
| --- | --- | --- |
| Clean echo | [Airwindows PurestEcho][aw-PurestEcho] | Time and four tap levels. This is a four-tap echo, not a feedback-delay algorithm. |
| Stereo feedback delay | [Airwindows Doublelay][aw-Doublelay] | Left/right delay, feedback, detune, dry/wet. Do not label it ping-pong without verifying that routing. |
| Tape delay / echo | [Airwindows TapeDelay2][aw-TapeDelay2] | Time, regeneration, filter frequency/resonance, flutter, dry/wet. |
| Simple algorithmic reverb | [Airwindows Reverb][aw-Reverb] | Size (`Big`) and wet amount; a simplified MatrixVerb design. |
| Plate reverb | [Airwindows kPlateD][aw-kPlateD] | Input pad, damping, low cut, predelay, wetness. |
| Cabinet / room / hall IR | [FFTConvolver][fft-source] | IR file/preset selection plus host output/mix controls; decay and coloration come from the IR. |
| Chorus | Existing Mutable Rings chorus; [Airwindows StereoChorus][aw-StereoChorus] as an alternative | Retain the existing capability; StereoChorus offers speed and depth. |
| Flanger | [DaisySP Flanger][daisy-flanger] | Delay, LFO rate/depth, feedback. |
| Phaser | [DaisySP Phaser][daisy-phaser] | Pole count, center frequency, LFO rate/depth, feedback. |
| Tremolo | [DaisySP Tremolo][daisy-tremolo] | Rate, waveform, depth. |
| Vibrato | [Airwindows Vibrato][aw-Vibrato] | Speed, depth, modulation speed/depth, inverse/wet. |
| Auto-pan | [Airwindows AutoPan][aw-AutoPan] | Rate, phase, width, dry/wet. |
| Auto-wah | [DaisySP Autowah][daisy-autowah] | Wah amount, dry/wet, level. |
| Compression | [Airwindows ButterComp2][aw-ButterComp2] | Compression, output, dry/wet. A direct compressor surface without invented attack/release controls. |
| Limiting | [DaisySP Limiter][daisy-limiter] | Input/pre-gain. Do not advertise true-peak mastering protection or stereo linking without evidence. |
| Noise gate | [Airwindows SoftGate][aw-SoftGate] | Threshold, darkening, silence. Preserve this processor's behavior rather than inventing generic gate controls. |
| Bass/treble EQ | [Airwindows Baxandall2][aw-Baxandall2] | Bass and treble. |
| Parametric EQ | [Airwindows Parametric][aw-Parametric] | Treble, high-mid, and low-mid frequency/gain/resonance groups, plus dry/wet. Repeated band pages suit a controller. |
| Filter | [Airwindows Biquad2][aw-Biquad2] | Type, frequency, Q, output, inverse/wet. |
| High-/low-pass coloration | [Airwindows Capacitor2][aw-Capacitor2] | Low-pass, high-pass, nonlinearity, dry/wet. |
| Saturation / distortion | [Airwindows Density2][aw-Density2] | Density, high-pass, output, dry/wet. |
| Tape coloration | [Airwindows ToTape6][aw-ToTape6] | Input, soften, head bump, flutter, output, dry/wet. Chosen for its direct controls, not as the newest tape algorithm. |
| Amp / pedal / preamp capture | [NeuralAmpModelerCore][nam] | Model selection, input/output gain, and any controls actually supported by that model. |
| Lo-fi / digital degradation | [Airwindows DeRez3][aw-DeRez3] | Rate, resolution, dry/wet. Do not invent exact bit-depth units for normalized controls. |
| Pitch shift | [Signalsmith Stretch][stretch] | Transpose and supported formant controls; account for processing latency. |
| Granular / freeze | [Mutable Clouds][clouds-source] | Position, size, density, texture, pitch, freeze, and native feedback/mix controls on suitable pages. |
| Stretch / looping delay / spectral | [Mutable Clouds][clouds-source] native modes | Mode, position, size, pitch, texture, feedback, freeze, and mix; label the controls according to the chosen mode. |
| Wavefold / ring modulation / vocoder | [Mutable Warps][warps-source] | Algorithm, timbre, input drives, carrier shape/pitch. Built-in carrier supports self-contained use; external carriers need an explicit route. |
| Rotary speaker | [mda Leslie][mda-leslie-controls] | Stop/slow/fast mode, speed, low/high width and throb, high depth, crossover, output. |
| Conventional dynamics | [mda Dynamics][mda-dynamics-controls] | Threshold, ratio, attack, release, output, limiter, gate, mix. A native alternative when ButterComp2's simplified controls are insufficient. |
| De-essing | [mda DeEsser][mda-deesser-controls] | Threshold, frequency, high-frequency drive. |
| Modulated dub delay | [mda DubDelay][mda-dubdelay-controls] | Time, feedback, feedback tone, LFO depth/rate, mix, output. An additional flavor alongside TapeDelay2. |
| Talkbox / vocal spectral shaping | [mda TalkBox][mda-talkbox-controls] | Wet, dry, carrier selection, quality. Requires separate carrier and modulator routing; a live microphone workflow is not implied. |

Airwindows parameter names were checked in the linked processor sources;
upstream [descriptions][aw-descriptions] and [catalog][aw-catalog] explain their
musical roles. Native dry/wet and inverse/wet conventions differ. Verify send
and parallel-path behavior per processor rather than assuming every effect
provides a wet-only output. Tempo divisions can map to native delay time where
supported; smooth tempo automation and tail behavior are not yet proven.

Rings also belongs in the audio-resonator selection. Warps and TalkBox require
truthful input routing for external carriers. A dedicated dynamic spring-reverb
processor remains unselected; a spring IR does not reproduce all dynamic spring
behavior. mda Leslie now supplies a concrete rotary-speaker candidate.

### NAM and convolution have complementary roles

NAM is central for nonlinear amp, pedal, and preamp captures. An amp/cab capture
can include cabinet coloration, but arbitrary reverbs are not a general NAM
compatibility promise. The official [NAM plugin][nam-plugin] itself uses a
separate IR processor. Use FFTConvolver for cabinet IRs and recorded spaces;
do not silently double a captured cabinet with another cabinet IR.

Use the DSP core, not the complete plugin/editor. Respect supported model
architecture, sample-rate and calibration metadata, initialization, and channel
layout. Missing metadata needs an explicit policy; do not silently assume
48 kHz. Required resampling must also use existing upstream DSP. Model loading
and warming belong off callback, and model complexity determines measured CPU
cost. Eigen's [license at the inspected submodule revision][eigen-license]
requires the MPL handling described above; [AudioDSPTools][adt-license] and
[nlohmann JSON][nam-json] carry their own MIT notices.

Convolution and Signalsmith processing introduce routing and latency questions.
Prepare IR/FFT state off callback, select supported channel layouts, and
account for reported latency on dry/send paths. Signalsmith Stretch depends on
[Signalsmith Linear][linear-license]; choose its portable implementation.
Stretch is a processor, not a complete sampler or a substitute for sfizz.

## Evaluation criteria

The recommended core has source-level musical, controller, and embedding
license support. STK model patent notices and sfizz maintenance remain explicit
qualifications. Runtime acceptance uses these criteria; current evidence and
remaining constraints belong in DESIGN.md:

- Use capability schemas and existing semantic actions through `AppState::apply`;
  keep asset/preset identities stable and preparation failures explicit.
- Preserve native preset envelopes and Crest's independent-note contract.
  A summed stereo synth output does not by itself support per-note envelopes.
- Prepare and retire resources off callback; activate prepared graphs at block
  boundaries. Check native C/C++ as well as Rust for allocation, destruction,
  locking, I/O, exceptions, and lazy initialization in admitted render paths.
- Compare representative presets, SF2/SFZ libraries, models, and IRs against
  upstream output. Audition standard effects and verify their real control
  ranges, levels, tails, rate handling, and supported mono/stereo operation.
- Measure release builds on modern target hardware with configurable resource
  budgets. Account for native rate/block adaptation and latency; do not turn
  upstream defaults or old Crest capacities into product limits.

[mutable]: https://github.com/pichenettes/eurorack/blob/master/README.md
[stmlib-license]: https://github.com/pichenettes/stmlib/blob/master/LICENSE
[clouds-source]: https://github.com/pichenettes/eurorack/blob/master/clouds/dsp/granular_processor.h
[msfa]: https://github.com/google/music-synthesizer-for-android
[msfa-license]: https://github.com/google/music-synthesizer-for-android/blob/master/COPYING
[msfa-midi]: https://github.com/google/music-synthesizer-for-android/blob/master/app/src/main/jni/synth_unit.cc
[daisy-license]: https://github.com/electro-smith/DaisySP/blob/master/LICENSE
[daisy-modal]: https://github.com/electro-smith/DaisySP/blob/master/Source/PhysicalModeling/modalvoice.h
[daisy-flanger]: https://github.com/electro-smith/DaisySP/blob/master/Source/Effects/flanger.h
[daisy-phaser]: https://github.com/electro-smith/DaisySP/blob/master/Source/Effects/phaser.h
[daisy-tremolo]: https://github.com/electro-smith/DaisySP/blob/master/Source/Effects/tremolo.h
[daisy-autowah]: https://github.com/electro-smith/DaisySP/blob/master/Source/Effects/autowah.h
[daisy-limiter]: https://github.com/electro-smith/DaisySP/blob/master/Source/Dynamics/limiter.h
[rustysynth]: https://github.com/sinshu/rustysynth/blob/main/README.md
[rustysynth-license]: https://github.com/sinshu/rustysynth/blob/main/LICENSE.txt
[tsf]: https://github.com/schellingb/TinySoundFont/blob/main/README.md
[tsf-license]: https://github.com/schellingb/TinySoundFont/blob/main/LICENSE
[sfizz-license]: https://github.com/sfztools/sfizz/blob/develop/LICENSE
[sfizz-readme]: https://github.com/sfztools/sfizz/blob/develop/README.md
[sfizz-status]: https://api.github.com/repos/sfztools/sfizz
[sfizz-api]: https://github.com/sfztools/sfizz/blob/develop/src/sfizz.h
[sfizz-opcodes]: https://sfz.tools/sfizz/development/status/opcodes/
[airwindows-license]: https://github.com/airwindows/airwindows/blob/master/LICENSE
[aw-descriptions]: https://github.com/airwindows/airwindows/blob/master/what.txt
[aw-catalog]: https://github.com/airwindows/airwindows/blob/master/Airwindopedia.txt
[aw-PurestEcho]: https://github.com/airwindows/airwindows/blob/master/plugins/WinVST/PurestEcho/PurestEcho.cpp
[aw-Doublelay]: https://github.com/airwindows/airwindows/blob/master/plugins/WinVST/Doublelay/Doublelay.cpp
[aw-TapeDelay2]: https://github.com/airwindows/airwindows/blob/master/plugins/WinVST/TapeDelay2/TapeDelay2.cpp
[aw-Reverb]: https://github.com/airwindows/airwindows/blob/master/plugins/WinVST/Reverb/Reverb.cpp
[aw-kPlateD]: https://github.com/airwindows/airwindows/blob/master/plugins/WinVST/kPlateD/kPlateD.cpp
[aw-StereoChorus]: https://github.com/airwindows/airwindows/blob/master/plugins/WinVST/StereoChorus/StereoChorus.cpp
[aw-Vibrato]: https://github.com/airwindows/airwindows/blob/master/plugins/WinVST/Vibrato/Vibrato.cpp
[aw-AutoPan]: https://github.com/airwindows/airwindows/blob/master/plugins/WinVST/AutoPan/AutoPan.cpp
[aw-ButterComp2]: https://github.com/airwindows/airwindows/blob/master/plugins/WinVST/ButterComp2/ButterComp2.cpp
[aw-SoftGate]: https://github.com/airwindows/airwindows/blob/master/plugins/WinVST/SoftGate/SoftGate.cpp
[aw-Baxandall2]: https://github.com/airwindows/airwindows/blob/master/plugins/WinVST/Baxandall2/Baxandall2.cpp
[aw-Parametric]: https://github.com/airwindows/airwindows/blob/master/plugins/WinVST/Parametric/Parametric.cpp
[aw-Biquad2]: https://github.com/airwindows/airwindows/blob/master/plugins/WinVST/Biquad2/Biquad2.cpp
[aw-Capacitor2]: https://github.com/airwindows/airwindows/blob/master/plugins/WinVST/Capacitor2/Capacitor2.cpp
[aw-Density2]: https://github.com/airwindows/airwindows/blob/master/plugins/WinVST/Density2/Density2.cpp
[aw-ToTape6]: https://github.com/airwindows/airwindows/blob/master/plugins/WinVST/ToTape6/ToTape6.cpp
[aw-DeRez3]: https://github.com/airwindows/airwindows/blob/master/plugins/WinVST/DeRez3/DeRez3.cpp
[nam]: https://github.com/sdatkinson/NeuralAmpModelerCore/blob/main/README.md
[nam-license]: https://github.com/sdatkinson/NeuralAmpModelerCore/blob/main/LICENSE
[nam-plugin]: https://github.com/sdatkinson/NeuralAmpModelerPlugin/blob/main/NeuralAmpModeler/NeuralAmpModeler.h
[nam-json]: https://github.com/sdatkinson/NeuralAmpModelerCore/blob/main/Dependencies/nlohmann/json.hpp
[adt-license]: https://github.com/sdatkinson/AudioDSPTools/blob/main/LICENSE
[eigen-license]: https://gitlab.com/libeigen/eigen/-/blob/bc3b39870ecb690a623a3f49149a358b95c5781d/COPYING.README
[mpl-faq]: https://www.mozilla.org/en-US/MPL/2.0/FAQ/
[fft-source]: https://github.com/HiFi-LoFi/FFTConvolver/blob/non-uniform/FFTConvolver.h
[stretch]: https://github.com/Signalsmith-Audio/signalsmith-stretch/blob/main/README.md
[stretch-license]: https://github.com/Signalsmith-Audio/signalsmith-stretch/blob/main/LICENSE.txt
[linear-license]: https://github.com/Signalsmith-Audio/linear/blob/main/LICENSE.txt
[plaits-engines]: https://github.com/pichenettes/eurorack/blob/master/plaits/dsp/voice.cc
[rings-source]: https://github.com/pichenettes/eurorack/blob/master/rings/dsp/part.h
[tides-source]: https://github.com/pichenettes/eurorack/blob/master/tides2/poly_slope_generator.h
[peaks-source]: https://github.com/pichenettes/eurorack/tree/master/peaks/drums
[warps-source]: https://github.com/pichenettes/eurorack/blob/master/warps/dsp/parameters.h
[plaits-sixop]: https://github.com/pichenettes/eurorack/blob/master/plaits/dsp/engine2/six_op_engine.cc
[plaits-fm-patch]: https://github.com/pichenettes/eurorack/blob/master/plaits/dsp/fm/patch.h
[plaits-fm-voice]: https://github.com/pichenettes/eurorack/blob/master/plaits/dsp/fm/voice.h
[daisy-drums]: https://github.com/electro-smith/DaisySP/tree/master/Source/Drums
[daisy-string]: https://github.com/electro-smith/DaisySP/blob/master/Source/PhysicalModeling/stringvoice.h
[stk-license]: https://github.com/thestk/stk/blob/master/LICENSE
[stk-readme]: https://github.com/thestk/stk/blob/master/README.md
[mda-suite]: https://github.com/steinbergmedia/vst3_public_sdk/tree/master/samples/vst/mda-vst3
[mda-jx10-dsp]: https://github.com/steinbergmedia/vst3_public_sdk/blob/master/samples/vst/mda-vst3/source/mdaJX10Processor.cpp
[mda-piano-data]: https://github.com/steinbergmedia/vst3_public_sdk/blob/master/samples/vst/mda-vst3/source/mdaPianoData.h
[mda-epiano-data]: https://github.com/steinbergmedia/vst3_public_sdk/blob/master/samples/vst/mda-vst3/source/mdaEPianoData.h
[mda-jx10-controls]: https://github.com/steinbergmedia/vst3_public_sdk/blob/master/samples/vst/mda-vst3/source/mdaJX10Controller.cpp
[mda-epiano-controls]: https://github.com/steinbergmedia/vst3_public_sdk/blob/master/samples/vst/mda-vst3/source/mdaEPianoController.cpp
[mda-piano-controls]: https://github.com/steinbergmedia/vst3_public_sdk/blob/master/samples/vst/mda-vst3/source/mdaPianoController.cpp
[mda-leslie-controls]: https://github.com/steinbergmedia/vst3_public_sdk/blob/master/samples/vst/mda-vst3/source/mdaLeslieController.cpp
[mda-dynamics-controls]: https://github.com/steinbergmedia/vst3_public_sdk/blob/master/samples/vst/mda-vst3/source/mdaDynamicsController.cpp
[mda-deesser-controls]: https://github.com/steinbergmedia/vst3_public_sdk/blob/master/samples/vst/mda-vst3/source/mdaDeEsserController.cpp
[mda-dubdelay-controls]: https://github.com/steinbergmedia/vst3_public_sdk/blob/master/samples/vst/mda-vst3/source/mdaDubDelayController.cpp
[mda-talkbox-controls]: https://github.com/steinbergmedia/vst3_public_sdk/blob/master/samples/vst/mda-vst3/source/mdaTalkBoxController.cpp
[stk-shakers]: https://github.com/thestk/stk/blob/master/include/Shakers.h
[stk-modalbar]: https://github.com/thestk/stk/blob/master/include/ModalBar.h
[stk-voicform]: https://github.com/thestk/stk/blob/master/include/VoicForm.h
[stk-clarinet]: https://github.com/thestk/stk/blob/master/include/Clarinet.h
[stk-flute]: https://github.com/thestk/stk/blob/master/include/Flute.h
[stk-brass]: https://github.com/thestk/stk/blob/master/include/Brass.h
[stk-bowed]: https://github.com/thestk/stk/blob/master/include/Bowed.h
[stk-mandolin]: https://github.com/thestk/stk/blob/master/include/Mandolin.h
[stk-beethree]: https://github.com/thestk/stk/blob/master/include/BeeThree.h
[stk-rhodey]: https://github.com/thestk/stk/blob/master/include/Rhodey.h
[stk-wurley]: https://github.com/thestk/stk/blob/master/include/Wurley.h
[elements-source]: https://github.com/pichenettes/eurorack/blob/master/elements/dsp/part.h
[elements-patch]: https://github.com/pichenettes/eurorack/blob/master/elements/dsp/patch.h
[stk-bandedwg]: https://github.com/thestk/stk/blob/master/include/BandedWG.h
[stk-mesh]: https://github.com/thestk/stk/blob/master/include/Mesh2D.h
