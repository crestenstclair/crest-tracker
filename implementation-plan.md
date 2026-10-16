# Crest Tracker full implementation plan

16 October 2026 · Product scope retained; recovery now uses an in-repository Synth fork. Current behavior and remaining gates are in README.md.

Build a controller-first desktop music application using Crest Synth as its working base and source dependency. All existing Synth functionality must be available in the integrated application, including its actual UI and audio rendering implementations. Deliver a complete path from an empty Song through composition, expressive recording, arrangement, sound editing, saving, reopening and performance. Tracker's composition/playback library remains strictly separate from Synth. The user confirmed full Synth reuse and a hard library boundary, then authorized the in-repository fork recovery.

Approval of this plan means implementing through M10, not stopping after an audible phrase or UI skeleton. M8 is the first broad usability checkpoint: the core song-making workflow should be mostly working there. M9 completes external synchronization and control-surface configuration; M10 proves and packages the result. Milestones describe dependencies and acceptance, not calendar promises.

The product specification and live Figma remain the product authorities. This plan supplies implementation decisions, execution order and evidence. Defaults explicitly proposed below become implementation defaults when accepted; they do not retroactively become confirmed user requirements. The architecture guide's phase numbers remain conceptual groupings; the milestone crosswalk below defines execution order.

Full Synth reuse and a hard Synth/sequencer boundary are confirmed. Section 3 defines the chosen dependency/co-development approach. Remaining proposed defaults include macOS first, the musical defaults in section 5, and exact resource references plus Collect Assets. The rest of the plan turns requirements into delivery gates. No recurring approval is needed for ordinary implementation choices covered by the accepted plan.

**Current scope override (16 October 2026):** recording setup and physical capture acceptance are deferred. Expanded view and its View Group UI are dropped. Minimap entry points are removed from the UI; retain its implementation. Retain existing saved data and backend compatibility; do not expose unfinished recording controls. Complete the remaining editing, settings, chord, generator, file/library and MIDI configuration workflows through the shared Synth shell. README.md records current behavior and outstanding evidence; the milestones below retain historical dependency context, not a claim that every physical or visual gate has passed.

## 1. What the completed application must do

The primary acceptance journey is:

1. Launch directly into an empty Song. Select a sound using the existing Synth catalog, then create a chain and phrase using a controller.
2. Enter notes, explicit instrument/channel targets, velocity and three FX slots. Change length, division, scale and meter; hear the result immediately through Synth.
3. Build a song across independently advancing tracks. Reuse a phrase, make one occurrence unique, and undo without changing the others.
4. Edit and quantize Piano Roll notes and expression with the controller. Recording setup and physical capture acceptance are deferred; preserve existing capture data and backend behavior.
5. Use Drum, Euclidean and Random forms, literal chords, inversions and spread through the same playback engine.
6. Move between Song, Chain, Phrase and Minimap without changing music or interrupting transport. Edit instruments, samples, effects, mixer and sends in the same application.
7. Save, close and reopen the exact Song, sound revisions, recorded expression and local mix. Missing resources produce actionable recovery rather than substituted sounds.
8. Stop, seek, unplug a device, interrupt a save or exhaust an admitted queue without corrupting the Song or leaving owned notes and overrides active indefinitely.
9. Configure controllers and external MIDI clock, then run a packaged macOS application outside either source checkout.

The first supported desktop is the current Mac. Preserve Linux-compatible boundaries and run available Linux checks before claiming Linux support. Windows packaging is a follow-up, not an implied promise from using Tauri. Ship a local application: no account, cloud service, collaboration server or subscription infrastructure.

Full M8 command parity, exact OXI algorithms, automatic voicing, comp lanes, replace recording, arbitrary SysEx recording, MIDI 2.0, plugins, audio recording and audio/stem export are outside this implementation. Ordinary MIDI output is included. These exclusions do not remove any confirmed Tracker requirement.

## 2. Starting point and reuse map

At kickoff, Tracker contained product, architecture and design artifacts, with no Cargo workspace, executable or tests. Synth is a Rust 2021 library and standalone Tauri 2 application, using CPAL, SDL3, midir, serde and prepared native DSP. The inspected Synth revision is 1a6b0ad9a18a817572ae9641482b11194759a635. This is source evidence, not a fresh test or physical performance result.

