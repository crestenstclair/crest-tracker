# Crest Tracker — implementation phases

Updated 15 October 2026. Each phase should leave an executable slice. Framework/language choices follow the existing repository after inspection. Phase numbers express dependency order, not an estimate.

## Phase 0 — establish the sound boundary and timing harness

Inspect Crest Synth, its public API, existing tests, lifecycle, state persistence and presentation modules. Record actual symbols and capabilities for timestamped MIDI input, note-on/off, instrument addressing, parameter control, patch/sample/SoundFont state, mixer/sends and public state observation. Prove these with unchanged Synth behavior. Document any unsupported capability before depending on it.

Establish the separate Tracker library and host composition root, dependency checks, rational musical-time values, injected clock and deterministic MIDI capture sink. Choose platform/controller/renderer adapters based on the repository. Set a realistic latency and scheduling budget. Audit timestamped external input, MPE master/member messages and pitch ranges, monitoring, sustain and destination channel allocation; use recorded packet fixtures and a fake clock before hardware proof.

**Gate:** Tracker domain builds without Synth/renderer; Synth builds without Tracker. At least one note, release and supported control works through the public MIDI boundary. Specific-instrument routing has a proven MIDI-addressing strategy or an explicit unresolved capability.

## Phase 1 — playable phrase, MIDI FX and independent cursors

Implement the smallest end-to-end path: gamepad intent → command → Phrase revision → compiled snapshot → occurrence cursor → shared FX evaluation → MIDI output → rendered state.

- Strong IDs; common NoteData, NoteEvent, recorded ControlEvent and FxEvent; rational onset/duration; explicit instrument/channel target; revisions and validation.
- Phrase grid with Note, target, Velocity and all three FX slots; contextual FX/value/target popovers.
- Working length, division through 1/64, subdivisions, meter and scale/pitch-policy settings.
- Multiple playback instances of the same phrase with isolated cursor, seed, FX memory and note ownership. Audition must use the same playback contract as later Chain/Song.
- Scale-only entry, Notes/Intervals display, global/phrase chromatic override and OFS. No hidden post-effect quantization.
- Typed FX descriptors and slot/stage ordering from day one. Implement meaningful timing/chance/flow/control examples, including FX-only rows, phrase jump and tempo; supported temporary-send mapping where Phase 0 permits it.
- Deterministic ordering, live snapshot activation, Stop/Seek cleanup, stale-output cancellation and no stuck notes.

**Gate:** known event traces match the capture sink; mixed straight/triplet timing aligns exactly; shared uses remain independent; display switching emits the same MIDI; FX-only rows work; stop and seek release owned notes and restore active parameter overrides. Do not defer the FX model to a later architecture rewrite.

### Suggested first coding increments

1. Prove Phase 0's MIDI contract and set up a capture sink before drawing the full app.
2. Add time/pitch/target values and an immutable phrase snapshot with a deterministic evaluator.
3. Add three typed FX slots and cleanup ownership before implementing the first real-time loop.
4. Add semantic controller editing and the phrase projection; connect the public sound adapter.
5. Add simultaneous phrase instances, live edits and working settings; run the acceptance traces.

## Phase 1R — Piano Roll capture and gamepad editing

