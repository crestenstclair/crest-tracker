# Implementation kickoff brief

Use this document to start a new coding session with the Crest repositories. It does not presume a language or framework.

## Objective

Implement Crest Tracker as a separate sequencing library integrated with unchanged Crest Synth through a host adapter. Begin with the Phase 0 boundary/timing audit, then the Phase 1 playable phrase. Do not begin with a full Song UI that has no real note/FX execution underneath.

## Context to load

Read README.md, product-spec.md, Crest-Tracker-Architecture-v1.md (current content version 4), implementation-plan.md, open-decisions.md and event-map.md in this folder. Open the relevant frames through design/screen-index.md. Live Figma is the product/interaction source of truth; latest user decisions supersede historical mockups. Archive notes are historical.

## First session

1. Inspect repository instructions, language, workspace/package layout, existing Synth tests and build commands. Record current behavior before changes. Do not assume an unseen public API exists.
2. Produce a short capability map with actual file/symbol references: MIDI note-on/off, timestamps, direct instrument addressing, mixer/send parameter controls, state snapshots, assets and reusable presentation modules.
3. Propose the smallest package boundaries that match the repository. Only the host Synth adapter imports Synth. Pure Tracker domain/application tests must run without the renderer or Synth.
4. Prove a MIDI note and owned release through the existing Synth ingress, using a capture sink for deterministic traces. Resolve or explicitly report missing capabilities without introducing Tracker concepts into Synth.
5. Build the first phrase slice: shared NoteData, exact musical time, target per event, velocity, three typed FX slots, explicit occurrence state, injected clock and semantic controller commands.
6. Wire a phrase view as a projection of state; implement working length/division/scale settings and scale-aware note entry. Include FX-only steps, local flow/timing examples and cleanup from the start.
7. Report changed boundaries, working behavior, meaningful verification and remaining capability gaps. Update the decision log when a proposal becomes a concrete implementation decision.

## Implementation constraints

- All phrase forms resolve to the same timed note and FX data. Keep generator settings distinct from resolved output; do not create independent playback engines.
- Keep DocumentState, RuntimeState, EditorState and machine Preferences separate. One readable AppSnapshot can compose them for the frontend.
- Cursors belong to playback occurrences, never shared phrase definitions or view widgets. UI focus and transport position are independent.
- Instrument or MIDI-channel routing is in the phrase grid and resolved per event. Preserve the resolved destination for eventual Note Off.
- FX are typed/versioned and include stage, scope, lifetime and conflict behavior. Distinguish musical effects from I/O requests. Stop/seek/disconnect must handle notes and temporary overrides.
- Rhythmic resolution reaches 1/64 with subdivisions and meters. Use exact musical positions; do not hardcode a 64-tick bar or 64 MIDI channels.
- Scale-aware entry/generation is default; OFS or explicit pitch policy permits non-scale pitches. Notes/Intervals is a display change. Chords remain literal.
- Shared edits must respect Make Unique/Edit Shared, including shared parent chains. Expanded/Minimap never fork the note store.
- Follow the current repository's engineering conventions. No event broker, distributed service split or full event-sourced persistence is required merely because the design is event-driven.

## Initial evidence to produce

A dependency diagram consistent with A00/A11; a verified Synth capability table; a reproducible run/build command; deterministic phrase/FX MIDI traces; two independent uses of one phrase; straight/triplet timing equivalence; display-invariant MIDI; and Stop/Seek cleanup. Add tests for these behavioral risks rather than tests that only mirror getters or UI scaffolding.

Do not claim completion of later phases, full M8 parity, final generator layouts or exact OXI algorithm equivalence. The requested Minimap is present as an exploratory design and can follow the core single-Song workflow.