| Responsibility | Existing evidence | Implementation decision |
|---|---|---|
| Canonical Synth state | [AppState](crates/crest-synth/src/control/app_state.rs), [AppLoop](crates/crest-synth/src/control/app_loop.rs) | Keep Synth edits in its reducer. Do not duplicate Patch or mixer models in Tracker. |
| Direct sound targeting | AppEvent::Midi carries PatchId; AppLoop::dispatch_midi_from separately fans out channel messages | Map a Song-local installed instrument identity to a Synth PatchId. Specific-instrument targets must not accidentally use channel fanout. |
| Audio commands | [AudioCommand](crates/crest-synth/src/real_time/audio_command.rs), [AudioRenderer](crates/crest-synth/src/real_time/audio_renderer.rs) | Add generic scheduled delivery. Current commands lack time fields and are drained before each block renders. DESIGN.md's statement about block-relative offsets is not proof that this exists. |
| Graph lifecycle | Prepared graphs, graph preparation worker, structural boundary and activation acknowledgement | Reuse off-thread preparation, acknowledged activation and off-thread retirement. Never install assets from playback or a callback. |
| Physical MIDI | [midi_device.rs](crates/crest-synth/src/control/midi_device.rs), [midir adapter](crates/crest-synth/src/adapter/midir_input_device.rs) | Reuse connection and bounded-ingress patterns. Capture needs a lossless performance path before lossy normalization and fanout. |
| External MIDI output | The inspected Synth adapters provide input and fixture playback, not a reusable physical output endpoint | Implement host-owned discovery, output connection, scheduling and disconnect recovery. Reuse midir where sufficient; use a native timestamped backend only where measurement requires it. |
| MIDI normalization | MidiMessageKind lacks polyphonic key pressure; normalize_channel_message rejects it and collapses CC120/123 | Add/retain required capture messages and original values. Existing normalized playback input is not the complete recording source. |
| MPE | Bend, channel pressure and CC message kinds exist | Per-member behavior, master controls, bend ranges, sustain and allocation require engine-specific proof. A message enum is not an MPE implementation. |
| Temporary sends | Normal Synth control edits affect saved sound/mixer fields | Add a Synth-owned effective-control layer over saved baselines. Temporary FX must never change the saved mixer value. |
| Saved sound state | [SavedSession](crates/crest-synth/src/control/saved_session.rs), schema version 4 | Keep serialization owned by Synth. Add reusable sound-definition capture/install; session capture alone is not a revisioned sound library. |
| Save/replacement safety | [session lifecycle](crates/crest-synth/src/shell/session_lifecycle.rs), save and candidate workers | Reuse atomic-write and correlated-replacement patterns for one composite Song document. No competing File menus. |
| Controller and focus | SDL3 discovery, translators, controller preferences, semantic focus and native key capture | Reuse physical mapping and focus behavior. Tracker introduces semantic editing actions, not raw device branches in domain code. |
| Presentation | [AppWindow](crates/crest-synth/src/shell/app_window.rs), [webview-page](crates/crest-synth/webview-page), semantic projections | Reuse the actual renderer, components, layout, tokens, meters, waveform/playhead display, focus and native input paths. Expose a narrow hosting seam in Synth; do not replace existing pages with Tracker approximations. |
| Assets and DSP | Installed registries, Sample/SoundFont browsers, preparation and bundled resources | Reuse these capabilities. Do not rebuild DSP, sample importers or SoundFont parsing. |
| Proof tooling | [Makefile](crates/crest-synth/Makefile), native witnesses, performance runner and callback guards | Extend production-path witnesses. Reducer tests alone do not establish playable audio or usable screens. |

Synth's DESIGN.md reports remaining visual/controller/package gaps and stress cases over callback budgets. Preserve working behavior, but measure the actual Tracker workload instead of assuming every existing path is fast enough.

## 3. Architecture and repository layout

The recovery replaces the dependency-only shell with the complete working Synth application in this repository:

- `crates/crest-synth`: the Synth fork, including its original executable, renderer, controllers, native DSP, assets, licenses and tests. It has no Tracker dependency.
- `crates/crest-tracker`: the existing pure composition, editing, playback, recording and harmony library. It has no Synth or platform dependency.
- `crates/crest-tracker-app`: composition root, Song lifecycle, scheduling bridge and Tracker workspace reducer/projection. It depends on both libraries.

`make dev` runs Tracker. `make synth` runs standalone Synth. Changes to both live in one Git diff and can be committed atomically. The sibling checkout is no longer a runtime or build dependency. `crates/crest-synth/FORK.json` records the source commit and hashes of the imported working tree, including its uncommitted integration fixes.

Synth owns SDL discovery, saved physical mappings, chord/hold interpretation, native keyboard capture, the Tauri window and rendering components. Both physical input paths route semantic actions to the active owner. Tracker contributes typed workspace projections and receives validated semantic events; it supplies no second HTML page, stylesheet, JavaScript application or controller mapper. Hosted Shift+Back opens the application menu; standalone Synth keeps its original bindings.

The host owns one Song lifecycle and musical transport. Tracker edits go through its application commands and pure Editor. Synth changes go through `AppState::apply` and `HostedSynthControl`. Neither library imports the other. Open validates the composite candidate and required Patch identities before replacing the active sound and composition.

The pre-recovery implementation remains at commit `0d85318cc63f63c8bce830dc2b0e5af43c0784fa`. Song format version 1 and older machine preferences remain readable; no saved music or preferences are deleted. Song rollback needs no conversion. New optional browser-directory preferences may be rejected by older binaries with strict unknown-field validation; restore their prior preference file when rolling back. Keep Synth assets under their inherited LFS rules and run `git lfs pull` in a new checkout before native audio/package checks.

### Hard boundary and behavior ownership

Sequenced notes, performance expression and musical MIDI controls use Synth's normalized MIDI boundary. Resource loading, state capture and graph installation use its existing lifecycle boundary.

Reuse existing Synth pages through their native semantic actions and reducer. These are Synth-owned UI operations inside the Synth side of the boundary, not sequencer access to sound internals. The host composes the two libraries and forwards their public commands/projections. Tracker composition never imports Synth or reaches into its Patch, voice, DSP, mixer or renderer internals.

Treat feature envy as an ownership defect: if the host adapter starts reading several Synth internals, reproducing its validation or orchestrating sound changes field by field, move that behavior behind a cohesive public Synth operation. Conversely, Synth never acquires phrase, chain, quantize, Song cursor or sequencer-FX rules. Sound rendering, capabilities, sound editing, preparation and native sound serialization belong to Synth; composition, musical scheduling and recording/editing semantics belong to Tracker; device-clock translation, target mapping and cross-document coordination belong to the host. Add a real public capability when needed instead of a broad getter/setter escape hatch or a Tracker-specific Synth API.

Enforce the boundary with package dependencies, restricted host integration imports and production-path contract tests. Change both sides freely in one development session when the responsibility requires it. Preserve the standalone Synth application and use it as a regression witness.

Temporary musical sends still need an explicit Synth-owned control profile. Saved baselines and transient effective values remain separate inside Synth. The host may attach generic source/lease identity and timing to delivery; no phrase, chain, tracker FX opcode or Tracker dependency enters Synth. External destinations receive only supported MIDI controls and require known restore values.

Do not split the entire Synth shell preemptively. Expose the narrow projection, action and host-lifecycle seams used by both apps. Extract actual shared presentation where useful; keep Tracker-only pages here. A coherent host-composition refactor is preferable to a maintained fork of StandaloneApplication or page.js.

