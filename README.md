# Crest Tracker

Crest Synth's complete application is now forked into this repository. Tracker adds sequencing through its existing shell, renderer and input system. The previous separate web UI and controller mapper have been removed.

```sh
make dev                       # Tracker
make synth                     # Original standalone Synth
make bundle ARGS=--release     # target/release/Crest Tracker.app
python3 scripts/package_linux.py # Linux x86_64 development archive; clean source required
```

A fresh checkout needs the pinned Rust 1.96.0 toolchain, the native build prerequisites from [Synth's DESIGN.md](crates/crest-synth/DESIGN.md), and `git lfs pull` for the inherited SoundFont. No sibling checkout or Cargo dependency override is required. Local builds and CI use the same toolchain; compiler and lint upgrades are deliberate changes.

The native Ubuntu 24.04 performance workflow builds and tests the app and embedded-engine
witnesses. Its package step creates `target/linux-candidate/`, extracts the archive into a
new directory, verifies checksums and loads the production engine inventory outside the
checkout. The layout is `bin/crest-tracker-app` plus `share/crest-tracker-app/`; keep both
together. A missing packaged SoundFont must fail even if the source checkout is still
available. System GTK3, WebKitGTK 4.1, ALSA and SDL platform libraries remain dependencies.
Native contracts and packaging passed at `52d9cfaf1612d00b384d86b40e1f16197b1e3ccb`.
Its SHA-256 is `ccfce17ae6af17d6a138a58607b6f35ef7cd49895b909e49db1ee7536e29123d`.
It does not qualify Steam Deck hardware, physical controls/audio or live Linux MIDI/audio
clock correlation.
The Linux desktop gate extracts that archive again and uses real XTest mouse/keyboard input
to load the expressive Sample example, launch/stop its clip, save, create a new project,
reopen and replay. PulseAudio monitor captures prove audible output and silence after stop;
the saved project must retain every source expression event. Reopening freezes the verified
sample into its content-hash library path; the test specifies exactly those reference changes
from the source WAV and compares every other project field exactly.
[Desktop run 39044273974](https://github.com/crestenstclair/crest-tracker/actions/runs/39044273974)
passed that package using witness `440c733`. Both loop captures had RMS 0.00587008; stopped
output was exactly zero. Rapid path entry, New clearing the bank and owned shutdown also
passed. This Xvfb workflow uses virtual audio and input, not physical devices.

## Ownership

- `crates/crest-synth`: complete Synth source, native DSP, devices, reducer, renderer, assets, licenses, tests and diagnostic executable. No Tracker dependency.
- `crates/crest-tracker`: pure sequencing, editing, recording and musical data. No Synth or platform dependency.
- `crates/crest-tracker-app`: composes both through `HostedSynthControl`; owns the Song file and Tracker workspace state.

The independent [Crest Performance library](https://github.com/crestenstclair/crest-linnstrument)
is consumed at the immutable [v0.3.0 release](https://github.com/crestenstclair/crest-linnstrument/releases/tag/v0.3.0),
commit `a05f8dfcb56f27498fb42693d4c8930f10d8e621` (API 12, library session schema 10),
with its lockfile and no sibling checkout. The release passed independent source and
extracted-package acceptance before this upgrade. This pin retains the M3 recording/playback
contract and the later harmony, generation, conditions and edit data without copying the
library implementation.
Public adapter fixtures compare schema-2 and schema-8 playback with the previous API-2 and
API-10 Git consumers' exact ordered traces. Old sessions migrate in memory, including original
free timing and expression; reading never rewrites the file. Explicit save writes schema 10.
Older API-2/API-10 applications reject these files. Keep the original project for rollback;
do not relabel new data with an older schema version.

Scene endpoints and per-note parameter locks survive project save/reopen, including values
unsupported by the current sound backend. Tracker does not yet advertise native parameter
mappings. A locked clip is rejected before launch, including from User Firmware Mode pads;
no early unlocked notes play and other running clips remain intact. Unlocked clips remain
playable when the same session contains locks or scene endpoints. Scene-morph commands reject
unsupported parameters without silently converting them to MIDI controllers. Remove locks
in the standalone host to play that clip in this version of Tracker.
Upgrade validation passed the core suite, 270 app tests, 17 native performance witnesses,
scoped lint and the verified release bundle. The native rejection witness keeps an unrelated
Braids clip audible while refusing a locked clip, with no late events or callback allocations.
A real macOS desktop opened the prior firmware-recorded Sample/arpeggio project, looped clips
1 and 3 together, stopped, saved a separate schema-10 copy and reopened it. Every performance
field remained equal except the explicit schema version. The source file retained its prior
SHA-256; the saved copy's SHA-256 is
`b4615d7c1d7e232d8c7d886e7bd284c92649368e8483feaa5acb1e40c4d5b2a5`.
The actual API-10 Git reader rejects that new desktop save without modifying it.

The current library's new controls are exposed deliberately through Tracker's semantic UI;
sharing a library version alone does not establish integration of every feature.
The Tracker-owned
`performance` adapter provides exact rational/fixed-point time conversion, instrument bindings,
public commands, timestamped MIDI input, versioned state and separate LED feedback. Shared
contract fixtures verify expressive record/reopen/playback equivalence. Song format 2 embeds
the library session and instrument bindings; save, sound import and recovery retain that state.
Library validation and engine allocation precede sound replacement. Format 1 Songs upgrade
in memory without changing their source files; older applications reject format 2 rather than
discarding performance state. Keep the original file when sharing with an older application.
Open **Menu → Performance clips → Open expressive example**, then return to Performance clips
and launch a clip. Its loop runs through the released library, Tracker's timestamped audio
scheduler and the embedded Synth's owned-note MIDI boundary. Launch additional clips to layer
them; pressing an active clip again stops only that clip. Stop releases all scheduled voices.
The example uses a firmware-captured phrase from the standalone v0.1.0 release and an existing
Braids patch. **Open expressive Sample example** uses the same phrase with the installed
Sample engine and its looping default asset. Sample CC74 controls per-note low-pass brightness
from 200 Hz to 20 kHz; pressure controls per-note gain. Spectral witnesses verify independent
pitch, timbre and pressure in Braids and actual resident sfizz audio. Integration witnesses
cover Sample playback and project save/reopen, deadline recovery and unsupported-engine
rejection; native heap/lock checks cover callback expression and voice reuse.

**Performance clips → Harmony · chords and strumplate** exposes the library's diatonic
chords in the existing semantic workspace. Choose Play or Chords, then enable MPE input.
Seven adjacent chromatic input notes beginning at the displayed input anchor select degrees
I–VII; the stock LinnStrument keeps generating its ordinary MPE messages. Tonic, scale,
sevenths/ninths, suspensions, borrowed chords, inversions, register, spread and voice leading
use the library's original implementation. The panel displays selected and last-played
voicings. All-voice or top-voice expression is captured at each attack; changing it does not
reassign held voices. Manual strumplate mode picks successive tones of the selected degree,
with each finger retaining its own expression and release. These controls use the same
keyboard/controller navigation as the other workspace panels.

Settings persist in the library session. Project reopening starts safely in Play. Recording
stores rendered chord notes and per-note curves alongside generating gestures. Existing
clip players retain their snapshots until relaunch. Mode changes cancel live sounding and
queued voices without stopping clips; an armed or active take must first be finished or
discarded. Return LinnStrument to Play also restores ordinary library playing. Chords cannot
start while Clips-mode firmware restoration is pending. Native Sample and Braids witnesses
exercise these controls, expressive chord recording, project round trips and simultaneous
playback through the actual embedded engines, with zero callback allocations/deallocations
and zero late scheduled events.

macOS desktop verification used the firmware replica's sensor pad, mouse drag and pressure
keys to record one C/E/G chord into clip 2. The saved schema-8 project contains 30 ordered
events with changing X/Y/Z on each generated note and balanced releases. Reopening retained
the phrase, and clips 1 and 2 looped together. The semantic inversion/register/spread controls
produced the independently expected `[76, 84, 91]` voicing while existing loops continued.
The local evidence project is `target/dev/firmware-harmony.crest-song`; native automated
witnesses provide reproducible assertions separately from this manual capture.

The Harmony panel also captures a custom voicing into the selected degree. In Play mode,
enable MPE input, hold 1–5 notes and choose **Learn held notes into selected degree**.
The lowest note becomes its root; unisons remain separate voices and pitch bends are not
baked into stored pitches. Release the fingers, then select Chords to play that entry.
Inversion, register, spread, articulation and expression still apply. **Restore selected
degree to diatonic** clears that entry. **Undo last clip or chord bank edit** shares the
library's single edit history; ordinary harmony settings are outside that history.
Armed or active recording rejects bank edits. Learning, restoring and Undo keep existing
notes and other playing clips intact. Bank entries persist in the unchanged project schema.
The native Sample witness verifies learning, restore/Undo, exact project reopening,
expressive chord recording and replay beside another looping clip, with no late events
or callback allocations/deallocations.

The macOS desktop also learned `[42, 43, 44]` from three latched firmware contacts while
the original clip looped, restored/undid the bank edit, then recorded that learned chord
with X/Y/Z from a single firmware trigger. The reopened project retained its bank and
21 ordered events with three balanced voice lifetimes; both clips launched together.
Local evidence is `target/dev/firmware-learned-chord.crest-song`. The app was saved and
closed with input and playback stopped.

Live chord pads support the library's delayed strum, repeating arpeggio and seeded humanized
articulations. The Harmony panel offers ascending/descending strum and arpeggio presets with
1/8-beat spacing (arpeggio gate 1/16 beat), plus humanization up to 1/16 beat with ±8 velocity
and seed 42. Imported settings retain their exact spacing, gate and seed until explicitly
changed. Manual strumplate remains immediate. Release cancels future tones; mode changes
cancel the live lane, including prepared attacks, while other clips continue.

The native clock advances quiet held gestures through a source horizon 20 ms behind the
rendered audio frame. This admits in-flight MIDI without scheduling into the input future;
packets older than that horizon fail explicitly rather than being retimed. Each generated
event uses its own musical timestamp plus the existing 50 ms monitor lead, so an input
packet arriving later cannot bunch earlier strum notes together. Recovery checkpoints are
dirtied by generated output even when no new MIDI arrives. The horizon is a bounded admission
policy, not a measured device-latency guarantee.

Native Sample and Braids witnesses hold a single gesture with no further packets, verify
all delayed chord tones sound, and check exact strum/arpeggio timing, expression on later
arpeggio voices, humanization bounds, release ownership, recovery prefixes and project
reopening. They replay alongside another clip and cancel waiting live tones without stopping
either clip. All 16 focused native performance witnesses pass with zero callback allocations,
deallocations and late events; 269 app tests and scoped lint also pass locally.

The optimized macOS desktop bundle also captured a held arpeggio from keyboard-controlled
virtual contacts through the pinned firmware and OS MIDI transport. The saved clip contains
349 attacks and 349 matching releases at exactly 120,000-tick intervals and 60,000-tick gates,
with independent pitch, timbre and pressure changes preserved. Reopening retained the phrase;
clips 1 and 3 then looped together and stopped cleanly. Local evidence is
`target/dev/firmware-arpeggio-release.crest-song` (SHA-256
`084816884d3f17a8acd34b0bd649032a9beed31b67d89d1b5a39668ce2082911`).
Debug desktop runs exceeded the remaining 30 ms scheduling margin and stopped input while
retaining the take. This acceptance therefore covers the optimized bundle, not arbitrary
control-thread stalls or the debug build. Deadline failures remain visible; notes are never
silently retimed to hide them. Deck and Linux clock correlation remain separate qualification.

Each player uses a disposable clip snapshot and the Song's tempo at start. Song and clip
playback share device-buffer-aware timing: publication starts beyond the callback's reserved
end, covers another whole device buffer, then adds a 50 ms launch or 100 ms scheduling margin.
Large audio buffers increase launch latency and lookahead; event positions remain exact.
Additional launches begin after already-published lookahead. Events merge chronologically on the
control thread, with stable slot order at equal timestamps and native voice identities unique
across all players and live input. Independent cancellation removes queued attacks and sustained
voices together without restarting another clip. The Synth's configurable voice-group and event
budgets bound admission; failures are explicit. Starting a clip preserves live monitoring and
recording. Opening its editor stops that clip only; edits affect the next launch, not another
player's snapshot. This does not layer clips over the separate Song-arrangement transport.
**Launch grid** cycles through immediate,
one beat and four beats, for workspace and virtual/physical pad launches alike. Quantized
launches align to a stable local grid whose beat zero is audio-stream frame zero; this is
not Song-arrangement synchronization. The library chooses the first grid point at or after
the safe scheduling time. Queued status follows the rendered audio boundary even when the
scheduler has already prepared future events. Changing the preference affects future
launches only. Stop or pressing the same queued pad cancels pending audio. The preference
is live application state, not persisted clip/project content. Tempo, graph or sample-rate
changes stop playback explicitly.
Desktop acceptance on macOS exercised the Sample example, immediate/one/four-beat
selection, queued-to-looping state and Stop. A virtual sensor pad launched the same
clip through stock firmware and CoreMIDI; decoded firmware output showed blue when
recorded, yellow while queued and green during playback, returning blue on pad stop.
The native scheduler witness also cancels a launch after future events are prepared,
proves silence past its former boundary, then relaunches three audible Sample voices
with zero late events and zero callback allocations/deallocations.
With MPE input enabled, **Capture last 4 beats to clip N** recovers up to four beats
since input activation, ending at the current audio boundary. It preserves free timing
and per-note expression; held notes close only in the copied clip. Original live notes
continue, and already timestamped packets before the cutoff remain valid. **Undo last clip
or chord bank edit** restores the previous clip. An armed/active take, interrupted input or expired
history rejects capture without changing the previous clip. Existing playback uses its
already launched snapshot until relaunch. Captured clips save with the ordinary project;
this operation does not arm recording or change the audio clock.
Desktop verification captured three held virtual-firmware fingers without arming Record,
undid and recaptured the clip while monitoring remained live, then saved/reopened and
launched/stopped it. The copied four-beat window retained each note's current expression
at its truncated start. A native Sample witness separately verifies curves inside the
window, delayed packets before the cutoff, exact project round trips and audible replay
with zero late events and zero callback allocations/deallocations.

The first output profile is lower-zone MPE with 15 members and a ±48-semitone range per destination;
only engines advertising proven owned-expression support are admitted (currently Braids and Sample).
With a selected LinnStrument input, **Enable LinnStrument MPE input** uses the released
adapter to monitor independently alongside a running clip. **Record clip** verifies sample
assets on a worker before arming the selected slot; wait for **ARMED** and release any held
notes before starting. The first member note starts capture. **Finish take and stop input**
commits the expressive take without stopping clip playback. Stop and Save also finish
the take. Discard leaves the previous committed clip intact. Relaunch a recorded slot
to hear its new take; an already running clip retains its original playback snapshot.

Input recording follows correlated source timestamps, with conversion error below one
library tick, and never follows the clip scheduling horizon. Monitoring uses a separate
ordered audio lane with a fixed 50 ms lead, not a measured latency claim. Quiet input advances
only through the 20 ms source horizon described above. Late input,
reordered timestamps, disconnects, overflow, tempo or sound changes stop input explicitly
and retain a take through its last accepted gesture. Input takeover releases default
channel-monitor voices first. The initial native path requires the existing CoreMIDI/
CoreAudio clock correlation on macOS. Linux input correlation remains implementation and
qualification work: the pinned CPAL ALSA stream and midir input queue report independent
relative clocks without a shared epoch. Capture rejects this configuration explicitly.
Native Sample witnesses cover concurrent clips and live voices, capture, project serialization,
reopened playback with live input and interrupted-take retention.

While recording, the existing recovery worker checkpoints a stopped copy every 250 ms,
using the native audio clock to retain elapsed holds even without new MIDI. The live engine
continues playing. Recovery can lose work since the last completed disk write; the cadence
is not a durability deadline. **Settings → Recording recovery** restores the checkpoint with
its sound graph, stopped and unsaved. Audition and Stop retain it; successful project Save
clears it only when that saved performance still matches current state. Discarding a later
take keeps preceding unsaved clips. Explicit document replacement or Close after the existing
save/discard decision clears the old checkpoint. Setup failure preserves the prior checkpoint.

Performance checkpoints use recovery format 2. Existing tracker-take checkpoints retain their
format 1 shape and remain readable; older readers reject format 2 explicitly. Recovery keeps
the composite Song/sound baseline from take setup plus the latest library performance; it is
not general Song autosave. Sample identities are verified before arming; moved or missing
assets still require repair. A native witness reads an unfinished three-note take from disk,
restores independent expression, plays it through Sample, then proves Save clears recovery.
**Refresh MIDI feedback destinations**, connect the matching LinnStrument feedback port,
then select **Clips mode** for its 128- or 200-pad geometry. Input and output endpoints are
selected explicitly; the stock firmware's NRPN mode report must arrive before pad input
is accepted. A once-per-second mode query detects firmware resets; three seconds without
a report interrupts control input and starts restoration. The bottom playable row launches recorded clips, and pressing the playing
clip stops it. This uses the same host launch path as the workspace. Switching to Clips
finishes an active take and releases live monitor voices. Multiple slots can play together,
including slots routed to the same native Patch, with independent queued/playing LEDs.

LED intent travels through the output worker and the firmware input parser. Blue means
recorded, yellow means queued, green means playing; the simulator renders decoded firmware SPI output. Complete
cell updates are coalesced, with at most eight cells every 34 ms after due musical output.
A claimed feedback port rejects musical routing until released. Stop retains clip controls;
**Return LinnStrument to Play**, document replacement, input loss, output disconnect and
shutdown restore stock Play. Mode restoration bypasses a full musical queue. Connected
close waits for firmware acknowledgement; a timeout keeps the app open with a visible
recovery action. Reconnecting requires an explicit feedback selection and full repaint.
The simulator must run with `LINNSTRUMENT_VIRTUAL_MIDI=1`; its internal test host then stays
out of the external host's musical and mode ownership.

Native desktop verification has captured an independent three-finger phrase from the
simulator's actual firmware, saved/reopened it in Tracker, launched/stopped it by clicking
a virtual pad, observed the decoded LED change from blue to green and back, and restored
the stock playing display. This is virtual validation, not physical-device qualification.
The independent clip-bank build also reopened two firmware-recorded clips, launched both
from virtual pads, displayed both firmware LEDs green, then stopped one with its pad while
the other stayed looping and green. Global Stop and firmware-confirmed Return to Play restored
the stock layout. Native Braids spectral checks and resident Sample witnesses separately
verify selective sustained/pending cancellation and unchanged surviving expression; live
capture can retain three independent notes alongside two three-note clips.
**Edit clip** opens the existing native piano-roll timeline over the library's note
projection. **View** switches between Piano roll and Tracker without changing the clip.
Tracker shows one row per note with exact start and duration, velocity and expression-point
count; simultaneous attacks and repeated pitches remain separate rows. Selection survives
view changes. Select a span, a Tracker cell or Previous/Next, then edit pitch, exact start
or duration.
The quarter-beat ruler is visual: controller timing nudges add 1/16 beat to the existing
time without snapping, and text accepts exact beat fractions representable at 960,000
ticks per beat. Moving or repitching retains every expression sample, including samples
at release. Shortening explicitly removes samples at or beyond the new end; **Undo clip
edit** restores the complete prior clip. Editing stops that clip first, and playing clips
reject edits until stopped. The editor uses stable note identities, rejects stale commands
and saves through the existing project lifecycle. It currently edits existing notes;
adding/removing notes, expression-curve tools and conversion into Song phrases remain work.
**Quantize clip attacks** explicitly moves whole note lifetimes to the nearest 1/4 beat,
1/2 beat or beat using the pinned library command. Durations, velocity, expression offsets,
polyphony and loop length stay exact; the library chooses an earlier feasible grid point
when moving later would cross the loop end. Undo restores the original performance.
Applying the same grid again preserves the previous Undo when no notes move. Desktop
verification quantized the expressive Sample fixture, checked the saved event offsets,
restored free timing with Undo, and rejected quantization during playback.
**Set playback · loop N of P** assigns a periodic rule to the selected note through the
pinned library: `4/4` plays on loops 4, 8, 12 and so on; `1/1` restores every loop.
The rule gates the complete note lifetime, including its expression and release. Other
notes keep their own rules. Raw events stay unchanged, save/reopen preserves the rule,
and Undo restores the prior clip. An unchanged rule preserves Undo; stale commands and
edits during playback are rejected. Imported Fill/Not Fill rules are displayed, but a
held Fill control is not implemented. Desktop verification exercised rule entry, no-op,
Undo, playback rejection and save/reopen; deterministic playback tests independently
check attacks on loops 4 and 8 and exact expression for each admitted note.
The clip's Tracker view does not import, quantize or replace the Song's arrangement.
Native Sample witnesses cover exact movement, Undo, repitching, save/reopen and audible replay.
Desktop verification exercised keyboard editing, unchanged expression counts, playback
rejection while editing, Undo's clean state and reopening the saved repitched clip.

External performance routes, Linux input clock correlation and Steam Deck qualification
also remain integration work. The standalone application retains its complete
firmware-driven workflow.

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

On Linux, `python3 scripts/linux_midi_clock_probe.py` measures ALSA queue timing
using private virtual ports. It needs a C compiler, `libasound2-dev`, loaded
`snd-seq`/`snd-hrtimer` modules and access to `/dev/snd/seq`. The separate Linux
MIDI clock feasibility workflow runs the same probe. Its retained trace checks
delayed reads and queue restart; a passing probe does not qualify live MIDI/audio
correlation or physical hardware. Hosted CI boots a generic Ubuntu kernel under
QEMU because the runner's Azure kernel omits ALSA sequencer modules; the guest
has no network or hardware devices and only its evidence directory is writable.

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