Recording is a core deliverable immediately after the playable phrase foundation. Build a bounded timestamped performance-input adapter, pure capture state and a recoverable take draft. Commit through existing Phrase revisions and Make Unique/Edit Shared transactions. Preserve original channel/control precision and note expression using the A16 contract and [Piano Roll acceptance scenarios](design/piano-roll.md#acceptance-scenarios-for-implementation).

Deliver source/channel/MPE-zone selection, explicit input-to-event target mapping, monitor, arm/record/end-take, fixed-loop overdub and future-boundary replay without double monitoring. Reuse the occurrence scheduler; phase 4 connects this proven recorder to the full Song accompaniment. Prove two independent playback occurrences alongside monitoring in this phase. Include save/reopen of a minimal phrase/take fixture now; the full Song file workflow remains Phase 5.

Add roll navigation, note insertion/deletion, selection, nudge, pitch/start/length/velocity/release/target edits, copy/paste/duplicate, expression-point edits, grid/triplets, quantize, undo/redo and zoom/scroll. All setup, editing and recovery paths must work with gamepad. Grid changes preserve captured precision; explicit edits preserve note-expression ownership.

**Gate:** all eight Piano Roll scenarios pass against deterministic packet/MIDI fixtures, including simultaneous MPE notes, sustain, loop crossing, tempo changes, routing saturation, shared-reference conflicts, overflow/disconnect and exact take persistence. Test real input-to-output latency and MPE against supported hardware/Synth before claiming faithful playback. A missing Synth capability must be exposed; external MPE output can prove capture/replay independently.

## Phase 2 — phrase forms and harmony

Add distinct Drum, Euclidean and Random authoring/generator models that resolve to the same NoteData. Preview and playback share deterministic resolution. Preserve fixed drum mappings and apply the same scale policy to pitched generation. Mode-view switching is distinct from regeneration or baking.

Implement Default chord derivation by song key/scale, explicit chord shapes, Custom copy/search/save, ADD, named VOI/inversions and octave-doubling SPR. Use literal pitches and range/voice-budget validation. Finalize exact open-voicing fixtures rather than claiming an unverified OXI algorithm. Wire held-Edit chord cycling and bottom Save actions.

**Gate:** Default entries fit every supported scale; explicit chords keep their quality; saved voicings recall exactly and apply once; three-slot limits are respected; generator output agrees across scheduling window sizes; all modes support shared phrase FX and occurrence isolation.

## Phase 3 — chains, references and repeats

Add reusable Chain definitions with stable PhraseUse identities, repeats, duration calculation and chain-local jumps. Show actual use sites in Shared References. Implement Make Unique as a transaction that can copy a shared containing chain before relinking one selected phrase use. Preserve undo semantics and pin resources.

**Gate:** different lengths, divisions, repeats and jumps produce expected monotonic-time MIDI/cursor traces; two uses of one definition are independent; one-occurrence edits leave all other references unchanged.

## Phase 4 — single Song and its projections

Make Song the default application entry. Add up to 16 tracks with independent ChainUse order and cursor stacks, scrolling track windows, empty/playing states and the agreed navigation. Add chain multi-select, Expanded with phrase boundaries and Save View Group.

Add the requested Minimap projection after deciding its bounded preview, density encoding and navigation details. Use the same normalized source graph and runtime projection. Minimap design exploration must not hold up the phrase foundation.

**Gate:** chain, expanded and minimap focus resolves to the correct source occurrence; editing in either editor changes the same content; changing views never rewrites notes, restarts playback or creates pattern data. Piano Roll can record one looped occurrence while other Song tracks continue, with one shared MPE allocator. Musical-time alignment survives unequal lengths, meter changes and jumps.

## Phase 5 — Song persistence and shared resources

Add New Song, Open Song, Save, Save As and rename for one Song document. New/Open replace the current Song after Save/Discard/Cancel when dirty. Save As writes that Song to another file; it does not create a second arrangement inside the document. Preserve song-local Chain/Phrase definitions, pinned shared sound/Custom chord references and local routing/mix. Persist exact recorded values, note-expression associations, source/zone metadata and explicit targets; resolve active drafts before Save/New/Open. Add recoverable persistence, schema migration and missing-resource handling; asset embedding/export remains an explicit policy choice.

**Gate:** each file reopens exactly one Song with its composition, pinned sound/resource identities and local mix. New/Open never append a Song. Cancel leaves the current document intact; failed Open preserves it; later edits remain dirty after an earlier capture saves. Publishing a sound revision does not change the Song until explicit update.

## Phase 6 — full Synth presentation and asset browsing

Reuse or reimplement Synth UI presentation through approved ports: Instruments, patch details, samples/SoundFonts, Mixer, Sends and Settings. Phase 0/1 already delivered the minimal sound adapter; this phase completes screen coverage. Add sample and SoundFont starting-directory preferences and platform location tokens.

**Gate:** existing Synth behavior remains intact; every live sound-control action respects the established MIDI contract; no Tracker concept enters Synth. Browsers use valid remembered/configured roots and handle inaccessible locations visibly.

## Phase 7 — broader MIDI input and synchronization

Add configurable input mappings and MIDI controllers through semantic input adapters. Piano Roll performance input already ships in Phase 1R; add broader controller profiles here. Keep clock synchronization separate from performance capture. Implement external clock, start/stop/continue, loss/reconnect and the agreed clock-master policy.

**Gate:** domain code contains no gamepad/MIDI device brand branches. Clock loss, hotplug and focus loss do not leave held modifiers, notes or temporary overrides active indefinitely.

## Phase 8 — performance, recovery and release

Measure jitter and scheduling cost under dense FX, 16 tracks and large Song arrangements. Validate bounded lookahead/queues, overflow handling, disconnect/reconnect, panic, live edits, undo/redo, interrupted save, migrations and missing assets. Do not broaden testing without a concrete correctness or release risk.

**Gate:** documented timing workload/budget passes, deterministic core and real Synth contract checks pass, output never silently drops required note-offs, and recovery produces useful diagnostics.

## Continuous rules

Dependencies point inward. Views dispatch commands and render projections. Domain transitions perform no device or file I/O. Distinguish musical FxSpec from application I/O requests. Keep the document, playback runtime, editor state and preferences separate. Version proposals as they become decisions; maintain Figma and these docs together when behavior changes.
