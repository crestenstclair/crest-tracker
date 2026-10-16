# Crest Tracker

Crest Synth's complete application is now forked into this repository. Tracker adds sequencing through its existing shell, renderer and input system. The previous separate web UI and controller mapper have been removed.

```sh
make dev                       # Tracker
make synth                     # Original standalone Synth
make bundle ARGS=--release     # target/release/Crest Tracker.app
```

A fresh checkout needs Rust, the native build prerequisites from [Synth's DESIGN.md](crates/crest-synth/DESIGN.md), and `git lfs pull` for the inherited SoundFont. No sibling checkout or Cargo dependency override is required.

## Ownership

- `crates/crest-synth`: complete Synth source, native DSP, devices, reducer, renderer, assets, licenses, tests and diagnostic executable. No Tracker dependency.
- `crates/crest-tracker`: pure sequencing, editing, recording and musical data. No Synth or platform dependency.
- `crates/crest-tracker-app`: composes both through `HostedSynthControl`; owns the Song file and Tracker workspace state.

Edit both packages in this checkout. One diff and commit can contain a cross-boundary change. The original sibling Synth checkout remains untouched. [FORK.json](crates/crest-synth/FORK.json) records the imported working-tree source and hashes, including the uncommitted fixes present at import.

## Current application

The first rebuilt slice provides Song, Chain and Phrase navigation; step-note entry and adjustment; Piano Roll and Drum grids; playback; basic chord recall/copy; phrase name, form and length; tempo; undo/redo; recording actions; and Song New/Open/Save/Save As. Existing version-1 `.crest-song` files retain their complete Synth sessions, mappings, resources and sequencer content. File replacement retains the current document until validation and graph preparation succeed.

Chains, phrases and instruments each have their own Song-local display numbering: `00`, `01`, … `09`, `0A`, … `0F`, `10`. These hexadecimal catalog numbers are independent of note counts and persistent reference IDs; labels wider than two digits remain supported for large Songs.

Instruments, Sample/SoundFont browsers, native parameter editors, effects, Mixer, Sends, MIDI devices and controller assignments use the original Synth screens. Standalone Synth and its diagnostics remain available. Native Synth settings and button preferences retain their existing storage and behavior.

Tracker screens receive the same semantic actions as Synth. The shared defaults use **Cross / south = Edit**, **Circle / east = Shift**, **Triangle / north = Start (transport/preview)** and **Options / Start = Settings**. Button positions are consistent across controller brands; printed Nintendo letters differ from Xbox letters. Custom assignments remain supported in Synth Settings. The former generated defaults (L1 Shift, Triangle Settings) are repaired on load, with an exact `controller-buttons.before-face-layout.json` backup beside the preferences file; customized layouts are left unchanged.

Default controls:

| Action | Keyboard | Controller role |
|---|---|---|
| Move | Arrows or W/A/S/D | D-pad |
| Confirm / enter note | Return | Edit tap |
| Adjust | Hold K + direction | Hold Edit + direction |
| Create / cycle chain or phrase in selected cell | Hold K + direction | Hold Edit + direction |
| Song → Chain → Phrase | Shift+Right / D | Shift+Right |
| Song settings / Phrase settings | Shift+Up / W | Shift+Up |
| Song → Mixer; other Tracker views → Quick Jump | Shift+Down / S | Shift+Down |
| Parent / clear | Q / E | Previous Patch / Next Patch |
| Play / stop Tracker | Space | Start |
| Application menu (Song or Synth) | Shift+Q | Shift+Previous Patch |

Held Edit cycles existing chain/phrase definitions and creates a new one beyond the last definition, without opening another layer. Release Edit before ordinary navigation. Each held edit is one undo step. Hold Circle and tap Right to enter Song → Chain → Phrase, or Left to return. Shift remains held when crossing into Synth. The menu has Tracker, Instruments, Chord Bank, Mixer, Sends and Settings. The original Synth controls operate inside Synth screens; the application menu also returns from those screens. Quick Jump includes Song File. Song File's “Open path…” accepts an existing absolute path. Side-panel actions offer the same operations with the mouse.

## Corridors of Time

Open **Song File → Import Corridors of Time**, then press **Start / Space**. The bundled MIDI becomes an editable, unsaved Song; the normal replacement prompt protects current work. Save it as a `.crest-song` to keep edits. This is a named bundled import, not a general MIDI file chooser.

```sh
make demo-corridors             # Visible event-driven tour, then full-song playback
make test-corridors             # Same event script through headless production audio
```

The demo creates a chain, phrase and note through Edit events, changes pitch, navigates layers, cancels and confirms import, tests Start press/release/stop, then plays the complete arrangement. It checks application state and rendered audio, prints a JSON report, and exits with an error on failure or early close. It uses isolated scratch storage and attaches no physical MIDI or controller devices. Native and headless runs share the same scene; only the window/audio device differs. The scene does not read controller state or modify the Song directly.

On macOS, `make demo-corridors` builds and launches the native `.app` bundle. After the short edit/import tour, playback returns to Song view with the current bar shown in the demo status. The import uses unity master gain; the scene checks final stereo output level and clipping as well as individual track signals.

Import preserves all 24,218 positive-velocity attacks and 21 sounding parts. Parts sharing MIDI channels occupy the same 16 arrangement lanes, with separate Synth patches. Song view contains 29 consecutive four-bar placements per lane (the final placement is one bar), each opening a chain and its Piano Roll/Drum phrase. Timing remains at the source's 384 ticks per quarter note; tempo is 110 BPM. The source has 33 same-tick gates (expanded to one tick) and two missing releases (closed at their source track end). Silent velocity-zero placeholders do not create notes.

Sounds use the bundled SGM SoundFont. Source GM programs are retained except for these explicit arrangement mappings: Electric Drum Kit uses bank 128/program 24; missing programs for Smooth Synth, 8-Bit Sawtooth and Koto use bank 0/programs 89, 81 and 107 respectively (zero-based). This is a SoundFont rendition, not the original game's sound bank.

Synth's prepared Patch budget defaults to 32, admitting all 21 parts. It is configurable at build time with `CREST_SYNTH_PATCH_CAPACITY`; the current fixed-size real-time transports still require rebuilding to change that budget.

## Remaining Tracker work

This is a recovered application base plus the first working sequencing slice, not completion of the full implementation plan. Advanced Tracker UI still needs rebuilding: Expanded/Minimap, generator authoring, full FX/voicing editors, expression/quantize/selection tooling, external MIDI/clock configuration, resource-repair and sound-library screens. Their retained backend code and saved data do not imply that those UI journeys are complete. Recording controls still need a full native device acceptance pass. Physical controller verification requires a connected device; a passing semantic test is not that verification.

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

The pre-recovery baseline is commit `0d85318cc63f63c8bce830dc2b0e5af43c0784fa`. No saved songs or machine preferences are removed or migrated destructively. The file format remains compatible with that baseline, so rollback does not require converting user files.