### Full Synth parity requirement

M0 captures the actual current Synth feature inventory from its registries, source and witnesses. Preserve every existing instrument/effect, native asset/import format, Sample/SoundFont/Drum Rack/DX7 workflow, parameter/editor surface, mixer/send operation, controller/device setting, preview, native session capability and diagnostic command. The inventory follows production functionality rather than the examples drawn in Figma. Newly added generic Synth capabilities remain reachable through the same registry-driven integration.

The first integrated build hosts the complete existing Synth workspace using its actual implementation. M7 completes Song-specific integration and rechecks parity; it does not rebuild missing Synth features later. Retain native Synth session import/export through its serializers alongside the combined Song lifecycle, with explicit sound-replacement/reference validation. Keep standalone test-pattern/demo behavior available without making Tracker's new empty Song autoplay. These host-context adjustments preserve the underlying capability.

Completion requires feature-inventory coverage and retained renderer/input/audio witnesses in both standalone Synth and the integrated host. Mere dependency linkage, presence in a menu or a hand-picked subset of instruments is not full reuse.

## 4. State and behavior contracts

### Persistent and transient ownership

| Owner | Contents | Must not contain |
|---|---|---|
| Song document | One Song; up to 16 tracks; Chain/Phrase maps; stable use IDs; NoteData; generator settings; key, initial tempo and meter map; pinned revisions; Synth-native saved sound/mix payload | Runtime cursors, active voices, device handles or a collection of Songs |
| Playback runtime | Transport epoch, tempo segments, occurrence paths, active snapshot, seeds, output horizon, notes, leases and overrides | Mutable shared definitions or UI focus |
| Recording session | Armed occurrence, input mapping, timestamp anchor, take identity, held input, draft and interruption state | A second committed note store or live state inside saved Song content |
| Editor state | Stable focus, selection, popovers, scroll, zoom, display mode and session shared-edit preference | Competing copies of composition content |
| Machine preferences | Controller mappings, device bindings, default BPM, browser roots and hardware resource budgets | Portable absolute paths inside the Song |

A sound library resource, its pinned revision, its installed use in a Song and the runtime Synth PatchId are distinct identities. Two installations of one sound can have independent subscription, routing and mix. Events target the intended installed use; that use pins its library revision. Changing a pin is one validated transaction. This resolves the resource-reference shorthand in A05 without collapsing separate installations into one target.

### Editing and undo

Semantic input resolves to a typed command with stable IDs and expected revisions. Validate first; commit once; invalidate dependent compiled content; project the accepted result. Rejected commands leave document/history unchanged. Undo covers musical edits, reference relinking, generator changes, quantize and one complete take. Coalesce held-button adjustment into one deliberate edit gesture. Navigation, playback ticks and meters do not create undo entries.

Shared edits inspect the current complete reference topology. Make Unique can copy a shared containing Chain before copying/relinking a Phrase. Validate the operation at commit, preserve resource pins and undo it atomically. Remember Edit Shared/Make Unique for the current session only.

### Exact time and output ownership

Use checked reduced rational quarter-note beats with overflow-safe intermediates. Do not derive musical time by accumulating floating-point UI frame durations. Tempo segments map beats to monotonic host time and audio sample positions; conversions round only at destination resolution. Preserve timestamp provenance and raw MIDI values for recording.

UI channels display 1–16; adapters translate once to the backend's numeric convention. MIDI pitch stays 0–127 internally while editors show note names. Percentage displays never round-trip untouched recorded velocities or controllers through a reduced-precision value.

One scheduler resolves every phrase form. It advances half-open windows, evaluates stable occurrence cursors, applies typed FX and produces ordered events. Compilation and preview happen off the audio callback. Live edits activate at a declared future boundary beyond delivered output. Failed compilation leaves the last valid snapshot active and visibly identifies the rejected edit.

For internal audio, add generic absolute-sample scheduling with bounded pending storage, future-event retention and delivery at offsets within a render block. Preserve immediate events for existing Synth callers. A distant future event must not block urgent releases at the head of a queue. Merge producers before the SPSC boundary or use explicitly separate queues, and reserve cleanup capacity. Graph revision and device-clock generation accompany relevant routing/timing decisions.

Ordering at equal times is explicit: epoch invalidation and owned cleanup first; required configuration and initial controllers before attacks; old releases before replacement attacks; note-relative expression in source sequence order. Slot order resolves conflicts within a declared FX stage. An attack's eventual release retains its original resolved destination and ownership even after edits.

Stop/seek invalidates unsent work and releases owned notes/controls. Internal queued events require epoch/source invalidation at consumption so stale attacks cannot execute after Stop. External packets already handed to an OS/device may be uncancellable; bound the delivery horizon and expose this constraint. Panic is an explicit whole-destination operation, not normal recording cleanup.

### Recording and MPE

Capture input before Synth normalization loses polyphonic pressure or original controller identity. Preserve source session, channel, timestamp and sequence, with bounded queues and typed overflow. One connection routes deliberately to recording, monitoring and optional ordinary Synth subscriptions; the same input must not reach Synth through a second automatic fanout.

Implement the full [Piano Roll contract](design/piano-roll.md), including initial controller state, physical key release separate from sustain, loop-crossing duration, raw pitch, release velocity, master/member expression and ordered RPN/NRPN. Capture and output clocks need a measured mapping; midir timestamps alone do not establish it.

One allocator owns each destination's member channels across monitoring and every playback occurrence. Independent master bend/sustain timelines require separate zones/endpoints or a proven isolating profile. Exhaustion rejects a new attack visibly, never steals another owner's member. Internal Synth bookkeeping must distinguish channel/attack identity where required; Patch-plus-pitch-only tracking cannot represent simultaneous same-pitch MPE voices correctly.

