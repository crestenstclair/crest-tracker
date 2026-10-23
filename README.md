# Crest Tracker

Crest Synth's complete application is now forked into this repository. Tracker adds sequencing through its existing shell, renderer and input system. The previous separate web UI and controller mapper have been removed.

```sh
make dev                       # Tracker
make synth                     # Original standalone Synth
make bundle ARGS=--release     # target/release/Crest Tracker.app
```

A fresh checkout needs Rust, the native build prerequisites from [Synth's DESIGN.md](crates/crest-synth/DESIGN.md), and `git lfs pull` for the inherited SoundFont. No sibling checkout or Cargo dependency override is required.

Ctrl-C in the launching terminal interrupts `make dev` or `make synth`. Use the application's Close command for normal shutdown with unsaved-change protection.

`make dev` uses optimized code with debug symbols (`profiling` profile). Performance
metrics are enabled by default: the terminal prints the JSONL path under
`.dev/performance/`. Set `PERF_DIR=/path/to/logs` to change it, or `PERF_DIR=0` to
disable measurement. Raw `cargo run -p crest-tracker-app` remains an unoptimized
debug launch; use optimized builds for real-time performance comparisons.

## Ownership

- `crates/crest-synth`: complete Synth source, native DSP, devices, reducer, renderer, assets, licenses, tests and diagnostic executable. No Tracker dependency.
- `crates/crest-tracker`: pure sequencing, editing, recording and musical data. No Synth or platform dependency.
- `crates/crest-tracker-app`: composes both through `HostedSynthControl`; owns the Song file and Tracker workspace state.

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

Tracker screens receive the same semantic actions as Synth. The shared defaults use **Cross / south = Edit**, **Circle / east = Shift**, **Triangle / north = Start (transport/preview)** and **Options / physical Start = play/stop in Tracker**. Start, Triangle and Space always toggle song playback on every Tracker and hosted Synth page, including with Shift held. Global Settings opens from Main Menu. Button positions are consistent across controller brands; printed Nintendo letters differ from Xbox letters. Custom assignments remain supported in Synth Settings. The former generated defaults (L1 Shift, Triangle Settings) are repaired on load, with an exact `controller-buttons.before-face-layout.json` backup beside the preferences file; customized layouts are left unchanged.

Default controls:

| Action | Keyboard | Controller role |
|---|---|---|
| Move | Arrows or W/A/S/D | D-pad / left stick |
| Confirm / enter note | Return | Edit tap |
| Adjust | Hold K + direction | Hold Edit + direction |
| Create / cycle chain or phrase in selected cell | Hold K + direction | Hold Edit + direction |
| Edit popover / detailed picker | `[` | West / Square |
| Song reference: ±1 / ±10 | K+Left/Right / K+Up/Down | Edit+Left/Right / Edit+Up/Down |
| Tracker pitch: single step / octave | K+Left/Right / K+Up/Down | Edit+Left/Right / Edit+Up/Down |
| Song → Chain → Phrase | Shift+Right / D | Shift+Right |
| Song / Chain / Phrase settings; selected Synth detail | Shift+Up / W | Shift+Up |
| Main menu from a workspace; return from detail | Shift+Down / S | Shift+Down |
| Parent / clear | Q / E | Previous Patch / Next Patch |
| Play / stop Tracker | Space | Start |
| Application menu (Song or Synth) | Shift+Q | Shift+Previous Patch |
| Global settings from any workspace | Menu → Global settings | Shift+Down → Global settings |

Held Edit cycles existing chain/phrase definitions and creates a new one beyond the last definition, without opening another layer. Release Edit before ordinary navigation. Each held reference edit is one undo step. Hold Circle and tap Right to enter Song → Chain → Phrase, or Left to return. Shift remains held when crossing into Synth.

**Shift+Down** opens Main Menu from Song, Chain, Phrase, Instruments, Mixer and Sends. From a detail/settings page it returns one level. **Menu Shift+Up** restores the exact previous page and focus. **Shift+Up** opens Song/Chain/Phrase settings, the selected instrument/effect detail, or Mixer track detail. Shift+Left remains a back shortcut. The visible **MENU** button opens the same menu; Quick Jump has been removed. Global Settings opens from Menu → Global settings. Music, Clock/MIDI, Devices, Files and Global/defaults have top tabs: Left/Right switches tabs, Down enters fields, Up from the first field returns to tabs. New-Song tempo lives in Global/defaults.

Chain rows show the phrase number once, an optional custom name, repeats, steps × division, and total quarter-note beats including repeats. Conditional Jump FX remain labeled conditional. Chain settings provides its name field. New patches use independent catalog names `00`, `01`, …; saved names stay unchanged. **Saved instruments** is the publish/adopt library, separate from installed patches. Song settings controls the saved Patch admission limit within the prepared build budget; it cannot be lowered below installed count or changed during structural preparation.

D-pad directions repeat after 280 ms, then every 45 ms. Page transitions and transport never repeat. Release, disconnect and remapping cancel held motion. Drum uses Left/Right for steps and Up/Down for lanes.

In Tracker list menus, including the file picker, Up/Down moves one item and Left/Right pages up/down by the visible list capacity, stopping at either end. Held directions repeat. Settings tabs and spatial Piano Roll/chord controls retain their own directional navigation; held Edit still adjusts values.

Tracker queues semantic actions, pointer events, stick movement and roll modifiers in arrival order. Each input's commands finish before the next input resolves its page or field, so rapid navigation and held edits cannot operate on a stale layer. Unconsumed actions retain the native Synth route.

## Piano Roll

The approved Gadget-inspired roll has visible piano keys, a bar/beat ruler, continuous duration bars, aligned velocity bars, and seven bottom editing controls. Dashed cyan editing and amber playback cursors remain independent. Zoom, pitch/time scrolling and scale shading preserve the music; manual cursor movement pauses Follow.

**Step Grid**, **New Length**, **Selected Length** and **Quantize Grid** are independent. Step Grid and Quantize offer 1/4 through 1/64, straight or triplet. Length also offers whole/half notes and exact beat fractions. New length, velocity and target defaults affect future notes only. The selected-note inspector previews pitch, start, duration, velocity, release velocity and target; Apply commits one undoable transaction, and Cancel leaves the document untouched.

Tools → **Overlapping notes…** lists notes under the exact cursor, distinguished by destination, start, length and velocity. Choose one to select it and reveal its start; Cancel preserves selection. The picker, inspector, keys and cursor summary all respect the Phrase spelling override.

D-pad/left stick moves time/pitch. Edit tap inserts or selects; selecting an occupied cell never deletes it. Edit+Left/Right nudges, West/Square (keyboard `[`) opens Tools, and right stick changes duration/value. Configured Range and Fine buttons work when those buttons have no global role. Tools includes selection/clipboard, overlapping-note navigation, Note Off at the exact cursor, expression/FX, and history. Velocity lane focus uses Left/Right for notes, Edit+Left/Right for ±1, Edit+Up/Down for ±10, and Up/Down to return to the roll. Fine makes velocity changes ±1. Bottom-right hints use the current field and actual button assignments.

Quantize previews cyan results against amber original outlines. It defaults to selected starts only; All Notes and duration quantize are explicit. Apply changes the chosen scope once, Cancel preserves exact originals, and Undo restores raw values and expression. Starts snap to the nearest valid grid point inside the phrase, with midpoint ties later; note tails may extend beyond it. Shared-phrase protection remains in force.

Expression shows a time-aligned held-value trace for the selected note's bend, pressure, key pressure, CC/timbre, sustain or program lane. Explicit MPE Master scope stays separate. Previous/Next Point and Displayed Lane browse without changing music; exact time/raw-value fields, coarse/fine directions, Add/Delete and Undo/Redo edit through the normal document commands. Point selection survives chronological reordering and duplicate timestamps. **Channel automation** browses independent phrase-time controls by destination and lane, including imported bend, pressure, sustain, CC and program events. Exact time/value editing, coarse/fine directions, Add/Delete and Undo/Redo use the same edit guard. Moving or deleting one control preserves notes, FX and other controls sharing its original event. Scope selection browses existing destinations and the new-note default; it does not reroute notes. These controls do not expose the deferred recording setup.

```sh
make demo-piano               # Isolated visible editing tour, then audible playback
make test-piano               # Same semantic events through production DSP
```

The tour creates its own scratch Song and checks independent grids/defaults, inspector previews, Apply/Cancel, quantize, Undo/Redo, expression and Stop cleanup. It never reads a controller directly or changes the user's working Song.

## Independent track demo

```sh
make demo-cursors              # Visible, isolated event scene
make test-cursors              # Same script through production DSP
```

Track 0 plays ten 10-row chain occurrences while Track 1 plays one 100-row chain. Both reuse the same phrase with a five-row offset, then loop. The scene checks every observed track cursor and Song cell, audible output and Stop cleanup. Song marks each track's active cell; slot rows are not a shared timeline. The demo uses scratch storage and never attaches physical controllers or MIDI inputs.

The Song slot picker contains exactly **Kill**, **Rest**, and **Chain**. Open it with West/Square (`[`), choose a type, then configure its number directly in Song: Edit+Left/Right changes by one; Edit+Up/Down changes by ten. Chain selects the chain number; Rest/Kill select the measure count. Choosing Chain keeps an existing chain selection or starts at 00, creating it if needed. West/Square closes the picker without changing the slot.

Rest and Kill are built in and need no chain definition. `Rest 2` pads two measures using the active project meter and preserves notes already ringing. `Kill 2` adds the same padding, force-releases that track's notes (including sustain), cancels pending attacks/FX, and ends temporary sends. Mixer routing and effect buffers remain active, so release/reverb tails finish. Later slots resume normally. Both types save with the Song, undo normally, and retain independent track playheads. Raw external MIDI cannot independently release a pedal-held note on a shared channel; that case reports an explicit error and requires per-note MPE routing.

West/Square (keyboard `[`) on an FX column opens commands grouped by type; selecting a command opens its parameters, with no redundant slot selector. Apply/Cancel returns to Phrase. Chord cells use bank names/symbols. Open **Chords** from Phrase or the FX popover. Choose a shared shape to apply immediately to the selected note's root, or Cancel to return to Phrase. There are no search/create/edit links in the picker. **Menu → Chord Bank** opens the separate editor and shared library. **OFF** in Tracker phrase tools inserts an explicit release marker at the cursor; playback releases only that track's earlier voices for the selected target.

Euclidean and Randomizer have primary controls and a compiled preview strip. Euclidean **Pulse notes** assigns pitches by pulse order; rotation preserves those assignments and unused assignments survive reducing pulse count. Unassigned pulses use the base note. Numeric Edit+Up/Down changes by ten (pitch fields by an octave); Left/Right remains fine. **Apply rhythm / Play preview** auditions the phrase. **Generate** advances the Random seed; Repeat behavior selects a fixed pattern or variation each loop. **Use notes in Tracker/Piano Roll** makes an editable phrase while retaining the generator source. Older Random documents retain their existing varying-loop behavior.

Text fields accept a keyboard, or controller character selection with Edit+direction, append/delete, and Accept/Cancel. Song File accepts absolute paths. Instrument editing, Sample/SoundFont browsing, native effects, Mixer and Sends keep their original Synth controls.

The chord editor has separate setup and voicing columns. Up/Down moves through Name, Quality and Interval octave; Right reaches Inversion, Open voicing, Spread, Save chord and Cancel. Up from Inversion reaches the interval keyboard; Left/Right browses intervals and Down returns to voicing. The editor scrolls focused controls into view. Shapes stay root-relative; the resolved-pitch preview shows voicing at a reference root.

The bottom-right guide follows the focused cell, field or action. It shows directional meanings and increments, activation, page navigation and playback; Piano Roll also shows each stick axis and active Fine/Range modifiers. Controller position labels follow the mapping received from Synth. Text entry shows typing controls instead of keyboard transport shortcuts.

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

Synth's prepared Patch budget defaults to 32, admitting all 21 parts. Song Settings can lower or raise the saved admission limit within that budget. The upper budget is configurable at build time with `CREST_SYNTH_PATCH_CAPACITY`; the current fixed-size real-time transports still require rebuilding to change that budget.

## Feature demo scenes

```sh
make list-demos
make demo SCENE=drums           # Also euclidean, random, chords, fx, navigation
make test-demos                 # The same six scripts through headless production DSP
```

Existing `piano`, `cursors`, and `corridors` scenes are also available through `make demo SCENE=…`; their original Make targets still work. Scenes use semantic actions, visible control activation, text submission and the normal file dialogs. They never poll controller state. Each run owns scratch preferences, recovery data, chord revisions and sound publications; it does not replace your open Song or modify your shared library. Native runs show the current checkpoint, print a JSON report, and close when complete. Headless runs also verify zero callback allocations, late events, routing failures and leaked voices.

| Scene | Checks |
|---|---|
| `drums` | Time/lane navigation, hit toggling, coarse velocity, Undo/Redo, playback and Stop |
| `euclidean` | Five pulses, individual octave assignment, playback, separate Piano Roll bake, Undo/Redo |
| `random` | Coarse density, new seed, fixed/varying repeats, Tracker bake and playback |
| `chords` | Picker Cancel, root-relative M7, Undo/Redo, editor columns, interval keys, voicing, isolated library publication |
| `rest-kill` | Event-driven slot picking, coarse measure edits, Rest/Kill cursors, sustained-note release and resumed DSP playback |
| `fx` | FX-column West/Square popover, typed Cut command, Apply/Cancel, explicit Note OFF and playback |
| `transport` | Instrument changes during playback; Start from Instruments, Detail, Mixer and Global Settings; cursor continuity and voice cleanup |
| `navigation` | Menu return, Chain rename, settings tabs, default tempo separate from current tempo, Synth round trip |

Set `CREST_DEMO_REPORT_DIR` to an existing empty directory when running `make test-demos` to retain one JSON report per scene. Existing report files are never replaced.

## LLM-assisted MIDI corpus import

The importer now has a provider-neutral planning boundary. Rust parses the MIDI exactly, then exports source analysis, installed SoundFont choices, a JSON Schema and a default plan. Version 2 plans let an LLM choose useful track/patch names, installed presets, one/two/four-bar phrases, four/eight-bar chains, and a small set of named sections. Analysis includes the real meter map and each channel’s repeated bar patterns, note ranges and activity. These are practical arrangement decisions, not generated replacement music. The resulting `.crest-song` uses ordinary Synth patches and the ordinary sequencer; no LLM is involved in playback.

```sh
# Offline smoke corpus: five original fixtures, then full DSP playback of each.
make midi-fixtures ARGS='--output target/midi-fixtures'
make midi-import ARGS='target/midi-fixtures --output target/midi-baseline --audio'

# Interpret a folder of songs with your authenticated Codex CLI.
make midi-import ARGS='~/Music/MIDI --output target/midi-interpreted --planner codex --audio'

# Replan a failed sound mapping using a previous run's playback evidence.
make midi-import ARGS='song.mid --output target/midi-corrected --planner codex --feedback target/midi-interpreted --audio'

# Replay the exact saved plans without calling a model again.
make midi-import ARGS='~/Music/MIDI --output target/midi-replay --plans target/midi-interpreted --audio'
```

Every output directory must be new. Input folders are scanned recursively for `.mid` and `.midi`; individual file paths also work. The runner continues after a failed song and returns a nonzero exit status if any failed. Openable save files are grouped under `songs/`, named after their source with a short content hash. These are ordinary `.crest-song` documents: editable arrangements, Synth patches, mixer state and portable resource manifests. The adjacent `songs/assets/` directory contains the exact SoundFont bytes, shared by all saves in the run. Copy the **whole `songs/` directory** to move the collection; for one song, copy its `.crest-song` and the adjacent `assets/` directory together. Original MIDI files, plans, logs, local sound libraries and model access are not needed to reopen it. Each source also gets a separate artifact directory containing `request.json`, `plan.json`, `import.json`, and command logs. `--audio` adds `audio.json` and production-DSP logs. The top-level `report.json` and incremental `result-*.json` files distinguish successful conversion from verified audio. Failed jobs retain their evidence, including structured audio reports when a scene rejects playback. Import reports include expanded note counts, unique/placed phrases and chains, reuse counts, rest phrases and multi-phrase chains. Requests and plans contain source track names, pitch/velocity summaries and bar-pattern sequences; only an explicitly selected planner sends them to a model.

To edit an imported save, launch `make dev`, then choose **Menu → Song File → Open Song** (or **Cmd+O** on macOS). The browser starts in the current Song's folder, or the working directory when the Song has no file yet. Select a `.crest-song`, or use the file browser's path entry to enter its absolute path. Press **Space / Start** to play. These files open in Tracker; they already contain their Synth patches and do not need another MIDI import.

`--planner source` is the offline default: it preserves source bank/program choices and uses conservative phrase/section grouping; an unavailable preset fails explicitly. Native MIDI import uses the same deterministic organization. Neither offline path claims LLM interpretation. `--planner codex` uses the configured Codex model, a scratch working directory, read-only sandbox, JSON Schema and a separate structured final-response file. `--model NAME` overrides that model. The adapter follows the official [Codex noninteractive command contract](https://learn.chatgpt.com/docs/developer-commands#codex-exec). It requires an installed, authenticated CLI; the framework does not provision credentials. `--timeout` bounds each analysis/planning/import process; `--audio-timeout` bounds each audio witness including build time.

For another hosted or local model, use `--planner command --planner-command 'your-planner executable-arguments'`. This trusted command receives the complete request JSON on stdin and must write only the plan JSON to stdout; stderr is retained separately. It runs without shell expansion. This supports any model or existing orchestration tool without adding provider dependencies to Tracker. `--plans PREVIOUS_RUN` bypasses all planners. `--feedback PREVIOUS_RUN` instead gives the selected model the prior plan and playback report for each selected source, so it can correct sound choices while keeping a useful arrangement. Feedback is bound to the source hash; older failure logs are limited to their final 16 KiB. It requires `codex` or `command`, cannot be combined with plan replay, and never authorizes dropping notes or parts to evade capacity failures. Each corrected attempt gets a fresh output directory.

Plans are untrusted input. Validation requires the exact MIDI SHA-256, the same preset-catalog SHA-256, every sounding instrument and active channel exactly once, valid labels, existing presets and matching melodic/percussion roles. Sections must start at bar 1 and increase within the source bar map. Unknown fields are rejected. The builder uses meter-aware boundaries, shares identical complete phrases and chains, and retains explicit reusable rests so empty passages keep their duration. Notes crossing boundaries keep their full length. It removes patches that never receive a note and their redundant controls; control snapshots on sounding patches and global tempo events survive. An expanded performance digest proves notes, exact onset/duration, attack/release velocity, controller order, tempo, meter and sounding targets survived independently of layout identities. The framework cannot drop sounding parts, generate replacement notes, quantize the source, or execute instructions returned in a plan. Mapping and section reasons are saved for review. Version 1 mapping-only plans remain replayable with their original layout; version 2 requires an arrangement plan. Repairs already required by Tracker's note model are reported explicitly: same-tick gates become one source tick, unterminated notes close at file end, unmatched releases are counted.

Low-level commands allow manual review between stages:

```sh
cargo build --release -p crest-tracker-app
target/release/crest-tracker-app --midi-inspect song.mid > request.json
# Have a model produce plan.json using request.json's instructions and plan_schema.
target/release/crest-tracker-app --midi-import song.mid plan.json song.crest-song
"target/release/Crest Tracker.app/Contents/MacOS/crest-tracker-app" --demo-song /absolute/path/song.crest-song
```

Build the native bundle with `make bundle ARGS=--release` before the last command. `--demo-song` opens a linear imported Song through the normal file dialog, plays to completion, checks audibility on every populated source-channel mixer lane, and verifies voice cleanup. It uses isolated storage. For headless playback of a single imported Song, set `CREST_DEMO_SONG` to its absolute path and `CREST_DEMO_REPORT` to a new report path, then run `cargo test --release -p crest-tracker-app imported_song_event_scene -- --ignored --test-threads=1 --nocapture`.

Test Mixer editing during imported-song playback with:

```sh
make demo-mixer SONG="/absolute/path/song.crest-song"
```

`--demo-mixer SONG` opens the save through the same file-dialog events, starts playback, then navigates and adjusts level, pan, mute and solo on four Mixer tracks. It includes queued repeat bursts to catch UI work starving the transport scheduler. Checkpoints verify actual values and advancing playback, then return to Song and play to completion. The JSON report includes Mixer values, beats, graph revisions and audio diagnostics. It uses scratch storage and never saves over the input Song. Use a finite imported Song long enough for the editing tour, with default Mixer levels and pan. For a headless DSP check, use the same environment variables above with the `imported_song_mixer_event_scene` test; the native scene is also required to expose wall-clock scheduling stalls.

Internal playback recovers from a missed scheduling deadline automatically: it resumes the remaining notes with fresh lead time, preserving held notes and effect tails instead of stopping or bursting overdue attacks. A long stall can still produce an audible timing gap. The report counts `playback_recoveries`; genuine routing/voice failures still stop with a fault. To force three producer stalls (213 ms, 2.13 s and 427 ms) while the same Mixer scene runs through production DSP, set `CREST_DEMO_SONG` and optionally `CREST_DEMO_REPORT`, then run:

```sh
cargo test -p crest-tracker-app imported_song_mixer_recovers_from_missed_deadlines -- --ignored --test-threads=1 --nocapture
```

The low-level `--midi-import` command also collects resources into `assets/` beside its output save; the output parent must already exist. Saves are published only after their resources are collected and existing saves are never overwritten.

Tempo conversion now caches exact cumulative fractions, so sequences of unrelated tempo values do not overflow the Song beat representation. Audio event positions round only once at the final frame boundary. When an exact sampled cursor cannot fit the saved beat type, its projection is bounded to less than 1/1024 audio frame; source events remain unchanged. Tempo maps admit up to 65,536 changes and 16,384-bit cumulative fractions.

Import still explicitly rejects type 2, SMPTE, SysEx, invalid meter boundaries and unsupported sound mappings. Input and plan files have a 64 MiB admission budget; organization admits at most 16,384 bars, 65,536 phrase placements and 1,000,000 expanded events to reject tiny files encoding enormous silent spans before allocation. Synth's configured Patch budget still applies. LLM interpretation covers organization and sound selection. Synth patch design, inferred chord-bank entries and rewriting harmony or performance are outside this import contract. Exact matching deliberately keeps timing/velocity variations as separate phrases; no approximate deduplication hides source differences. Full-DSP playback is still required to verify a chosen sound renders the source notes.

## Deferred work and validation limits

Recording setup and its physical input/monitoring acceptance are deferred at the user's request. Expanded view is dropped. Physical MIDI/MPE output, external clock, control-surface and attached-controller acceptance remain separate hardware checks; event-driven and virtual tests do not prove those devices. Runtime adjustment of Synth's prepared Patch budget still requires replacing the inherited build-time capacity mechanism.

Tracker uses the shared shell with full-width grids, eight-track Song windows, contextual chord/FX popovers, interval-keyboard editing and the approved Gadget-inspired Piano Roll editing layout. Event tests cover the reviewed navigation, repeats, OFF ownership, chord selection and generator paths. Hardware acceptance and full-screen Figma parity require native comparison; passing reducer tests alone is not visual acceptance.
## Verification

Run native playback/edit stress tours across your generated saves:

```sh
make stress-corpus CORPUS="$HOME/Music/Crest Tracker/MIDI Corpus 2026-10-17/Organized Songs"
# Narrow a reproduction; --scene may be repeated. Without it all three run.
make stress-corpus CORPUS="/path/to/songs" ARGS='--match Scars --scene mixer'
make dev ARGS='--demo-stress edit "/path/to/song.crest-song"'
```

The `mixer`, `edit`, and `navigation` scenes use semantic actions and workspace
events during audible playback. They exercise queued navigation/adjustment bursts,
level/pan/mute/solo, multi-note velocity edits, undo/redo, quantize previews,
Piano Roll scrolling, settings tabs, and native instrument pages. Each tour uses
scratch preferences/recovery, stops and verifies voice cleanup, and checks the
source save's hash. No source song is saved or redistributed.

The runner builds an optimized native app once and runs scenes sequentially so
they do not compete for audio or CPU. `.dev/stress/<run>/report.json` retains source
and executable fingerprints, per-song hashes, commands, failures, and timings;
each case also keeps its event report, stderr, and performance JSONL. Failures do
not prevent testing the remaining songs. Ctrl-C stops the run; completed case
reports remain. Use `--binary` for an already-built app or `--limit` for a smaller
sample. Imported finite Piano Roll songs are the supported corpus input.

Performance logs contain cumulative p50/p95/p99/max timings and sample counts for
audio callbacks, native UI frames, WebView DOM/layout work, host ticks, requests,
scheduling, and projections. Playback timings are separate from loading/idle work.
A background writer flushes every two seconds and writes a final summary on normal
close; Ctrl-C retains the flushed samples. Callback measurement only reads the
monotonic clock and updates fixed atomics, with no callback allocation, logging,
locking or I/O. These measurements do not establish physical input-to-speaker latency.

Stress gates require zero callback deadline misses, late events, playback recoveries,
audio routing/voice faults, clipping, or stuck voices; callback p99 must stay below
70% of the device buffer deadline. During playback, native/page frame p95 must stay
below 16.667 ms, host tick/request p95 below 8 ms, and their maxima below 50 ms;
tick-gap p99/max must stay below 50/100 ms. The report includes the exact budgets.
Timing gates require a quiet machine and are separate from functional DSP witnesses.

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
