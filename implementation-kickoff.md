> Historical kickoff. The current repository layout and recovery workflow are in [README.md](README.md) and [implementation-plan.md](implementation-plan.md). The pinned sibling dependency was superseded by the user-authorized in-repository fork. Product requirements below remain applicable.

# Implementation kickoff brief

Implementation is already underway under the user’s autonomous execution authorization. Start with README.md, current source/tests and both repository diffs; do not recreate the workspace or re-ask permission to implement. The original brief below remains the target, not a claim that work has not started.

Use this document after the user approves execution of the [full implementation plan](implementation-plan.md). Its proposed stack is a pure Rust Tracker library and a Rust/Tauri host using the sibling Synth library. Before approval, the plan and defaults remain proposals.

## Objective

Implement Crest Tracker through M10 of the full plan: a packaged application with composition, expressive recording, arrangement, integrated sound editing, safe Song persistence and synchronization. Preserve standalone Synth behavior while adding the plan's narrow generic integration capabilities. Begin with M0's sound/timing boundary, M1's persistent musical document and M2's playable phrase with FX. M8 is the broadly usable checkpoint, not the final stopping point. Do not begin with a full Song UI that has no real note/FX execution underneath.

## Context to load

Read README.md, product-spec.md, Crest-Tracker-Architecture-v1.md (current content version 6), implementation-plan.md, open-decisions.md and event-map.md in this folder, plus design/piano-roll.md. Open the relevant frames through design/screen-index.md. Live Figma is the product/interaction source of truth; latest user decisions supersede historical mockups. Archive notes are historical.

## First session

1. Inspect repository instructions, language, workspace/package layout, existing Synth tests and build commands. Record current behavior before changes. Do not assume an unseen public API exists.
2. Produce a short capability map with actual file/symbol references: MIDI note-on/off, timestamps, direct instrument addressing, mixer/send parameter controls, state snapshots, assets and reusable presentation modules. Include timestamped input, MPE expression/zone/bend ranges, monitoring and global member allocation.
3. Establish the accepted two-package boundary. Only the host Synth integration imports Synth. Pure Tracker domain/application tests must run without the renderer or Synth. Revalidate the inspected Synth revision before relying on source details.
4. Prove a MIDI note and owned release through the existing Synth ingress, using a capture sink for deterministic traces. Resolve or explicitly report missing capabilities without introducing Tracker concepts into Synth.
5. Build the first phrase slice: shared NoteData, exact musical time, target per event, velocity, three typed FX slots, explicit occurrence state, injected clock and semantic controller commands.
6. Wire a phrase view as a projection of state; implement working length/division/scale settings and scale-aware note entry. Include FX-only steps, local flow/timing examples and cleanup from the start.
7. Follow the phrase foundation with M3: raw external MIDI/MPE capture, explicit routing, loop overdub, expression-preserving gamepad edits and quantize. Do not defer recording to external-sync work. Basic Song save/open and recovery ship during M1/M2, before full resource portability in M6.
8. Continue M4–M10 after each gate passes. Report changed boundaries, working behavior, meaningful verification and remaining capability gaps. Update the decision log when an accepted proposal becomes a concrete implementation decision. Do not ask again for routine defaults covered by the approved plan.

## Implementation constraints

- Use the complete working Synth application as the source dependency and rendering foundation. Preserve all existing functionality, including its UI, importers, audio engine and diagnostics. Establish local paired-repository development in M0; use a pinned revision for clean builds. Move sound behavior into public Synth operations when needed rather than letting Tracker manipulate Synth internals.
- Model exactly one Song document. Store one Song record and its local Chain/Phrase definitions, never a collection of songs or a selected-song index. New/Open replaces the active document through Save/Discard/Cancel; Save As writes another file.
- Keep shared sound-definition revisions pinned until explicit update. Installed sound use, subscription, routing and mix belong to the current Song. Preserve Synth-native ownership and serialization through the host adapter.
- Piano Roll is the fifth phrase type. Preserve raw captured pitch/time/control values; scale policy applies to authored entry, never incoming performances. Exact recording contracts and defaults are in design/piano-roll.md.
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

A dependency diagram consistent with A00/A11; a verified Synth capability table; a reproducible run/build command; deterministic phrase/FX MIDI traces; two independent uses of one phrase; straight/triplet timing equivalence; display-invariant MIDI; and Stop/Seek cleanup. For Phase 1R, include two-member MPE capture/replay, loop-crossing notes, monitor deduplication, quantize/undo and exact save/reopen fixtures. Add tests for these behavioral risks rather than tests that only mirror getters or UI scaffolding.

Claim milestone completion only with its evidence. Full M8 hardware-command parity and exact OXI algorithm equivalence are outside this plan. Generator/Minimap layouts begin from explicit provisional defaults and are validated in the native application. Stop at the full plan's M10 acceptance condition, or report a specific external validation gate that remains open.