Provide a tested built-in MPE destination profile if a selected installed engine can faithfully support the required dimensions. Otherwise extend the narrow Synth-owned receive/control path and existing engine adapter before claiming that profile. Preserve unsupported engine distinctions. External MPE capture/replay uses a validated endpoint profile; this is not evidence that every internal instrument supports MPE.

## 5. Proposed defaults that unblock implementation

These are reviewable defaults, not invitations to reopen confirmed scope during every milestone. Record an evidence-driven change in open-decisions.md when implementation disproves one.

| Area | Proposed initial behavior |
|---|---|
| New work | Empty Song, 120 BPM, 4/4, C major. New phrase: 16 sixteenth-note steps. Machine default BPM initializes new Songs; current tempo and saved initial tempo remain separate. |
| Phrase limits | Positive exact musical length. No arbitrary product cap on rows, notes, instruments or voices; validate explicit resource budgets and virtualize long views. Confirmed 16-track Song limit remains. |
| Divisions | Quarter through 1/64, with straight, triplet 3:2, quintuplet 5:4 and septuplet 7:4 presets. Validate explicit positive ratios. Budget/overflow failures reject explicitly. |
| Authored duration | Default gate is one step, with release before a same-time retrigger. Empty rows do not cut. Off/cut is explicit. Recording keeps measured duration. |
| Duplicate MIDI 1.0 pitch | Release older owned attack before same-address/same-pitch retrigger; stale offs cannot release its replacement. Repeated input tuples use documented FIFO pairing including retired attacks and show ambiguity. |
| Meter changes | Bar boundaries only in the initial editor; preserve absolute note times. Denominator is a positive power of two with explicit grouping. Invalid placement rejects atomically. |
| Division edits | Explicit retiming command with preview and undo. Piano Roll grid/zoom never retime notes. Length reduction previews trimmed notes/controls and commits as one reversible edit. |
| Pitch policy | Scale-only authored pitched entry; fixed drum triggers exempt. Scale changes preserve authored pitches and mark out-of-scale notes. Notes/Intervals changes display only. |
| Scale catalog | Major, natural/harmonic/melodic minor, seven church modes, major/minor pentatonic, blues, whole-tone and chromatic; explicit custom pitch-class sets. Stable IDs and spelling metadata. |
| Flow | Empty Song slot stops its lane. Explicit repeat counts/loop ranges. Jumps follow a positive-duration step; Stop wins. Simultaneous tempo conflicts use lowest track order, then last slot, with a diagnostic. |
| Live edits | Step edits adopt at the next eligible step; structural/generator edits at the next eligible phrase boundary. Both are beyond delivered output and visibly pending until active. Seek explicitly cleans up/repositions. |
| Generators | Deterministic by saved seed, revision, scale, occurrence, cycle and event key. Regenerate/reseed/bake are explicit commands; view switching does none of them. |
| Recording | One armed occurrence, fixed-length loop, raw additive overdubs, no count-in. Arm does not record. Record starts at the next eligible phrase start. Monitor is explicit and initially off until setup is validated. End take commits one undo action and leaves accompaniment running. |
| Quantize | Selected starts only; nearest grid point, midpoint later; duration and relative expression offsets preserved. Duration quantize is explicit. No pitch quantize or automatic input snapping. |
| Shared edits | Prompt before editing/arming. Remember choice for the session. Revalidate references at take commit. |
| Navigation | Song Shift+Down opens Mixer; every other view opens Quick Jump. Existing Shift/Edit conventions and stable return focus prevail. All actions, text entry and recovery are controller-reachable. |
| Minimap | Vertical musical time, horizontally windowed lanes, activity density, phrase boundaries, focused occurrence and independent playheads. Explicit zoom/follow. Finite preview marks uncertain future flow. |
| Clock loss | One master: Internal or one selected input. Loss beyond max(500 ms, four measured pulse intervals) stops safely, preserves a recoverable take and shows loss. Reconnect never resumes automatically. |
| External routing | Persist logical endpoint identity and show unresolved bindings. Never silently bind to a similar device. Clock/performance echo is off by default. |

### Chord behavior and fixtures

Start with major, minor, diminished, augmented, sus2, sus4, major/minor sixth, dominant/major/minor seventh, half-diminished and diminished seventh, plus user interval sets. Default-bank derivation tests catalog pitch classes against each scale root; unusual scales show honest interval-set names rather than forced major/minor labels. No arbitrary chord-bank count limit.

ADD accepts selected semitone intervals from -12 through +12 around the root. CRD references an immutable shape revision. VOI and SPR apply after the shape exactly once. Custom definitions retain shape, versioned voicing/spread settings and a resolved verification snapshot; recall must not apply transforms twice.

Proposed open voicing rule: after the selected inversion is sorted, raise the second-lowest voice by one octave and sort again. For fewer than three voices, Open is unavailable. With C minor rooted at C4, version-1 fixtures are:

| Preset | Exact pitches |
|---|---|
| Root / closed | C4, Eb4, G4 |
| First inversion | Eb4, G4, C5 |
| Second inversion | G4, C5, Eb5 |
| Open | C4, G4, Eb5 |
| Open first | Eb4, C5, G5 |
| Open second | G4, Eb5, C6 |

SPR is the count of added octave-doubled voices. Append the lowest unused higher-octave chord tones above the current highest pitch. Root C minor plus SPR 1 is C4, Eb4, G4, C5. Reject range/capacity failures with a preview; do not silently drop voices. These are Crest definitions, not exact OXI behavior. Manual voicing ships; Auto remains outside this plan.

### Initial FX catalog

Every descriptor defines value type, range, stage, target, scope, duration, conflict rule and capability. Three slots are always the limit. Recorded channel/master controls are performance data, not extra slots. Check displayed abbreviations against current Figma screens before wiring them.

