# Crest Tracker — Domain architecture and implementation guide

Version 7 · 16 October 2026 · Synth fork and shared application shell

Figma remains the product/design source of truth, subject to the latest explicit user decisions. This guide describes the single-Song document and related coding contracts. Product constraints are confirmed; unresolved runtime policies remain proposals. Tracker now has a pure Rust composition library and a native host built on the in-repository Synth fork. See README.md for the implemented paths, verification boundaries and remaining release gates; the contracts below remain the full target.

[Architecture overview](https://www.figma.com/design/mvRQEFZc5Oi2ZbRkoHhXjH/Crest-Tracker?node-id=2072-3) · [Visual event map](https://www.figma.com/design/mvRQEFZc5Oi2ZbRkoHhXjH/Crest-Tracker?node-id=2077-33) · [Phrase FX pipeline](https://www.figma.com/design/mvRQEFZc5Oi2ZbRkoHhXjH/Crest-Tracker?node-id=2079-43)


[Scale-aware entry](https://www.figma.com/design/mvRQEFZc5Oi2ZbRkoHhXjH/Crest-Tracker?node-id=2096-6020) · [Default and Custom chords](https://www.figma.com/design/mvRQEFZc5Oi2ZbRkoHhXjH/Crest-Tracker?node-id=2096-6036) · [Voicing FX contracts](https://www.figma.com/design/mvRQEFZc5Oi2ZbRkoHhXjH/Crest-Tracker?node-id=2096-6052)

## A00 — A MIDI sequencer with a hard boundary

Architecture proposal v6 · 15 October 2026. Product constraints are confirmed; runtime policies below are implementation proposals, with explicit capability gates.

### Tracker and Synth library boundary

Crest Tracker owns musical composition and sequencing. Crest Synth owns instruments, patches, voices, audio DSP, mixer and send processing. Only the application package imports the Synth library; the pure sequencer does not. Synth never imports Tracker.

Confirmed implementation direction: the complete working Synth application is forked under `crates/crest-synth`. The pure sequencer and Synth remain separate packages; the application composes both. Synth owns physical input, its reducer, renderer and DSP. Tracker screens use typed workspace projections through that same shell. Both packages can be edited, tested and committed in this repository; standalone Synth remains independently runnable. See implementation-plan.md section 3 for ownership and preservation gates.

### Inside Tracker

Composition, Harmony and Playback are cohesive domain responsibilities. Song is the sole document: its arrangement, reusable chains/phrases, resource references and local sound/mix state are saved together. Application use cases coordinate document lifecycle, input, views and MIDI delivery.

Tracker contributes immutable typed workspace projections to Synth's renderer. Rows, grid cells, text entry, focus and presentation labels belong to the host. The renderer emits only focus, activation and text submission; Rust validates visible identities and file-operation tokens before queueing application commands. Physical keyboard and controller input share Synth's semantic translation and saved mappings. No separate Tracker page, controller mapper or browser document exists. Musical values remain exact Rust types; presentation values cross this seam as strings rather than editable JavaScript numbers.

The recorder also owns the pinned occurrence/phrase identity exposed in capture projections. Navigation never substitutes the current editor selection for that identity. A capture draft renders only against its source phrase and remains distinct from stored editable events. Retained-take admission is a pure-core Editor transaction guard: other existing phrases remain editable, while the armed phrase, arrangement, routing, Song settings and phrase-definition membership stay fixed. Apply, Undo, Redo and replacement validate complete candidates before changing content or history. Recorder rebasing adopts each admitted baseline/revision and translates its private contiguous event-ID range above the new Song identities; timing, raw expression, decoder voice IDs and packet order remain unchanged. This also handles Undo moving the identity range downward. Recovery stores the adopted baseline and shifted capture together. Finalization releases the guard only after a successful candidate; asset-preparation failure and explicit discard clear it immediately. Shared-edit presentation suppresses Make Unique during capture and retains the user's remembered choice for afterward.

The previous browser snapshot/BigInt protocol and native-feedback harness were removed with the parallel shell. Their measurements do not certify the rebuilt workspace. Retained viewport and timeline backend code remains available for subsequent UI slices; its presence does not establish a completed native interaction.

### Stateless logic, explicit state

Pure transitions take state + command/input and return next state + facts + effect requests. State remains explicit: document, playback runtime, editor session and preferences. An event loop alone does not make a sequencer stateless.

### Confirmed navigation

Tracker · Instruments · Chord Bank · Mixer · Sends · Settings. Shift + Down in Song opens Mixer; elsewhere it opens Quick Jump. Sample and SoundFont starting directories live in Settings.

### Source status

The complete Synth application is forked under `crates/crest-synth`, including its renderer, input system, devices, native DSP, assets, tests and standalone executable. `FORK.json` identifies the imported source snapshot. Its hosted facade supplies callback-acknowledged graph replacement, native session capture, source/attack-owned MIDI, absolute-frame scheduling and temporary send leases. Synth's reducer remains its only product-state mutation path. The pure Tracker library has no Synth or platform dependency.

The rebuilt workspace supports Song, Chain and Phrase navigation, note entry/adjustment, basic Piano Roll and Drum grids, phrase settings, playback, undo/redo, basic chord operations and the validated Song lifecycle. Instruments, Sample/SoundFont browsers, effects, Mixer, Sends and device/controller settings use the original Synth screens. README.md records the exact remaining UI scope, including Expanded/Minimap, generators, full FX/voicing, expression/selection/quantize tools, MIDI/clock configuration and resource-repair screens. Retained backend capabilities are not evidence of completed UI journeys.

The backend retains exact musical timing, stable occurrence identities, shared-edit guards, recording/MPE capture, owned playback/monitor lifetimes, MIDI output, resource identity validation and recovery. Song format version 1 remains compatible. Candidate loading validates the full document and required Patch identities before replacing the active graph or composition; failed preparation preserves the current document. File operations use correlated tokens and atomic writes. Editor revisions remain monotonic across replacement so stale commands cannot mutate a new document with repeated IDs.

Braids exposes a validated owned-expression profile; initialized-voice delivery applies complete member/master state and attack atomically under the callback budget. Tracker owns MPE interpretation and musical lifetimes; Synth owns native pitch admission and rendering. External MPE delivery uses host-side endpoint workers and transactional allocation. Scheduled accompaniment and live monitoring retain separate ownership. A producer whose horizon falls behind rendered audio stops and cleans up its output before submitting overdue events. Recovery closes interrupted capture at the current audio cutoff and preserves the retained take.

The bundled example uses Synth's canonical session and sound-definition APIs and loads through the same unsaved decisions and acknowledged replacement as Open. Integrated tests exercise the production host, workers and audio renderer. Native smoke checks exercise the rebuilt workspace and original Synth screens, including save/reopen. Passing semantic tests does not certify physical controllers, MIDI devices, native latency or the full release journey; those remain distinct acceptance gates.

## A01 — Ownership, aggregates and references

Song is the only saved document. Chain and Phrase definitions retain their own editing invariants; saving the document captures their references and sound state together. The document envelope is a serialization boundary, not another domain object above Song.

### Composition / Phrase aggregate

Owns shared NoteData, length, division, optional scale override, mode editing/generation settings and three typed FX slots per step. All phrase forms use the same note and FX event types. It validates timing, note bounds and effects without knowing controllers or Synth patch internals.

### Composition / Chain and Song aggregates

Chain owns ordered PhraseUse entities and repeats. Song owns up to 16 Tracks, ordered ChainUse entities and its time signature / meter map. Each occurrence has a stable ID; row position is display order. Meter determines bar boundaries and beat grouping; track ordering owns no instrument routing.

### Song document / persistence boundary

The Song document contains one arrangement, song-local Chain/Phrase definitions, pinned shared sound/chord references and Synth-owned sound state with song-local routing/mix. Application operations coordinate Make Unique and document capture; there is no parent container, song collection or song membership lifecycle.

### Harmony / Scales and chord banks

Harmony owns key/scale definitions, allowed-pitch lookup and explicit chord shapes. Song selects key/scale. Scale-only input/generation is the default; notes or intervals are display projections. Each song has a derived read-only Default chord bank. Custom chords remain shared through the library. Chord quality is explicit: never quantize chord tones or silently change Major into Minor.

### Synth / Sound model

Owns Instrument, Patch, Sample, SoundFont, voice allocation, effect processors, mixer levels and sends. Tracker stores stable instrument references and read-only presentation descriptors, not a second authoritative patch model.

### Shared definitions / isolated occurrences

Definitions contain no cursor or active FX state. The same phrase can play in two tracks at different positions. Runtime state is keyed by playback instance and occurrence path. Make Unique copies the phrase and, when needed, its shared containing chain before relinking only the selected occurrence.

All Chain/Phrase definitions and incoming references are scoped to the current Song. Make Unique validates the selected source/path and current complete reference topology in one serialized commit; a concurrent additional use can invalidate an earlier isolation preview. Sound definitions instead use library identity plus pinned content revision. Updating that reference is explicit and preserves song-local routing/mix; graph-dependent installation commits only after matching activation acknowledgement.

## A02 — State and storage contracts

Views read projections. Commands are the only path to edits. Save musical state independently of transient playback and UI.

### DocumentState / persistent

Store one Song record, song-local maps of Chain/Phrase definitions, pinned shared sound/chord references and Synth-native sound/mix state. Stable ChainUse/PhraseUse IDs connect definitions within the Song. Store schemaVersion and revisions; validate references before committing. No songs collection, selectedSongId or song-order field is needed.

### RuntimeState / transient

Transport epoch, monotonic musical time, active document revision, per-track cursor stacks, generated-event queue, per-occurrence FX memory, seeded randomness, active note ownership and parameter overrides. None belongs in Phrase or a UI widget.

### EditorState / transient

Screen, focus cell, selected occurrences, scroll position, popover draft, held semantic buttons and reference-edit choice. Remember Shared Edit defaults to the current session; a persistent option must be explicit and resettable.

### Preferences / machine-local

Default sample starting directory, SoundFont starting directory, default BPM, controller mappings and endpoint bindings. A browser uses the last valid folder for that asset type, then the configured start folder, then the platform picker’s default. Portable Song files do not bake in machine paths.

### Music types

One musical timeline uses exact rational quarter-note beats. Base divisions through 1/64 combine with explicit tuplet ratios; meter stores numerator, denominator and grouping. The grid is not an internal 64-tick limit. Pitch remains MIDI 0–127; Notes/Intervals are projections using an effective ScaleContext. A displayed interval includes accidental and octave. MIDI channels remain 1–16 per endpoint.

### Percent and tempo types

Velocity/amount use typed decimal percentages in the UI with an explicit Off/zero where meaningful. Captured velocities and controllers retain exact MIDI values; opening/saving never round-trips them through integer percentages. Note-on velocity maps 1–100% to 1–127; zero is a separate cut/off action. CC maps 0–100% to 0–127. Tempo is positive BPM, not a percentage. Current tempo, song initial tempo and preference default BPM are distinct.

### Save package / proposed

Save one versioned Song document with authored definitions, pinned shared sound/chord revisions and Synth-native sound state, including song-local routing/mix. Sound library edits create new revisions; the Song keeps its saved revision until an explicit update. Preserve exact referenced content or report missing resources without substituting latest. Asset embedding/export policy remains to be chosen.

New/Open replaces the single active document after Save/Discard/Cancel when dirty. Validate and prepare the replacement before publishing it; failed or cancelled operations preserve the prior Song. A failed or cancelled prerequisite Save prevents replacement. Save captures immutable owner-valid content and establishes that exact successful capture as the clean baseline; later edits remain dirty. Before continuing New/Open after Save, recheck the current revision and resolve any later edits through the same guard. Save As writes the same single-Song document under another file identity. No parent-container schema or child-song migration is required by this unimplemented plan.

### Meter and grid semantics

Bar duration = numerator × 4 / denominator quarter-note beats. Meter changes alter bar boundaries and grouping without silently moving notes. A division edit explicitly retimes the phrase grid and its events; a view-only change preserves event times. Triplet and other ratios remain exact underneath. Compile to sufficiently fine scheduler timing; never equate a screen row with a transport tick.

## A03 — Five phrase forms, one execution contract

A common phrase envelope and timed output contract; each sequencer form owns its useful authoring representation.

### Common note data

NoteData is one ordered collection of timed NoteEvent, ControlEvent and FxEvent values. Every phrase form reads, edits or generates that representation. NoteEvent carries onset, duration, pitch, exact attack/release velocity, explicit target and optional source/expression metadata. ControlEvent preserves timestamped channel/master controls; note-relative expression follows stable note identity. See A16. FxEvent carries time, slot 1–3, typed effect and target when required. Mode settings describe editing/generation; they do not define another playback format.

### Tracker and drum editing

Tracker rows and drum lanes are projections of the same NoteData. A tracker note cell edits one timed note; a drum hit edits the same event type, grouped by voice. Empty cells add no note and do not cut existing notes. Cut is an explicit event. Both expose the shared three FX slots.

### Piano Roll capture and editing

Piano Roll records external MIDI/MPE into authored NoteData and edits that same source. Preserve off-grid timing, raw control precision, source identity, note expression and sustain; capture bypasses scale filtering. Input mappings stamp explicit event targets. Its edit grid is presentation/entry state, not the phrase-division retiming command. A16 defines recording, routing and take ownership.

### Euclidean and random generation

Euclidean and Random models retain their own parameters, seed and editing state, but generate the same NoteData. Their previews and playback use the same resolved event timeline. Generated output is a versioned result of its inputs, not a second independently editable source of truth. Manual overrides are explicit and use the same event types.

### One playback representation

Resolve authored or generated content to NoteData, group due events into StepFrames, apply the shared phrase FX pipeline, then schedule MIDI. The same clock advances every mode. Different divisions and meters change musical positions and grouping; they never select a different scheduler or note representation.

### Determinism and views

Seed generation by phrase revision, occurrence, cycle and stable event key so scheduler window size cannot change output. Switching the view preserves the note data. Regenerating or retiming is an explicit edit. Baking a generator freezes its resolved NoteData into a new authored phrase without changing the event format.

### One view model

Phrase, piano-roll, drum and expanded views project the same envelope and source references. Expanded cells retain occurrence path, phrase ID, step and FX slot. Musical-time alignment is exact and independent of visual row height; no second pattern store or playback engine.

## A04 — Playback cursors and control flow

Each track has an independent cursor stack. Shared content never means shared playback position.

### TrackCursor

trackId + songId + chainUseId + phraseUseId + phraseStep + repeat counters + phraseCycle + nextDueBeat + revision. The cursor path uses stable IDs and derives current indices for display. Phrase audition uses the same cursor with synthetic parent occurrences.

### Advance transaction

At the next due musical time: adopt eligible document revisions; collect due steps across tracks; evaluate mode and all FX; arbitrate global controls; schedule bounded output; resolve cursor transitions; publish a compact runtime projection. Note attacks, expression/control events, off events and override expiry can be due between steps. StepFrames must retain exact due beats rather than rounding recorded attacks to a row.

### Hierarchy

Advance within phrase; on phrase end, repeat the PhraseUse or advance inside the chain; on chain end, advance that track’s ChainUse. Song lanes need not arrive together. Proposed empty song slots stop that lane; explicit rests belong in phrases. Loop ranges and end policy are explicit transport data.

### Jump semantics / proposed

A phrase jump changes the next local step; a chain jump changes the next PhraseUse; an arrangement jump changes that track’s next ChainUse. Apply after the current positive-duration step. Global time never rewinds. Invalid targets are rejected; defensive runtime faults stop the affected lane and release its notes.

### Conflicts and loops

Evaluate FX1 → FX2 → FX3; the last command for the same local control scope wins. Stop takes precedence over jumps. Simultaneous global tempo conflicts use lowest track order, then last slot, with a visible diagnostic. Cap transitions per scheduling window; a zero-time loop faults instead of hanging.

### Independent editing / live revisions

Editor focus never advances the sequencer. A committed edit becomes active at a declared future boundary beyond already handed-off MIDI. Invalidate unsent work by revision; retain note-off ownership from the version that emitted the note. Deleting the current occurrence or explicit seek cancels affected work and releases its notes.

### Unequal divisions example

16 steps at 1/16 = 4 beats. 12 steps at 1/8 triplets = 4 beats. Their local cursors differ but phrase boundaries align at beat 4. A phrase jump can change the local path without changing the monotonic shared beat clock.

## A05 — MIDI bridge and Synth integration

Tracker produces musical intent. Its output adapter converts that intent to the public MIDI contract; Synth sees sound control only.

### Target union

EventTarget = InstrumentRef(resourceId, revision) | MidiAddress(endpointId, channel) | MpeZoneAddress(endpointId, masterChannel, memberChannels). Instrument selection is per note/hit in the phrase grid. The integration catalog resolves InstrumentRef to a MIDI-addressable destination and declares MPE capabilities. MPE member allocation is shared across monitoring and playback; a fixed MidiAddress cannot preserve independent MPE expression. A track does not carry a hidden route.

### Confirmed / rhythmic resolution

The clarified 1/64 requirement refers to note division: quarter, eighth, sixteenth, thirty-second and sixty-fourth notes, including triplets and other subdivisions. Time signatures define bars and beat groupings on the same exact timeline. MIDI channel addressing and instrument capacity are separate from this rhythmic resolution.

### Hard capability gate

Verify direct instrument addressing. If instruments share one MIDI channel and all receive its notes, a normal Note On cannot select just one. Use an existing dedicated MIDI endpoint/channel or a Synth-owned generic MIDI addressing profile. Do not invent a Tracker callback into a Synth voice or patch object.

### Synth parameter controls

Sequencer-generated mixer/send/instrument controls translate through a supported Synth-owned MIDI/control profile; unsupported controls remain explicit capability gaps. Existing Synth editor pages retain their native semantic actions and AppState::apply path on the Synth side of the boundary. They do not grant Tracker access to sound internals. No phrase/chain/jump opcode crosses the bridge. Resource loading and opaque Synth-native state use its public lifecycle boundary.

### Page reuse strategy

Reuse the actual Synth renderer, pages, components, focus/input handling, meters and waveform projections through a narrow Synth-owned hosting seam. Extract a shared presentation package only where necessary for the two real hosts. Preserve all current Synth functionality in the integrated base; no parallel Tracker-owned renderer or partial page rewrite. The host adds Tracker navigation and views while Synth retains sound-state and presentation ownership.

### Delivery and observation

Resolve a target when scheduling and retain the resolved address with active notes. Queue timestamped MIDI through a port; observe delivery failures separately. A queued batch is not proof a sound played. Meter/capability snapshots are read-only observations and never let Synth mutate Tracker composition.

### Performance boundary

Use a dedicated scheduler/worker where required. Compile immutable snapshots outside the real-time path; bounded queues connect application and playback owners. UI drawing, file browsing, disk saves and logging cannot delay MIDI timing. Measure timestamp support and jitter in Phase 0.

## A06 — Event emit map

Commands request changes; domain facts describe accepted changes; effect requests cross ports. MIDI is an output protocol, not the domain event bus.

### Input → application

Gamepad adapters emit semantic intents: MoveFocus, AdjustValue, BeginEdit, EndEdit, Select, Back, Start, Shift. The interaction controller resolves current screen/focus and dispatches typed commands. Future MIDI-controller mappings reuse these intents; MIDI clock and performance-note input take separate adapters.

### Application → domain owners

Commands include EditPhraseStep, SetPhraseFx, ConfigurePhrase, MakeUnique, ReorderChain, StartPlayback, Seek, SaveChord and SetBrowserRoot. Owners validate, transition once and emit facts or a rejection. No subscriber mutates another owner’s private state.

### Document → compiler → playback

PhraseChanged, ChainChanged, SongChanged, SongScaleChanged, PitchPolicyChanged and ChordRevisionPublished invalidate dependent execution snapshots. DefaultChordBankDerived refreshes the chord-picker projection; pinned selected shapes remain stable. SnapshotReady is adopted at its declared effective beat. Playback produces CursorAdvanced/TransportChanged projections and timestamped MusicalOutput batches.

### Playback → bridge → MIDI sink

MusicalOutput contains NoteIntent, ParameterIntent and transport output. The target resolver and MIDI encoder emit MidiBatch to Crest Synth or external endpoints. DeliveryFailed returns to application diagnostics and the note-safety controller.

### Facts → projections and persistence

Projectors update PhraseView, SongView, ExpandedView and reference-use lists from committed versions. Persistence saves document checkpoints, not every clock tick. Effect completions carry request IDs and return as typed application inputs.

### Ordering and reliability

Single writer per state partition; ordered sequence IDs, correlation IDs and revisions at queue boundaries. Commands reject stale expected revisions. Apply committed facts once; explicit effect IDs deduplicate retries. Never retry a Note On blindly. Rendering may coalesce cursor frames; output may not drop note-offs or stop cleanup.

### Implementation stance

Event-based does not require event sourcing, a broker, microservices or one global untyped bus. Start with in-process typed queues, pure reducers and snapshot persistence. Domain transitions and effect execution are separate, testable functions.

## A07 — Event contracts and consumers

Initial contract inventory. Names are proposals; ownership, payload and delivery semantics are the implementation contract.

### Composition commands and facts

EditPhraseStep / SetPhraseFx / ConfigurePhrase { phraseId, expectedRevision, edit } → PhraseChanged { newRevision, dirtySteps }. MakeUnique { occurrencePath } → ReferencesRelinked { copiedIds, path }. ReorderChain / ArrangeSong → ChainChanged / SongChanged. Compiler, UI and save coordinator consume committed facts.

### Harmony and workspace

SetSongScale → SongScaleChanged; derive DefaultChordBank and invalidate dependent contexts. CopyChordToCustom → CustomChordCreated. SearchChords → ChordSearchResults. SaveChord → ChordRevisionPublished. Default is read-only per song; Custom is shared. Pinned shape revisions stay stable through scale changes. SetPitchDisplay → EditorDisplayChanged only.

### Transport commands and runtime inputs

StartPlayback { scope, start, seed }; StopPlayback { reason }; Seek { location }; AdvanceWindow { startBeat, endBeat }; ExternalClockPulse { timestamp }. Playback emits TransportChanged, CursorAdvanced, MusicalOutput and PlaybackFault. Time windows are half-open to avoid duplicate boundary events.

### Snapshot exchange

CompileRequested { documentRevision, dirtyIds } → SnapshotReady { snapshotId, revision, dependencies }. ActivateSnapshot { effectiveBeat } → SnapshotActivated. Compiler errors produce CompileRejected; playback keeps the last valid snapshot and UI shows the reason.

### Output contract

MusicalOutput { epoch, batchId, beat, origin, intents } → MidiBatch { endpoint, timestamp, bytes }. DeliveryAccepted means queued locally; DeliveryFailed carries endpoint, batch and error. EndpointLost triggers lane safety and a visible warning. Stop/seek increments epoch and removes unsent stale batches.

### Integration and preferences

SetSoundParameter { target, parameter, value } uses the same parameter arbiter as phrase FX. SoundSnapshotObserved { revision, descriptors, levels } updates read-only projections. SetBrowserRoot { assetType, locationToken } → BrowserRootChanged; directory I/O returns BrowseCompleted / BrowseFailed.

### Envelope and serialization

Domain facts: eventId, ownerId, ownerRevision, correlationId, schemaVersion, payload. Runtime outputs also carry epoch and musical time. Store data contracts, not serialized UI objects or external SDK types. Keep authored FX parameters separate from scheduler effect requests.

## A08 — Phrase MIDI FX are foundational

FX storage, evaluation, target resolution and cleanup ship in the first playable phrase slice. Every phrase mode uses this pipeline.

### Typed effect model

FxSpec is a closed/versioned tagged union, not an arbitrary string plus byte. Descriptor: parameter type/range, target scope, stage, duration, conflict rule and capability requirement. Phrase step stores [Fx1?, Fx2?, Fx3?]. FX-only steps execute even when the note cell is empty.

### Execution stages

Resolve scale-aware note/root and target → OFS chromatic offset → exact CRD shape → VOI voicing → SPR spread → chance/timing/gate/retrigger → capability resolution and MIDI. Descriptor stages fix cross-family order; slot order breaks ties within a stage. No scale quantizer runs after OFS or chord expansion. Three FX slots remain the budget.

### Initial families

Pitch/harmony: chromatic offset (OFS), chord/add interval (CRD/ADD), voicing/inversion (VOI), spread (SPR), velocity and gate/cut. Timing: delay, retrigger and probability. Parameters: CC, bend and temporary send through supported mappings. Flow: phrase jump and tempo; chain/song scopes ship with those layers. No tables.

### Scope and memory

Effects declare hit, step, phrase occurrence, channel or transport scope. Repeats and shared phrases have distinct runtime FX memory. A one-step temporary send expires after that step’s musical duration. It does not edit the saved mixer/send baseline.

### Parameter arbitration / proposed

Key overrides by resolved endpoint + channel + parameter. Latest effective override wins; deterministic track/slot ordering breaks ties. Expiry reveals the next override or the latest manual baseline. Manual edits update baseline beneath active overrides; do not restore a stale captured value. Unknown external baseline requires an explicit reset value or rejection.

### Note ownership and channel limits

Track NoteInstanceId → resolved address, pitch, due off and source occurrence. Proposed MIDI 1.0 duplicate-pitch policy: release the owned same-address pitch before retrigger; stale note-offs cannot kill the newer note. Independent overlapping copies require distinct channels/addresses. Channel bend/send affects all notes there; flag unsupported per-note isolation.

### Cleanup and failure

Stop, seek, lane fault and disconnect cancel unsent work, release owned notes, remove overrides and emit required parameter restoration. Endpoint loss may prevent physical release; mark it unresolved and apply an agreed reconnect/panic policy. Queue overflow fails safely; never silently discard note-offs.

### Acceptance trace

Same revision + seed + input stream yields identical notes, FX and cursor paths. Verify FX-only rows, slot/stage order, triplets, jump/repeat interactions, overlapping sends, baseline changes, duplicate pitches and stop/seek cleanup before moving on to Chain.

## A09 — Implementation phases / foundation to song

Phrase first, including MIDI FX. Each phase leaves an executable slice and a measurable completion gate.

### 0 / Boundary and timing spike

Identify language/framework from the repository. Prove a MIDI note, note-off, parameter control, direct instrument addressing and timestamps against unchanged Synth. Audit patch/sample persistence and page reuse. Establish dependency checks, fake clock and MIDI capture sink. Gate: unresolved sound capabilities are documented, never hidden in Tracker-to-Synth calls.

### 1 / Playable tracker phrase + FX

Build shared NoteData, per-note target/velocity/three FX columns, controller input, length/divisions through 1/64 with subdivisions and meter, independent cursors and working settings. Add scale-only entry, Notes/Intervals display, chromatic Settings/phrase overrides and OFS in the common FX pipeline. Include timing/chance/jump/tempo and supported sends. Gate: deterministic traces, out-of-scale overrides preserved and clean Stop/Seek.

### 1R / Piano Roll recording

After the phrase foundation, implement timestamped external MIDI/MPE capture, input routing, monitoring, loop overdub, recoverable takes, expression-preserving gamepad edits and grid quantize. Use the existing scheduler and exact NoteData. Prove the eight recording scenarios in design/piano-roll.md before broader generators; Phase 4 adds full Song accompaniment. Recording is not deferred to Phase 7.

### 2 / Phrase forms and harmony

Drum, Euclidean and Random models share NoteData and the same pitch policy/FX contract. Add per-song derived Default banks, shared Custom copy/search, explicit chord shapes, ADD, VOI and SPR with note previews. Preserve fixed drum triggers. Gate: default chords fit every supported scale; explicit chords retain their tones; voicing preserves pitch classes; modes and multiple cursors agree with previews.

### 3 / Chain arrangement

Build ordered reusable PhraseUse entries, repeats, length calculation and chain jump resolution. Add shared-use inspection and transactional Make Unique. Gate: mixed phrase lengths/divisions and nested repeats/jumps produce expected cursor/MIDI traces with two simultaneous uses of one definition.

### 4 / Single-song workflow

Make Song the default entry, with up to 16 independent track arrangements and full controller navigation. Add chain multi-select, expanded tracker projection, phrase separators and saved view groups. Add the requested Song Minimap as another projection after its exploratory display policies are settled. Gate: editing either view changes one source model; playback stays aligned by musical time through jumps and live edits.

## A10 — Implementation phases / persistence to release

Song save/open and additional input methods extend the proven phrase, FX and cursor foundation.

### 5 / Song persistence and shared resources

Add New Song, Open Song, Save, Save As and rename for the sole document. New/Open replace the current Song through Save/Discard/Cancel; Save As writes a separate document, never adds a nested song. Preserve exact shared sound/chord revisions and song-local routing/mix. Gate: reopen restores one complete Song; explicit sound updates preserve local mix; interrupted saves remain recoverable.

### 6 / Complete Synth pages and asset browsing

Complete Song-specific integration of the Synth workspace already preserved in Phase 0. Verify every existing Synth feature, renderer and importer against its baseline inventory. Apply starting-directory preferences and platform location tokens. Gate: sequenced controls respect the bridge, native Synth editors retain their own reducer path, and all original Synth functionality remains available in the combined app.

### 7 / Configurable MIDI and synchronization

Add mapping profiles and MIDI control-surface adapters to the existing semantic input port; extend the Phase 1R performance input and add separate clock adapters. Handle external clock, start/stop/continue, loss and device reconnect. Gate: no domain code branches on controller brand or raw gamepad/MIDI button codes.

### 8 / Timing, recovery and release

Measure jitter and queue budgets under dense FX and large Song arrangements; exercise overflow, stuck-note prevention, hotplug, undo/redo, save interruption and schema migrations. Gate: deterministic core tests, Synth contract tests, bounded scheduling and useful diagnostics pass under a documented workload.

### Continuous engineering gates

Import rules forbid Synth → Tracker, domain → UI/OS/filesystem and view → mutable domain data. Keep meaningful tests around timing, ownership, effect lifetime, serialization and bridge contracts. Test long mixed-division runs; performance tests use a declared latency budget established in Phase 0.

### Decision log before implementation

Confirm phrase maximum length, default gate/overlap behavior, the exact subdivision presets and meter-change editing policy, supported M8 FX subset and Synth MIDI profile. Review arbitration/jump policies and external-clock loss timeout. Rhythmic division through 1/64 is confirmed. Drum, Euclidean and Random visual details remain open; all use shared NoteData.

## A11 — Coding contracts and dependency rules

Language-neutral type sketches. Map these to the repository’s language after the Phase 0 audit; do not introduce a framework just to implement the diagram.

### Phrase and FX storage

NoteData = ordered collection of NoteEvent | ControlEvent | FxEvent.
NoteEvent = { id, startBeat, duration, pitch, velocityMidi7, releaseVelocityMidi7, target, source?, attackSequence?, releaseSequence?, initialExpression?, expressionPoints[] }.
ExpressionPoint = { id, offsetBeat, sequence, dimension, rawValue }; dimension = Bend14 | Pressure7 | Timbre7 | PolyPressure7.
ControlEvent = { id, beat, sequence, target, sourceScope, message }; message is a validated MIDI channel control with exact raw values.
FxEvent = { id, beat, slot: 1..3, spec: FxSpec, target? }.
Phrase = { id, revision, timing, scaleOverride?, pitchPolicy: Inherit | ScaleOnly | Chromatic, modeConfig, noteDataSource }.
resolve(noteDataSource, occurrence, effectiveScaleContext, pitchPolicy, seed) → NoteData.
An authored source holds NoteData; a generator source produces the identical representation. Target-dependent FX-only rows require an explicit grid target.

### Scale and chord types

ScaleDefinition = { id, revision, name, orderedPitchClasses, intervalSpellings }.
KeyContext = { tonicPitchClass, scaleRef }.
EffectiveScaleContext = { songId, keyRevision, phraseOverride?, allowedPitchClasses }.
PitchEntryPolicy = Inherit | ScaleOnly | Chromatic; global default = ScaleOnly.
PitchDisplay = Notes | Intervals, owned by EditorState/preferences, never a second note array.
ChordShapeRevision = { chordId, revision, name, rootRelativeSemitones }.
DefaultBankKey = { songId, keyRevision, chordCatalogRevision, derivationVersion }.
DefaultBankEntry = { rootInterval, rootPitchClass, preferredShape, fittingAlternatives }.
ChordRef pins a shape revision; Custom references use editable chord identities.
Voicing = { selectedRoot, shapeRef, presetId, presetVersion, spreadAddedVoices, explicitOffsets }; save both the settings and resolved octave placements.
FxSpec adds ChromaticOffset(signedSemitones), ChordShapeRef, ChordVoicing(presetId, version) and ChordSpread(addedVoiceCount).

Application composes the effective scale context from the song and phrase; Harmony receives values and owns musical rules. Harmony never reaches into Song, a cursor, Synth or a view to read mutable state. Composition owns the phrase policy setting; preferences own the global default. Derived-bank caches are replaceable read models, not editable aggregates. Save a chosen shape revision with the Song so a refreshed bank cannot break an existing phrase.

### Pure transitions and evaluation

handle(ownerState, command) → Accepted(nextState, facts, requests) | Rejected(reason).
resolvePhrase(snapshot, cursor, effectiveScaleContext, pitchPolicy, seed) → shared NoteData.
readDueEvents(noteData, window) → StepFrames.
applyPhraseFx(frames, fxState) → notes + parameters + flow + nextFxState.
advance(runtime, snapshot, window) → nextRuntime + MusicalOutput + RuntimeFacts.
Clock and seed are explicit inputs; every mode advances on the same timeline.

### Separate the two meanings of effect

FxSpec means a musical MIDI/sequencer effect authored in a phrase. IoRequest means a request to an output/file/device adapter. Use different types and names. A musical effect may yield a parameter intent, but it never performs I/O inside the domain.

### Ports owned on the inside

Application defines MidiInputPort, MidiOutputPort, SongRepository, AssetBrowserPort, SoundCatalogPort and ClockPort contracts as simple data. Outer adapters implement them. Only the Synth adapter / host composition root imports crest-synth. The domain imports no device SDK, renderer, filesystem or Synth type.

### Suggested module layout

crest-tracker/domain/{composition,harmony,playback,music}
crest-tracker/application/{commands,projections,ports}
crest-tracker/adapters/{gamepad,midi,storage,assets}
crest-tracker/presentation/{screens,view-models}
crest-tracker-host/{composition-root,synth-adapter,synth-views}
crest-synth remains its independently testable library.

### Deterministic output order

For equal musical timestamps: release old notes, apply parameter changes/restorations, then attack new notes for generated output. Captured source packets preserve their same-time sequence inside each source stream (including post-attack expression); initialize per-note state before its attack and place owned cleanup before any channel reuse. Merge streams deterministically without reordering each stream. Stable track/occurrence/event IDs break remaining ties. Global tempo/flow arbitration occurs before timestamp conversion. Probability gates note emission; it does not randomly suppress global flow or required cleanup.

### Single readable application model

AppSnapshot composes immutable DocumentSnapshot, RuntimeProjection (including RecordingProjection), EditorState, Preferences and SoundProjection. Frontend selectors derive visible rows and popovers. SoundProjection is observed Synth state, not an editable copy of Synth’s aggregate. A view dispatches a command and renders the resulting snapshot.

## A12 — Scale-aware entry, one note model

Confirmed product behavior: all pitched editors and generators share scale-aware input. Chord shapes keep their explicit intervals.

### Scale context and ownership

Harmony owns ScaleDefinition, KeyContext, allowed-pitch lookup, interval spelling and scale-fit queries. Song stores key/scale selected from Settings; a phrase may override the scale. Settings also provides the default note-entry policy. Effective context = phrase override when present, otherwise the song settings. Synth receives resolved pitches only.

### Entry and generation policy

Scale-only is the default. Pitch increments/decrements move to the next/previous allowed pitch; Euclidean note sources and Random pitch choices use the same allowed-pitch set within their pitch range. Empty candidate ranges return a visible validation error. Rhythm generation is independent of pitch choice. Fixed drum trigger mappings remain explicit trigger notes rather than being silently repitched.

### Notes or intervals

Notes and Intervals are two projections of the same stored pitch. Display the tonic-relative interval, accidental and octave; for C natural minor, C4 / Eb4 / G4 correspond to 1:4 / b3:4 / 5:4. The ordinal scale position and the interval label are distinct, especially in pentatonic scales. Switching display changes no MIDI output.

### Chromatic access

Settings: Note entry = Scale only or Chromatic. Phrase view: an explicit Inherit / Scale only / Chromatic override, with scope visible in its contextual popup. OFS adds signed semitones to the selected note/root after scale-aware entry or generation. These paths deliberately allow non-scale notes. Show them clearly; never snap them back after an intentional override.

### Existing data and shared phrases

Authored NoteData continues to store absolute pitches. Changing key/scale does not silently rewrite existing notes; mark existing out-of-scale notes and apply the new policy to future edits. Regenerating content is explicit or follows a declared live-generation policy. A phrase inherits scale per occurrence; editing shared content still uses Make Unique / Edit Shared protection.

### Cache and event contracts

SetSongScale → SongScaleChanged → derived bank refresh and affected projections/generator snapshots invalidated. SetNoteEntryPolicy / SetPhrasePitchPolicy → PitchPolicyChanged / PhraseChanged. SetPitchDisplay → EditorDisplayChanged only. Cache generated results by phrase revision + effective scale revision + entry policy + occurrence + seed, so the same phrase can run safely in different phrase-local scale contexts.

## A13 — Default and Custom chord banks

The bank can guide chord choice. Chord playback always uses the exact selected root-relative interval shape.

### Default / derived and read-only

Each song has a Default bank derived from its key and scale. Entries show root interval, root note, chord name/quality and its chord tones. Default entries cannot be renamed, edited or deleted. Changing the song key/scale generates a new derived bank revision; it never edits a Custom chord.

### Chord quality stays explicit

Selecting Major, Minor, Diminished or another shape plays exactly those intervals above the chosen root. Do not replace a chord quality or quantize its tones to the current scale. Scale-fit labels are guidance. A user can intentionally choose an out-of-scale chord; no automatic correction occurs.

### All-scale derivation / proposed algorithm

For each scale root, test chord-catalog shapes by transposing their pitch classes and checking containment in the scale. Show all fitting candidates; use stacked scale thirds as a familiar initial default where meaningful. If a familiar named triad is unavailable, show a fitting generated interval set with an honest label. Do not impose seven-degree major/minor rules on pentatonic, whole-tone or custom scales, or invent a “correct” quality.

### Example / C natural minor

1 → C minor · 2 → D diminished · b3 → Eb major · 4 → F minor · 5 → G minor · b6 → Ab major · b7 → Bb major. These are fitting triads for this scale, not compulsory choices. Harmonic minor generates a different set because its seventh differs.

### Custom / shared and editable

Custom contains user-created, copied or search-generated chords and remains shared through the library under the existing revision policy. Copy Default or copy Custom creates a new editable identity. Save from Chord Search writes Custom. Phrase chord references pin the selected shape revision so a later key change does not silently change a saved major/minor quality.

### Phrase override and chord search

A phrase-local scale does not mutate the song Default bank. The popup shows both the bank key and the effective phrase scale when different. Chord Search can filter by song scale, effective phrase scale, chosen notes, root, quality or intervals; show candidate notes and fit before Save to Custom. A no-match result keeps the selected interval set and allows an explicit custom name.

## A14 — Chord FX and harmony contracts

OXI One chord mode is the behavioral reference for voicing and spread. Crest retains explicit chord quality, its existing three FX slots and shared NoteData pipeline.

### Typed effects

OFS = ChromaticOffset(signedSemitones). CRD = ChordShapeRef(id, revision). VOI = ChordVoicing(presetId, version), covering inversions and open arrangements. SPR = ChordSpread(addedVoiceCount). VOI replaces the earlier inversion-only command. All resolve to common timed notes and FX before MIDI.

### Order and scope

Resolve note/root → OFS → expand the exact CRD shape → VOI → SPR → timing/gate and MIDI. Column order resolves ties within a stage. Preview the resulting pitches and voice count. Never quantize chord tones after expansion or voicing; the selected major/minor/diminished quality remains intact.

### Voicing / OXI-informed design

Use named presets: Root / closed, 1st inversion, 2nd inversion, Open, Open 1st and Open 2nd. Store versioned per-voice octave transforms. Root and ordinary inversions have standard definitions; exact open-preset pitch mappings require accepted note fixtures before implementation. Do not claim the undocumented OXI transformation table is reproduced.

### Spread / replaces the previous proposal

Add octave doublings of existing chord tones. Remove the alternating-voice octave-shift algorithm. Proposed Crest value: number of extra voices. SPR 0 preserves the voicing; positive counts append the lowest unused higher-octave chord tones above its highest note, in pitch order. This count mapping is a Crest contract, not a verified OXI numeric mapping. Validate voice budget and MIDI range.

### Three slots and saved chords

CRD + VOI + SPR uses three slots. A phrase Chromatic override permits an out-of-scale root when no OFS slot is free. Save Chord stores the shape plus versioned voicing/spread settings and resolved offsets; recall preserves the heard result. Resolved offsets are a verification snapshot, not another base shape to transform again. Defaults stay read-only. Copy/save produces Custom; it never overwrites the source.

### Events and ownership

SetPhraseFx(VOI / SPR / OFS / CRD) → PhraseChanged → voicing preview + execution snapshot. SaveChord → ChordRevisionPublished. Harmony evaluates copied pitch arrays using plain inputs, never Synth or mutable UI state. Voice count belongs to note generation and is independent of the number of MIDI channels.

### Phase gates and acceptance

Phase 1 establishes typed FX and chromatic entry. Phase 2 adds named voicings, octave-doubling spread, Default/Custom search and exact pitch previews. Test quality preservation, added voice count, range/budget failures, custom recall and three-slot limits. Root C minor + SPR 1 previews C4 Eb4 G4 C5 under the proposed Crest count policy.

### OXI reference / verified

OXI ONE MKII manual v2.0a, printed pp.150,153–154: chord type, voicing and spread are separate controls. Voicing includes inversions/open choices; spread adds upper-octave tones. Auto voicing adjusts inversions for smoother movement. User slots remember root, type, voicing and spread. These behaviors guide Crest; OXI hardware limits and scale-driven quality selection do not override Crest’s explicit-chord rules. [Official OXI ONE MKII manual, v2.0a](https://drive.google.com/file/d/1LdJvG-GqzzKI2qw92CyZKklE7kSoZLFE/view).

### Auto voicing / separate proposal

Keep Manual as Crest’s default. A future Auto option may choose octave placements to reduce movement while preserving the selected chord tones. It needs previous-voicing state per playback occurrence, deterministic ties and declared Stop/Seek/Jump resets. It must not mutate shared phrases or change chord quality. Bass doubling, strum and arpeggiation remain separate concerns, not hidden spread behavior.

## A15 — Song Minimap projection

Confirmed addition: the top-level Song screen offers a Minimap mode alongside the normal chain arrangement. The user supplied a Stria Android tracker screenshot as inspiration and explicitly left the display design open. [Figma T52](https://www.figma.com/design/mvRQEFZc5Oi2ZbRkoHhXjH/Crest-Tracker?node-id=2110-5926) and [T53 view picker](https://www.figma.com/design/mvRQEFZc5Oi2ZbRkoHhXjH/Crest-Tracker?node-id=2110-5958) are exploratory screens.

### Shared source, different projection

Minimap reads the existing Song → Track → ChainUse → PhraseUse → resolved NoteData graph. It adds no Pattern aggregate, note store, cursor or playback engine. A view change preserves transport, selection and stable occurrence identity. Expanded remains the multi-select-chains convenience view, with phrase separators and edits directed back to source phrases.

### Provisional display and interaction

The current sketch uses 16 narrow lanes, musical time vertically, a 32-bar example viewport, activity stripes, phrase boundaries, a focused occurrence and playback position. Bar labels come from the meter map, not a fixed row count. All data in the sketch is illustrative. Exact note-density/pitch encoding, zoom, scrolling/follow behavior, controller shortcut and narrow-screen behavior are open decisions.

### Projection contract / proposal

Use a bounded musical-time viewport plus document/runtime revisions. Project occurrence segments carrying source paths, time spans and activity summaries. Focus/hit testing returns a stable occurrence path, not only a screen row. Independent track cursor stacks project into the same musical-time axis. Repeats, conditional jumps and unbounded loops need an explicit bounded-preview policy; do not claim a complete future performance timeline is always knowable. Random/generative previews must declare whether they represent the current deterministic realization or a density estimate.

### Events and implementation gate

SetSongViewMode updates EditorState and invalidates only the projection. FocusMinimapOccurrence updates focus; OpenFocusedChain uses the existing navigation intent. Save View Group retains occurrence references. These names are proposed, not a frozen public API. Phase 4 can add Minimap once the single-song playback and expanded projection are sound; unresolved rendering decisions must not block the phrase foundation.

Acceptance: switching Chains ↔ Minimap ↔ Expanded changes neither authored NoteData nor MIDI output. After a jump or Make Unique, focus and activity refer to the correct occurrence. Preview generation is bounded and cannot starve playback.

## A16 — Piano Roll capture, expression and routing

Piano Roll is the fifth Phrase form. Recording external MIDI/MPE while accompaniment plays is its first responsibility. The detailed product flow, proposed defaults, controller actions and eight acceptance scenarios are in [Piano Roll](design/piano-roll.md). They extend the common phrase model; there is no parallel piano-roll scheduler or instrument store.

### Owners and transitions

The MIDI input adapter validates/normalizes timestamped source packets, separate from semantic button input. Application recording state owns one armed occurrence, transport anchor, source mapping, draft take and held-input identities. Pure capture transitions assemble NoteData; Composition alone commits it through expected revisions and the existing shared-reference transaction. One completed take is one undoable append. Conflict/overflow/disconnect retains a recoverable draft and a visible reason. No silent replacement of prior notes.

Runtime capture state is not saved as live device handles or active subscriptions. Song saves exact committed notes, scoped controls, source/zone/bend-range metadata and explicit targets. Save finalizes an active take before immutable capture; New/Open guards include the draft. A failed commit/save preserves it. Draft recovery checkpoints are required before claiming crash-safe recording.

### Exact timing and expression

Convert monotonic input time through the transport tempo map into rational beats; do not snap to the grid. Each source has a stable sequence to preserve equal-time packet order. Record note-on/off velocity, 14-bit bend, pressure, CC74, sustain and other supported channel controls without reducing precision. Source endpoint/session/channel/attack identifies a note; pitch alone cannot. Channel-wide data stays channel-wide. MPE member expression is note-owned, initialized at attack and combined with separate master controls. Preserve physical key release independently of pedal-held lifetime. Unknown zone/bend-range configuration is an explicit capability/setup error.

Note-relative expression is part of the NoteEvent value, not three extra FX slots. Channel/master controls are ControlEvents in the common ordered NoteData. MidiInputPort delivers timestamped packets; note ownership and destination capability checks stay in the existing MIDI boundary. StepFrames retain off-grid note/control due times. Views project these values without rounding, flattening chords or discarding unsupported columns.

### Loop, monitoring and output ownership

The first slice proposes raw fixed-loop overdub on one armed occurrence. Other occurrences keep advancing. Draft notes are monitored once and never replayed during their capture take. Commit activates at a future loop boundary beyond already delivered output. A held note crossing the loop stores modulo onset plus unwrapped duration and relative expression; it is not cut/re-attacked at each visual edge. Freeze source/route/timing for an active take. Leaving/deleting the capture path, seek/stop or input failure ends it visibly and preserves material.

Input routing stamps InstrumentRef, MidiAddress or MpeZoneAddress onto each event. Instrument destinations need a proven MPE profile; a fixed MIDI channel must not silently flatten expression. One allocator owns each destination's MPE member leases across monitor, accompaniment and independent occurrences. Source channel provenance remains stored even when output channels are remapped. Validate zone/master compatibility and capacity before output. Independent master expression/sustain timelines require distinct zones/endpoints or a proven isolating profile; member allocation alone is insufficient. Exhaustion rejects a new attack visibly; sustain and owned release complete before reuse. Cleanup is scoped to owned notes/controllers, never a global reset that cuts accompaniment.

### Editing and FX

Grid choice is editor state. Nudge/quantize move selected note starts and attached expression, preserving duration unless explicitly edited. Preserve raw capture on scale changes; scale policy governs newly authored entry only. Copy/duplicate creates new note/point IDs; delete removes owned points; channel/master lanes need explicit selection. Commands are revisioned, validated and undoable. Retargeting shared-channel controls requires an explicit scope, not a guessed conversion.

Probability drops a note with its owned expression; delay moves them together. Chord/retrigger expansion creates new runtime note identities and member allocations. Reject unsupported expression/FX combinations at compilation with the previous valid snapshot retained. Existing three FX slots and temporary-control arbitration remain in force. For a captured controller value and FX override on the same target, captured playback supplies the timestamped baseline; expiry reveals its latest value, never a stale setup value.

### Implementation gate

Phase 0 proves ingress, timestamps and MPE destination capabilities. Phase 1 establishes exact NoteData and safe output; Phase 1R implements recording/editing and minimal take persistence; Phase 4 verifies full Song jamming. The Piano Roll acceptance suite includes raw round-trip, simultaneous per-note expression, loop/tempo crossing, gamepad-only editing, quantize/undo, global member allocation, recovery and shared-reference isolation. Synth support remains unverified until those integration checks pass.

## Event contract matrix

| Producer / owner | Trigger | Emitted fact or output | Consumers | Ordering / failure contract |
|---|---|---|---|---|
| Input adapter | Gamepad buttons and axis edges | Semantic input intent | Interaction controller | Held modifiers are explicit; coalesce axis repeats, not release edges. |
| Composition | EditPhraseStep, SetPhraseFx, ConfigurePhrase | PhraseChanged(id, revision, dirtySteps) | Compiler, projectors, save coordinator | Expected revision required; rejected edits emit no change fact. |
| Composition | MakeUnique(occurrencePath) | ReferencesRelinked(copies, path) | Reference index, compiler, views | Copy and relink atomically; undo is one application transaction. |
| Harmony | SaveChord(customBankId, intervals, name) | ChordRevisionPublished | Catalog, explicitly updated Song references, compiler | Existing revision pins remain valid; Default entries reject edits. |
| Composition + Harmony query | SetSongScale(songId, key, expectedRevision) | SongScaleChanged; DefaultChordBankDerived | Compiler, generator cache, note and chord projectors | Commit key first; derive against its revision; discard stale derivations. Authored notes and pinned chord shapes stay unchanged. |
| Preferences / Composition | SetNoteEntryPolicy / SetPhrasePitchPolicy | PitchPolicyChanged / PhraseChanged | Entry controller, generators, projections, compiler | Resolve Inherit in the occurrence context. Chromatic overrides bypass pitch filtering. |
| Editor | SetPitchDisplay(Notes or Intervals) | EditorDisplayChanged | Phrase and generator projectors | Changes presentation only; no note rewrite or MIDI recompile. |
| Harmony | CopyChordToCustom(sourceShapeRevision) | CustomChordCreated | Custom catalog, save coordinator | New independent identity; copied source remains unchanged. |
| Harmony query | SearchChords(criteria, contextRevision) | ChordSearchResults | Search popup | Read-only result; Save is a separate explicit command. Discard stale query results. |
| Composition | SetPhraseFx(OFS / CRD / VOI / SPR) | PhraseChanged | Compiler, voicing preview, playback | Validate slot budget and parameter range; stable stage order; no post-chord scale quantization. |
| Compiler | Changed definitions | SnapshotReady / CompileRejected | Playback coordinator, views | Immutable dependency/version set; retain previous valid snapshot on failure. |
| Playback | ActivateSnapshot(effectiveBeat) | SnapshotActivated | Runtime projector | No earlier than already delivered output horizon. |
| Playback | AdvanceWindow([start,end)) | MusicalOutput, CursorAdvanced, PlaybackFault | Delivery adapter, view projector | Half-open windows; deterministic source IDs; bounded transition budget. |
| Playback | StopPlayback / Seek / lane fault | TransportChanged plus cleanup output | Delivery adapter, UI | Cancel unsent epoch, emit owned releases and override restoration before new attacks. |
| Parameter arbiter | Phrase override / manual baseline / expiry | EffectiveParameterChanged | MIDI encoder, sound projection | Same effective time uses stable ordering; expiry reveals latest baseline. |
| Output adapter | MusicalOutput | MidiBatch / DeliveryAccepted / DeliveryFailed | Synth or external endpoint; diagnostics | Accepted means locally queued, not audibly played; never blindly retry Note On. |
| Device adapter | Endpoint disconnected | EndpointLost | Playback safety, UI | Cancel future work; physical cleanup may be impossible until reconnect. |
| Song document lifecycle | SaveSong / SaveSongAs / OpenSong / NewSong | SongSaved / SongOpened / SongCreated / failure | UI, compiler, catalog | One Song per document. New/Open replaces it after Save/Discard/Cancel; failure preserves current state. Save establishes only the exact written capture as clean; later edits stay dirty. |
| Sound configuration | AdoptSoundRevision(useId, definitionId, revision, expectedRevision) | SoundRevisionAdopted / failure | Song capture, sound projection | Explicit update preserves local routing/mix. Structural adoption commits only after matching activation acknowledgement; failure retains the previous pin and configuration. |
| Preferences | SetBrowserRoot(assetType, token) | BrowserRootChanged | Asset browser, settings projection | Machine-local path/token; missing or inaccessible path gets a visible fallback. |
| Synth integration | Observed public sound state | SoundSnapshotObserved | Read-only sound UI projector | No Synth object ownership crosses into Tracker; mutations still use approved MIDI controls. |

## Minimum acceptance scenarios

1. **Pure phrase plus FX:** a controller edits Note, Instrument/Channel, Velocity and all three FX slots; playback emits the expected MIDI trace through a capture sink and the Synth adapter.
2. **Time arithmetic:** a 16-step sixteenth-note phrase and a 12-step eighth-triplet phrase meet exactly at beat 4. Repeat for thousands of cycles without cumulative musical-time drift.
3. **Shared phrase, independent cursors:** two tracks reference the same phrase but start on different steps. Cursor, RNG and FX memory do not leak between them.
4. **Jump safety:** a phrase jump changes the next source step after the current step duration; chain and arrangement jumps preserve monotonic transport time. Bad targets or excessive zero-time transitions fault the affected lane.
5. **FX-only step:** a parameter or jump effect executes with an empty note cell. Probability affects note generation, not stop cleanup or global control execution.
6. **Send arbitration:** baseline 20%, override A 35%, override B 60%; A expires, then manual baseline becomes 25%, then B expires. Effective values are 35 → 60 → 60 → 60 → 25. Stop removes all overrides.
7. **MIDI ownership:** changing a note’s target while it sounds does not redirect its Note Off. A stale release from an older duplicate-pitch note cannot kill the newer retrigger.
8. **Live edit:** an edit changes unsent future output at an explicit boundary. Events already handed to an endpoint are not falsely claimed cancellable; old note releases remain valid.
9. **Generator determinism:** one wide render window and several adjacent narrow windows produce the same output and cursor path for the same revision, occurrence and seed.
10. **Atomic Make Unique:** copying a phrase inside a shared chain changes exactly the selected occurrence; all other uses and resource pins stay intact, and undo restores the original links.
11. **Missing resources:** reopen a Song file with a missing sample or shared revision. Show the unresolved item, preserve its identity, and do not silently substitute an instrument.
12. **Architecture boundaries:** build/test Tracker domain without Synth or the renderer. Build/test Synth without Tracker. MIDI integration tests prove sound-control behavior against the actual Synth API.

13. **Scale-aware entry:** in C natural minor, pitch-up from D4 yields Eb4 in Tracker and the same candidate restriction applies to Euclidean/Random pitch sources. Verify key changes, phrase overrides, all supported scales and empty pitch ranges. Fixed drum trigger mappings remain unchanged.
14. **Display invariance:** Notes ↔ Intervals changes labels only, including accidentals and octaves; stored NoteData and emitted MIDI are byte-for-byte equivalent.
15. **Chromatic intent:** global/phrase Chromatic permits E4 in C minor; D4 + OFS 2 emits E4 even under ScaleOnly. C Major emits C–E–G in C minor without correction.
16. **Default and Custom:** every Default candidate belongs to the selected song scale. Default cannot be edited; copying creates an independent Custom identity. Changing the song key refreshes Default while Custom and previously selected shape revisions remain stable.
17. **Voicing and spread:** C minor at C4 with 1st inversion yields Eb4–G4–C5. Root/closed with Crest SPR +1 yields C4–Eb4–G4–C5: one extra octave-doubled voice, no shifted-away original voice. Verify open-preset note fixtures, preserve chord pitch classes, and reject unsupported or out-of-range results. The numeric spread mapping is a Crest proposal; exact OXI equivalence is not claimed.
18. **Three-slot budget and recall:** CRD + VOI + SPR occupies exactly three slots. A chromatic phrase override permits an out-of-scale root without a hidden fourth slot. Save Chord retains the selected shape, versioned voicing/spread settings and resolved voiced offsets. Recalling Custom reproduces the saved output.
19. **Occurrence isolation:** reuse one generator Phrase in two tracks of the Song. Each occurrence keeps its own cursor/RNG state. After a Song-key or phrase-scale change, results resolve the effective context; stale revisions cannot reactivate an old generated snapshot.
20. **One Song document:** New replaces the current document with one empty Song; Open replaces it with one validated Song; Save As writes a separate file containing that same single arrangement. Cancel, failed Open and failed/cancelled prerequisite Save preserve the current Song.
21. **Save consistency:** edit after a save capture begins. Successful persistence marks only that captured revision clean; later edits remain dirty. A pending New/Open must resolve those later edits before replacing the document.
22. **Pinned sound adoption:** publish a new sound-definition revision while a Song uses the older revision. Playback and reopen retain the saved revision until explicit adoption. Adoption preserves Song-local subscription, routing and mix; a failed activation retains the previous pin and sound configuration.

23. **Piano Roll recording:** pass all eight [recording/editing acceptance scenarios](design/piano-roll.md#acceptance-scenarios-for-implementation), including exact raw-value persistence, per-note expression, monitoring, quantize/undo and failure recovery.

## Technical foundations

These sources establish protocol and architectural terminology. Project-specific data structures, event names, runtime policies and implementation phases above are design proposals.

MIDI 1.0 channel voice messages identify channels 1–16; Note On/Off, CC, bend, SysEx and clock are separate protocol messages. This constrains the bridge, not the number of tracks or shared instruments. [MIDI message semantics](https://midi.org/summary-of-midi-1-0-messages).

Dependencies point toward domain rules. Use-case-owned ports let outer devices, storage and views implement required operations without importing their SDK types into the domain. [Clean Architecture](https://blog.cleancoder.com/uncle-bob/2012/08/13/the-clean-architecture.html).

DDD usage: a bounded context defines model meaning and ownership; an aggregate is a consistency boundary within that model. Tracker and Synth are library boundaries, not an exhaustive bounded-context map. Validate domain boundaries through language, ownership and invariants. Do not create a service or event broker for every noun.
