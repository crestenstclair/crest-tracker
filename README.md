# Crest Tracker

Crest Synth's complete application is now forked into this repository. Tracker adds sequencing through its existing shell, renderer and input system. The previous separate web UI and controller mapper have been removed.

```sh
make dev                       # Tracker
make synth                     # Original standalone Synth
make bundle ARGS=--release     # target/release/Crest Tracker.app
```

A fresh checkout needs the pinned Rust 1.96.0 toolchain, the native build prerequisites from [Synth's DESIGN.md](crates/crest-synth/DESIGN.md), and `git lfs pull` for the inherited SoundFont. No sibling checkout or Cargo dependency override is required. Local builds and CI use the same toolchain; compiler and lint upgrades are deliberate changes.

## Ownership

- `crates/crest-synth`: complete Synth source, native DSP, devices, reducer, renderer, assets, licenses, tests and diagnostic executable. No Tracker dependency.
- `crates/crest-tracker`: pure sequencing, editing, recording and musical data. No Synth or platform dependency.
- `crates/crest-tracker-app`: composes both through `HostedSynthControl`; owns the Song file and Tracker workspace state.

The independent [Crest Performance v0.1.0](https://github.com/crestenstclair/crest-linnstrument/releases/tag/v0.1.0)
library is consumed by its Git tag and lockfile, without a sibling checkout. The Tracker-owned
`performance` adapter provides exact rational/fixed-point time conversion, instrument bindings,
public commands, timestamped MIDI input, versioned state and separate LED feedback. Shared
contract fixtures verify expressive record/reopen/playback equivalence. Song format 2 embeds
the library session and instrument bindings; save, sound import and recovery retain that state.
Library validation and engine allocation precede sound replacement. Format 1 Songs upgrade
in memory without changing their source files; older applications reject format 2 rather than
discarding performance state. Keep the original file when sharing with an older application.
Open **Menu → Performance clips → Open expressive example**, then return to Performance clips
and audition a clip. Its loop runs through the released library, Tracker's timestamped audio
scheduler and the embedded Synth's owned-note MIDI boundary. Stop releases its scheduled voices.
The example uses a firmware-captured phrase from the standalone v0.1.0 release and an existing
Braids patch. Spectral witnesses verify independent pitch, timbre and pressure in rendered audio,
along with deadline recovery, unsupported-engine rejection and allocation-free callback work.

Audition uses a disposable clip snapshot, the Song's tempo at start, a 50 ms start lead and the
existing 100 ms scheduling window. Tempo, graph or sample-rate changes stop it explicitly.
The first output profile is lower-zone MPE with 15 members and a ±48-semitone range per destination;
only engines advertising proven owned-expression support are admitted (currently Braids).
This does not enable integrated live capture, virtual-pad launch/LED feedback, sample MPE or
Steam Deck qualification. Those remain integration work; the standalone application retains
its complete firmware-driven record-and-launch workflow.

Edit both packages in this checkout. One diff and commit can contain a cross-boundary change. The original sibling Synth checkout remains untouched. [FORK.json](crates/crest-synth/FORK.json) records the imported working-tree source and hashes, including the uncommitted fixes present at import.

## Current application

Song, Chain and Phrase editing now share the original Synth shell and semantic input path. The application includes:

- Global Settings: tempo, key and custom scale, pitch display/entry mode, new-Song tempo, clock configuration, device/controller links, and Sample/SoundFont starting directories.
- Tracker, Piano Roll and Drum editing; duration spans, selection, copy/cut/paste/duplicate/delete, movement/resizing, exact fractional quantize grids, attack/release velocity, instrument/channel targets, and note/master expression points with raw values.
- Phrase length, division/tuplets with lossless retiming, key inheritance, Euclidean/Random authoring, repeatable seeds and baking generated notes into editable phrases.
- Three typed FX slots with Apply/Cancel returning to Phrase. The chord picker contains shared interval shapes and Cancel; a separate Chord Bank editor provides piano-style interval toggles, voicing and shared-library saving.
- Track mute/loop/name, Chain naming, occurrence repeats/reordering/removal, Make Unique, scoped playback, and scoped source navigation. Minimap code is retained, with its UI entry points removed.
- Song New/Open/Save/Save As, controller text entry, recovery, Collect Assets, native Synth-session import/export, sound library publish/adopt, missing-resource locate/replace, and general MIDI import.
- MIDI output selection, lower/upper MPE output zones and control-surface mappings with conflict validation.

Existing version-1 `.crest-song` files retain their complete Synth sessions, mappings, resources and sequencer content. File replacement retains the current document until validation and graph preparation succeed. Recording setup is deferred; Expanded view is removed from the user interface. Existing recording and legacy projection data remain readable.

Chains, phrases and instruments each have their own Song-local display numbering: `00`, `01`, … `09`, `10`, `11`. These decimal catalog numbers are independent of note counts and persistent reference IDs; labels wider than two digits remain supported for large Songs.

Instruments, Sample/SoundFont browsers, native parameter editors, effects, Mixer, Sends, MIDI devices and controller assignments use the original Synth screens. Standalone Synth and its diagnostics remain available. Native Synth settings and button preferences retain their existing storage and behavior.

Tracker screens receive the same semantic actions as Synth. The shared defaults use **Cross / south = Edit**, **Circle / east = Shift**, **Triangle / north = Start (transport/preview)** and **Options / physical Start = play/stop in Tracker**. Shift+Start opens Global Settings; native Synth keeps its Settings binding. Button positions are consistent across controller brands; printed Nintendo letters differ from Xbox letters. Custom assignments remain supported in Synth Settings. The former generated defaults (L1 Shift, Triangle Settings) are repaired on load, with an exact `controller-buttons.before-face-layout.json` backup beside the preferences file; customized layouts are left unchanged.

Default controls:

| Action | Keyboard | Controller role |
|---|---|---|
| Move | Arrows or W/A/S/D | D-pad / left stick |
| Confirm / enter note | Return | Edit tap |
| Adjust | Hold K + direction | Hold Edit + direction |
| Create / cycle chain or phrase in selected cell | Hold K + direction | Hold Edit + direction |
| Song reference: ±1 / ±10 | K+Left/Right / K+Up/Down | Edit+Left/Right / Edit+Up/Down |
| Tracker pitch: single step / octave | K+Left/Right / K+Up/Down | Edit+Left/Right / Edit+Up/Down |
| Song → Chain → Phrase | Shift+Right / D | Shift+Right |
| Global settings from Song / Phrase settings from Phrase | Shift+Up / W | Shift+Up |
| Song → Mixer; Phrase/Chain → Quick Jump | Shift+Down / S | Shift+Down |
| Parent / clear | Q / E | Previous Patch / Next Patch |
| Play / stop Tracker | Space | Start |
| Application menu (Song or Synth) | Shift+Q | Shift+Previous Patch |
| Global settings from any workspace | Menu → Global settings | Shift+Start in Tracker; Options in Synth |

Held Edit cycles existing chain/phrase definitions and creates a new one beyond the last definition, without opening another layer. Release Edit before ordinary navigation. Each held reference edit is one undo step. Hold Circle and tap Right to enter Song → Chain → Phrase, or Left to return. Shift remains held when crossing into Synth.

Native Synth keeps its own hierarchy: Detail **Shift+Down** returns to Overview; Overview **Shift+Down** opens Mixer. **Shift+Up** reverses the Song-to-Mixer transition. **Shift+Left** from an instrument Overview returns to its Tracker origin; the visible **SONG** button returns directly to Song. Settings/Quick Jump close back to their prior page and focus with the opposite vertical page gesture. Tracker Settings opens with Shift+Start or Menu → Global settings. Music, Clock/MIDI, Devices and Files have top tabs: Left/Right switches tabs, Down enters fields, Up from the first field returns to tabs.

D-pad directions repeat after 280 ms, then every 45 ms. Page transitions and transport never repeat. Release, disconnect and remapping cancel held motion. Piano Roll uses D-pad/left stick for time/pitch, Edit tap to add/remove notes, Edit+Left/Right to resize, Edit+Up for note actions, and right stick for length/velocity. Select holds a range; R1 holds fine timing, when those buttons have no global binding. Clicking a note selects it. Grid is available in the bottom toolbar; note tools retain movement and exact timing. Drum uses Left/Right for steps and Up/Down for lanes.

Edit+Up on an FX column opens commands grouped by type; selecting a command opens its parameters, with no redundant slot selector. Apply/Cancel returns to Phrase. Chord cells use bank names/symbols. Open **Chords** from Phrase or the FX popover. Choose a shared shape to apply immediately to the selected note's root, or Cancel to return to Phrase. There are no search/create/edit links in the picker. **Menu → Chord Bank** opens the separate editor and shared library. **OFF** on the Phrase toolbar, or **Menu → Editing tools → Note OFF**, inserts an explicit release marker at the cursor; playback releases only that track's earlier voices for the selected target.

Euclidean and Randomizer have primary controls and a compiled preview strip. Euclidean **Pulse notes** assigns pitches by pulse order; rotation preserves those assignments and unused assignments survive reducing pulse count. Unassigned pulses use the base note. Numeric Edit+Up/Down changes by ten (pitch fields by an octave); Left/Right remains fine. **Apply rhythm / Play preview** auditions the phrase. **Generate** advances the Random seed; Repeat behavior selects a fixed pattern or variation each loop. **Use notes in Tracker/Piano Roll** makes an editable phrase while retaining the generator source. Older Random documents retain their existing varying-loop behavior.

Text fields accept a keyboard, or controller character selection with Edit+direction, append/delete, and Accept/Cancel. Song File accepts absolute paths. Instrument editing, Sample/SoundFont browsing, native effects, Mixer and Sends keep their original Synth controls.

## Corridors of Time

Open **Song File → Import Corridors of Time**, then press **Start / Space**. The bundled MIDI becomes an editable, unsaved Song; the normal replacement prompt protects current work. Save it as a `.crest-song` to keep edits. For other files, use **Song File → Import MIDI file**. It accepts metrical SMF type 0/1, preserves channels, program changes, timing, tempo/meter changes, release velocities and channel controls, and prepares sounds from the bundled SoundFont. Type 2, SMPTE timing, SysEx and unavailable presets fail explicitly before replacement. Input size is limited to 64 MiB.

```sh
make demo-corridors             # Visible event-driven tour, then full-song playback
make test-corridors             # Same event script through headless production audio
```

The demo creates a chain, phrase and note through Edit events, changes pitch, navigates layers, cancels and confirms import, tests Start press/release/stop, then plays the complete arrangement. It checks application state and rendered audio, prints a JSON report, and exits with an error on failure or early close. It uses isolated scratch storage and attaches no physical MIDI or controller devices. Native and headless runs share the same scene; only the window/audio device differs. The scene does not read controller state or modify the Song directly.

On macOS, `make demo-corridors` builds and launches the native `.app` bundle. After the short edit/import tour, playback returns to Song view with the current bar shown in the demo status. The import uses unity master gain; the scene checks final stereo output level and clipping as well as individual track signals.

Import preserves all 24,218 positive-velocity attacks and 21 sounding parts. Parts sharing MIDI channels occupy the same 16 arrangement lanes, with separate Synth patches. Song view contains 29 consecutive four-bar placements per lane (the final placement is one bar), each opening a chain and its Piano Roll/Drum phrase. Timing remains at the source's 384 ticks per quarter note; tempo is 110 BPM. The source has 33 same-tick gates (expanded to one tick) and two missing releases (closed at their source track end). Silent velocity-zero placeholders do not create notes.

Sounds use the bundled SGM SoundFont. Source GM programs are retained except for these explicit arrangement mappings: Electric Drum Kit uses bank 128/program 24; missing programs for Smooth Synth, 8-Bit Sawtooth and Koto use bank 0/programs 89, 81 and 107 respectively (zero-based). This is a SoundFont rendition, not the original game's sound bank.

Synth's prepared Patch budget defaults to 32, admitting all 21 parts. It is configurable at build time with `CREST_SYNTH_PATCH_CAPACITY`; the current fixed-size real-time transports still require rebuilding to change that budget.

## Deferred work and validation limits

Recording setup and its physical input/monitoring acceptance are deferred at the user's request. Expanded view is dropped. Physical MIDI/MPE output, external clock, control-surface and attached-controller acceptance remain separate hardware checks; event-driven and virtual tests do not prove those devices. Runtime adjustment of Synth's prepared Patch budget still requires replacing the inherited build-time capacity mechanism.

Tracker uses the shared shell with full-width grids, eight-track Song windows, contextual chord/FX popovers, interval-keyboard editing and continuous Piano Roll duration bars. Event tests cover the reviewed navigation, repeats, OFF ownership, chord selection and generator paths. Hardware acceptance and full-screen Figma parity require native comparison; passing reducer tests alone is not visual acceptance.
## Verification

```sh
make test                      # Pure sequencing + application contracts, packaging checks
make test-synth                # Inherited Synth library suite
make test-host                 # Explicit integrated audio/file witnesses
make lint
make fmt-check
make verify-bundle
```

The discarded shell's paint/latency measurements do not certify the rebuilt UI. Native smoke checks must exercise the current application, including edits, Synth navigation and save/reopen. Keep the remaining acceptance conditions in [implementation-plan.md](implementation-plan.md).

The pre-recovery baseline is commit `0d85318cc63f63c8bce830dc2b0e5af43c0784fa`. No saved songs or machine preferences are removed or migrated destructively. Existing version 1 Songs still open; Euclidean per-pulse assignments require this build. Sample/SoundFont roots now live in the shared Synth `browser-starts.json` configuration (macOS: `~/Library/Application Support/crest-synth`). Existing Tracker paths are imported only if shared settings are absent. The original preferences remain readable; saved Songs contain no machine paths. New Songs use Plaits as the initial engine.