| Family | Required effects and semantics | Delivery |
|---|---|---|
| Pitch | OFS signed semitones with MIDI-range validation; no later scale correction | M2 |
| Dynamics | Velocity, positive gate fraction, explicit cut/off | M2 |
| Time and chance | Rational positive delay, bounded retrigger count/spacing, 0–100% probability seeded by stable event identity | M2 |
| MIDI controls | CC number/value and exact 14-bit bend, explicit target/scope; empty-row controls execute | M2 |
| Temporary controls | Send amount and step/occurrence lifetime; override stack over latest manual baseline; known restore value externally | M2 |
| Local flow | Phrase jump and positive BPM tempo change; positive-time progress and conflict policy | M2 |
| Harmony | CRD, ADD, VOI and SPR with literal pitch fixtures | M4 |
| Arrangement flow | Chain and track-arrangement jumps/repeats through the same cursor/cleanup contracts | M5 |

Delay/retrigger/gate transform note and owned expression together. Probability suppresses generated notes and their expression, never mandatory cleanup or unrelated global controls. Unsupported expression/FX combinations fail compilation visibly and preserve the prior valid snapshot. Review official M8 documentation for selected analogues during M2; document Crest differences instead of promising command parity.

## 6. Milestones and completion gates

Each milestone ends with a runnable application or focused executable witness, exact commands and a short evidence record. Continue through milestones once approved. Do not build large UI surfaces ahead of working behavior. Drawn controls and mocks cannot satisfy a gate.

### M0 Establish the real integration boundary

Dependencies: none. Maps to former Phase 0.

1. Record both repository baselines. Run relevant existing Synth build, MIDI, controller, lifecycle and callback-safety checks. Distinguish pre-existing failures from regressions.
2. Create the three-package workspace and host composition root based on the complete in-repository Synth fork. Establish inward-dependency checks and reproducible clean-checkout builds.
3. Prove direct Patch targeting and channel fanout separately through AppState/AppLoop and the actual renderer. Produce rendered note-on, release and one supported control, not just queued messages.
4. Add generic timed MIDI delivery, clock observation, bounded pending storage and sample-offset rendering in Synth. Preserve immediate callers and graph safety. Prove future retention, ordering, late-event diagnostics and epoch cancellation.
5. Expose the native presentation seam and host the complete existing Synth workspace without duplicated product state or renderer code. Record its feature inventory and native rendering/input witnesses. Confirm host resource lookup.
6. Inventory expression capabilities, input timestamp origins, temporary-control ownership, sound-definition export/install and package licensing/resources. Record exact supported profiles and gaps.

Establish the physical output adapter contract here, including backend timestamp support, monotonic-clock conversion, endpoint identity and disconnect behavior. Internal audio and external MIDI must consume the same musical output, with destination-specific delivery resolution.

Gate: pure Tracker builds without platform/Synth dependencies; standalone Synth remains independent; the integrated base preserves its existing feature inventory and rendering; local Synth edits are exercised by Tracker without publication; a clean checkout builds from its own Synth source and materialized assets; note/release/control render through the real boundary; scheduled attacks land within one sample of intended offsets; RT guards remain clean. New Tracker capability gaps receive owned M2/M3/M6 tasks, not invisible TODOs or permission to drop existing Synth features.

Artifact: verified capability matrix, baseline commands/results and one integrated executable. No full shell rewrite.

### M1 Implement the musical document and durable editing foundation

Dependencies: M0. Foundational Phase 1 work; brings Phase 5 serialization forward.

1. Implement validated IDs/revisions, beat/division/meter/tempo types and one Song with Chain/Phrase maps and stable occurrence identities.
2. Implement common NoteData: exact note velocities/targets, channel/master controls, note-relative expression and three typed FX slots. Preserve unsupported capability identity for inspection/recovery.
3. Add command validation, transactional editing, dirty tracking, undo/redo and stable focus. Add reference lookup and atomic Make Unique before recording depends on it.
4. Add scale lookup, pitch entry/display policy and a read-only host sound catalog. Preserve distinct installed uses of the same resource.
5. Implement initial versioned Song encoding and atomic Save/Open of minimal composition plus Synth-native saved payload. Validate size, references and ranges; serialize integer/rational values exactly.
6. Add immutable execution snapshots and fake-clock/capture-sink harness. Compiler failures preserve the previous valid snapshot.

Gate: exact save/reopen of authored and expressive fixtures; stale edits reject atomically; Make Unique/undo preserves other uses; invalid IDs, ratios, references and unsupported schemas fail safely. Core tests require no desktop/audio hardware.

Artifact: minimal persistent Song, reproducible pure-core tests and behavioral fixtures.

### M2 Ship an audible editable phrase with real FX

Dependencies: M1. Completes former Phase 1.

1. Implement occurrence cursors, scheduler windows, tempo conversion, ordering, chance and note ownership. Run simultaneous uses of one phrase through one engine.
2. Implement all M2 FX, including FX-only rows, tempo/jump, CC/bend and temporary sends. Add Synth's effective-control layer/profile; manual edits update baseline beneath active overrides.
3. Build phrase grid, note/target/FX/value popovers, settings and Notes/Intervals projection. Every displayed field edits actual state.
4. Wire SDL/keyboard semantic input, held Edit/Shift, repeat, focus-loss cleanup, transport and explicit panic. Include controller text entry wherever names/numbers require it.
5. Provide minimal Song landing and navigation into a phrase. Include one safe bundled sound and an optional example phrase; New Song stays empty.
6. Expose New/Open/Save/Save As with dirty guards. Add periodic recovery snapshots outside playback; full resource portability follows in M6.

Deliver external endpoint/channel selection and actual MIDI output in this milestone. Validate through virtual loopback before hardware, including owned note releases, output saturation and disconnect. A capture sink alone does not satisfy external output.

Gate: controller edits change audible notes and three FX slots; length/division/scale work; 16 sixteenths and 12 eighth-triplets meet at beat 4 over thousands of cycles; two instances isolate cursor/FX/RNG; display changes preserve MIDI; stop/seek releases notes and restores latest baseline. Save/reopen reproduces output.

Artifact: first musically usable phrase app. Native input/paint witnesses cover empty, editing, playing, popover and error states.

### M3 Record and edit expressive Piano Roll performances

Dependencies: M2. Former Phase 1R, before secondary generators.

1. Implement timestamped capture/clock correlation, channel/zone routing, explicit monitor and arm/wait/record/end/interrupted states.
2. Capture ordinary MIDI including polyphonic pressure, original controller values and ordered parameter messages. Add required generic Synth receive coverage without weakening diagnostics.
3. Implement MPE master/member state, bend ranges, global leases, scoped sustain and initialization before attacks. Prove a declared destination profile end to end; label unsupported engines precisely.
4. Record raw fixed-loop overdubs across tempo/loop changes. Pin the armed occurrence despite navigation. Monitor once; committed content activates at a future eligible loop.
5. Implement every roll command in design/piano-roll.md: overlapping-note selection, insert/delete, time/pitch/length/attack/release/target edits, ranges, copy/paste/duplicate, expression lanes, quantize, undo/redo and zoom/follow.
6. Add recoverable take checkpoints, conflict-preserving commit and Save as new phrase recovery. Save during capture finalizes before document capture. New/Open cancellation preserves drafts.

Gate: all eight Piano Roll scenarios pass through capture, compilation and delivery. Quantize preserves expression ownership; sustained members cannot be reused early; monitoring plus two occurrences allocates without leakage. Overflow/disconnect preserves data and never rearms automatically. Physical MIDI and supported MPE-device/endpoint evidence remain separate gates.

Artifact: controller-complete recording workflow and raw-value/recovery fixtures. If hardware is unavailable, complete deterministic and virtual-loopback proof, retain physical validation as pending and continue independent milestones.

### M4 Complete harmony and the other phrase forms

Dependencies: M2 and M3's common expressive editing contract. Former Phase 2.

1. Implement scales, derived Default banks, chord search, editable Custom revisions, copy/save and held-Edit chord navigation.
2. Implement CRD/ADD/VOI/SPR, exact fixtures and versioned recall. Preserve three slots and phrase chromatic entry when OFS has no free slot.
3. Implement Drum lanes with fixed trigger/target mappings, velocity and shared FX. Use the provisional grid unless the intended dedicated reference becomes available.
4. Implement Euclidean pulses/steps/rotation, target/pitch/velocity/gate. Define pulse placement and rotation with exact fixtures.
5. Implement Random scale-constrained pitch range, density/probability, velocity/gate ranges, target and deterministic seed. Reject empty pitch candidates visibly.
6. Separate authored content from generator recipes. Preview/playback share resolution. Regenerate/reseed/bake and generator conversion are explicit non-destructive commands; opening a view never flattens a take.

Gate: C major remains C–E–G in C minor; Default fits each supported scale; Custom recalls exact pitches once; generation is invariant to window partitioning; all five forms share timing, targets, expression safety and FX.

Artifact: usable provisional Drum/Euclidean/Random layouts using existing design vocabulary, with visual refinement isolated from musical behavior.

### M5 Complete chains and independent Song arrangement

Dependencies: M2–M4. Former Phases 3/4, excluding richer projections.

1. Implement Chain/Phrase use creation, reuse, ordering, repeat/removal, Shared References and complete Make Unique UI.
2. Implement 16 independent Track cursor stacks, chain/arrangement flow, lane stop/loop policies and bounded transition faults.
3. Build empty/stopped/playing Song/Chain screens, horizontal track windows, multi-select, arrangement editing and scoped transport. Complete main menu and Quick Jump navigation.
4. Integrate Piano Roll into the full Song, including one armed loop while other tracks advance independently.
5. Preserve live activation, stable focus and undo across structural edits. Deleting active occurrences invalidates and cleans up affected owners only.

Gate: compose/play a multi-track Song with unequal lengths/divisions, repeats/jumps; shared-use isolation and alignment hold; recording does not pause unrelated tracks; New/Open replace one document and Save As never appends a Song.

Artifact: complete arrange, record, save and reopen demo through the actual application.

### M6 Finish sound libraries, portable resources and recovery

Dependencies: M1 persistence, M3 takes and M5 composition. Completes former Phase 5.

1. Add Synth-owned sound-definition capture/install types excluding installed-use subscription/routing/mix. Reuse canonical Synth types rather than copying private SavedPatch JSON.
2. Store immutable sound/Custom chord revisions with stable identity and digest over deterministic canonical serialization. Publishing creates a revision; adoption is explicit and preserves local mix/subscription.
3. Use a versioned Song package with composition, pinned definitions, Synth-native local state and content-addressed resource manifest. Normal Save includes pinned definitions and exact external sample/SoundFont references with hashes. Collect Assets creates a self-contained package for referenced files that can be copied.
4. Use a maintained archive implementation if needed. Reject path traversal, duplicate entries, bad references, unsupported versions and excessive decompression/declared sizes before extraction/install. Resource budgets are explicit.
5. Prepare complete candidates before replacement. Correlate graph activation with Song publication and retain/restore previous state on failure. Capture both owners at one host revision boundary; an in-flight structural sound change must finish or be visibly excluded before capture.
6. Complete exact save-baseline handling, migrations and crash recovery. Keep valid prior files, immutable checkpoints and bounded take logs. Recovery shows original/recovered versions and checkpoint time; never silently overwrites user files. Retain referenced revisions; no automatic library garbage collection.
7. Provide missing/changed asset repair: preserve identity, show unresolved items, locate/relink explicitly, verify hashes and require deliberate replacement when content differs. Missing audio never substitutes another sound or destroys composition.

Gate: exact save/open of every form and expression value; publication does not change pinned Songs; failed adoption retains prior pin/graph; edits after save capture remain dirty; interrupted/corrupt writes preserve prior document; collected package opens from a clean resource location.

Artifact: reliable Song packages, revisioned libraries, recovery UI and portability fixtures. Final version-1 format freezes before external release; development schema changes use explicit fixture migrations.

### M7 Complete Song integration and verify full Synth parity

Dependencies: M0's complete Synth foundation and M6 resource ownership. Former Phase 6, now focused on Song-specific integration rather than recreating Synth screens.

1. Connect the already hosted Synth workspace to complete Song resource/reference and save lifecycles. Verify every feature in M0's inventory, including importers, native session compatibility and diagnostic paths, through its original implementation.
2. Preserve asynchronous lifecycle/failure states, exact return focus and preview behavior. Host owns global navigation; no Tracker context enters Synth's standalone PATCH/MIXER model.
3. Add Song tempo/key/meter alongside retained Synth settings, audio/MIDI devices, volume, new-work defaults and sample/SoundFont starting folders.
4. Resolve last valid folder, configured root, then platform default. Keep machine paths out of portable content; show inaccessible roots/missing bundle assets.
5. Expose supported installed registry entries. Figma example instruments/effects and current fixed Synth capacities are not new product limits.

Gate: all existing Synth functionality remains available in the integrated app; choose/import sounds, edit parameters/effects, route, mix, save and reopen through the real host. Retained standalone and integrated rendering/input/audio witnesses pass. One application File lifecycle owns the combined document; native Synth sound-session operations remain explicitly available.

Artifact: complete sound-editing workflow with native input/paint evidence.

### M8 Complete Minimap and broad daily usability

Dependencies: M5 arrangement and M7 workspace. Completes former Phase 4 projections.

1. Expanded and its View Group UI are dropped by the current scope override. Preserve legacy saved data; use Song/Chain/Phrase and Minimap for navigation.
2. Build bounded Minimap activity/density, independent playheads, occurrence navigation, zoom/scroll/follow and narrow-screen windows. Mark conditional/unbounded flow at the preview horizon.
3. Complete controller-only file picking, naming, dialogs, selection, recovery, Help and focus return. Keyboard equivalents supplement controllers; pointer use cannot be required.
4. Review native screens against current relevant Figma frames. Fix clipping, unreadable fields, wrong hierarchy, hidden focus, missing statuses and broken shortcuts. Test designed desktop size and a smaller window without losing operations.
5. Bundle an original small demo Song exercising arrangement, harmony, generators, expression and sound/mix recall with redistributable assets. Keep New Song empty.

Gate: section-1 journey through save/reopen works on the Mac except M9 sync/control-surface work. View switching preserves data/output and source identity. No required action is a placeholder or mouse-only dead end.

Artifact: first broadly usable build—the “mostly working” checkpoint. Continue M9/M10; do not conceal pending physical evidence.

### M9 Add external synchronization and configurable input profiles

Dependencies: M3 capture and M5 transport. Former Phase 7.

1. Extend semantic controller mapping for Tracker and add MIDI control-surface mappings with visible conflicts/reset defaults.
2. Separate clock/system messages from performance capture. Implement 24-PPQN clock in/out, Start/Stop/Continue and Song Position Pointer; use timestamped pulses, not UI ticks.
3. Implement one selected master, tempo/phase estimation, loss/reconnect policy and loop prevention. Preserve musical positions while wall-clock mapping follows the selected clock.
4. Handle focus loss, controller hotplug, rebinding, modifier release and source changes during recording through explicit cleanup/recovery.

Gate: jittered/changing/missing pulse fixtures and reconnect produce declared behavior; virtual-loopback and physical clock tests cover supported endpoints. No domain device-brand/raw-button branches. Claim external-sync recording only after timestamp mapping passes.

Artifact: working MIDI/clock Settings and synchronization fixtures.

### M10 Validate, package and hand over the complete application

Dependencies: all earlier gates. Former Phase 8.

1. Run bounded workload/failure proof below through production paths. Fix demonstrated regressions at their owning layer; no unrelated Synth redesign or open-ended benchmarking.
2. Build relocatable macOS app with Synth's resource conventions. Include licensed fixtures/assets and required notices. Test outside the workspace with fresh preferences.
3. Add CI for pure core, host contracts and build/package checks. Run native/hardware gates on suitable machines; unavailable evidence is not a passing skipped witness.
4. Provide build/run/test/package commands, installation, demo Song, controller guide, recovery instructions and supported MIDI/MPE/clock profiles.
5. Update README, architecture as-built sections and decision log. Keep one current plan and concise evidence, not competing planning systems.

Gate: every confirmed requirement maps to a passing gate or explicitly identified external validation blocker; no critical data-loss, stuck-note, routing or required-workflow defect remains. Clean checkout builds reproducibly; packaged app completes the journey. Name remaining cosmetics/unvalidated platforms precisely.

## 7. Performance and failure proof

Use focused tests at each milestone, then the integrated workload at M10. These are proposed release targets, calibrated in M0 and fixed before declaring a pass. If hardware cannot meet a target, fix its responsible path or explicitly revise the supported configuration; never silently relax a gate.

| Area | Target and evidence |
|---|---|
| Musical determinism | Identical ordered traces for equal revision/seed/input under different window partitions; no cumulative rational-time drift over 10,000 cycles. |
| Internal delivery | At most one sample conversion error in deterministic fixtures, including within-block events, variable blocks, tempo segments and queued Stop/Seek. |
| Supported workload | Start at 48 kHz/256 frames: 16 active tracks, 64 admitted simultaneous voices, mixed divisions, dense controls/retrigger, monitoring and live UI edits. These are fixtures, not product caps. Include large arrangements to prove bounded projection/compilation. |
| Callback headroom | Zero callback allocations/destructions and no deadline misses in a 30-minute admitted workload; target p99 callback time below 70% of block duration. Report misses, late events and queue high-water marks. |
| Scheduler/MIDI | Start with 20 ms planning lookahead, tune from actual device/block behavior. Host delivery lateness target p99 below 2 ms under declared load. Immediate monitoring bypasses playback lookahead. Report external wire/OS constraints separately. |
| Input monitoring | Target physical MIDI-to-audio p95 at or below 15 ms on declared Mac/device configuration. Measure loopback/audio; enqueue timestamps alone cannot prove physical latency. |
| Interaction | Target p95 action-to-painted-feedback below 50 ms during playback; no UI/disk work on scheduler/audio paths. Cursor/meters may coalesce, note/control output may not. |
| Recovery | Inject queue saturation, stale graph activation, device loss, interrupted writes, bad resources, stale take commits and restart. Preserve committed document/latest durable checkpoint; report checkpoint age. |

Initial lookahead/buffer values are tuning starting points, not fixed product limits. Use device-derived timing and configurable hardware budgets. Preserve Synth's separate event/scalar/structural/observation transports. Deterministic proof, native input/paint and physical audio/MIDI remain separate gates.

### Commands and evidence to establish

The existing Synth baseline commands are make check, make test-midi-devices, make test-controller and make test-session-lifecycle. Audio changes also run cargo test --test phase7_callback_safety and the affected renderer/production-runtime tests. Presentation changes run the relevant existing native witness, such as make test-webview-detail-native. Use make fmt-check and make lint for touched Rust code; broaden to the full suite before release, not after every small edit.

The Makefile now provides development, standalone Synth, test, lint, format, workload and bundle commands for one workspace. The discarded shell's cursor, selection and edit latency evidence is historical and does not certify the rebuilt renderer. Its separate feedback scripts were removed. New native acceptance must exercise the current shared shell. Packaging stages a fresh signed app and verifies exact resources and system-only dynamic dependencies; `make verify-bundle` repeats this static gate on a relocated package.

- `make dev` and `make synth` run the composed application and standalone Synth.
- `make test`, `make test-synth` and `make test-host` cover pure behavior, inherited Synth and integrated production witnesses.
- `make fmt-check` and `make lint` check formatting and application-owned Rust code.
- Native witnesses must fail explicitly when a required window/controller path is unavailable.
- Performance evidence must record host, revision, device, sample rate, block size, workload, raw timing and queue metrics.
- `make bundle ARGS=--release` builds the macOS app; native acceptance must also run it outside the checkout.

Keep fixtures for phrase timing/FX, two shared occurrences, ordinary MIDI, two-member MPE, loop/tempo crossings, save revision races, pinned sound adoption and interrupted takes. Tests must observe emitted music, saved content, real activation or painted behavior; do not substitute tests that only mirror private helper logic. Retain concise evidence with exact commands and source revisions, not ever-growing test-count reports.

### Acceptance coverage

| Contract group | Required proof owner |
|---|---|
| Architecture scenarios 1–8: phrase, time, cursors, jumps, FX, sends, ownership, live edits | M0–M2, repeated with full Song in M5 |
| Scenarios 9/19: deterministic generators and occurrence context | M4/M5 |
| Scenario 10: atomic Make Unique | M1 primitive, M3 recording, M5 UI |
| Scenario 11: missing resources | M6 |
| Scenario 12: independent libraries and real bridge | M0 and every Synth integration change |
| Full Synth functionality, actual renderer reuse and workspace builds | M0 inventory/base, M7 complete parity, M10 release proof |
| Scenarios 13–18: scale, display, chromatic intent, banks, voicing, recall | M2/M4 |
| Scenarios 20–22: one Song, save consistency, pinned adoption | M1/M2 initial safety, M6 final proof |
| Scenario 23 and all eight Piano Roll scenarios | M3, full accompaniment in M5, release evidence in M10 |
| Minimap source identity and view invariance | M8 |
| Required navigation/controller-only operations | Native witnesses in M2/M3/M5/M7/M8 |
| External clock/input failure policy | M9 |
| Relocatable package/clean-machine resources | M10 |

## 8. Execution discipline after approval

Proceed milestone by milestone through M10. Keep accepted slices runnable and preserve existing user changes. Use coherent changes with focused proof; do not accumulate an unreviewable rewrite before audible output. Cross-boundary changes require both packages’ compatibility checks and can be committed atomically in this workspace.

Report completed behavior, proof, current blocker and next milestone. Update docs for durable decisions or verified boundaries. Do not re-ask routine choices covered here. An implementation problem calls for investigation and a fix, not a request for the user to design the missing method.

Pause only for material changes outside approved scope: destructive handling of user data, persistent/public contracts unable to honor this plan, paid/licensed dependencies, unavailable credentials/hardware needed for validation, or changes to confirmed musical behavior. Continue independent work while external validation is pending. Never mark unavailable physical tests passed.

The user has authorized autonomous implementation of this plan. That authorization covers proposed defaults, narrow Synth integration changes and work through M10. Publishing releases, spending money, signing/notarization credentials and deleting user resources remain separate actions when needed.

## 9. Risks and explicit stop condition

Largest technical risks: sample-accurate Synth scheduling, MPE voice/control identity, combined document/graph replacement, portable revisioned sounds and native presentation reuse. M0 moves timing/hosting risks first; M3 proves expression before broad UI investment; M1/M2 bring saving forward; M6 closes resource ownership/portability.

Provisional Drum, generator and Minimap layouts can become useful using defaults here and existing design vocabulary. Pixel polish can follow demonstrated usability. Wrong timing, lost expression, stuck notes, broken save/reopen and missing controller operations cannot be called polish.

Stop when M10 passes and the packaged app completes the acceptance journey. Deliver app location, source revisions, commands, proof, supported hardware/profiles and material limitations. If external validation remains impossible, deliver the working build with that exact gate open; do not claim full completion or silently reduce scope.
