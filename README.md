# Crest Tracker implementation handoff

Prepared 21 September 2026. Start here when opening a repository or a fresh implementation session.

The [Crest Tracker Figma file](https://www.figma.com/design/mvRQEFZc5Oi2ZbRkoHhXjH/Crest-Tracker) is the live product and interaction source of truth. These documents preserve the current decisions and provide a coding guide. The latest explicit user decisions override older mockups or historical notes. Proposals and unresolved details are identified rather than silently treated as requirements.

## Read in this order

1. [Current product specification](product-spec.md): confirmed behavior, navigation, screens, scale and chord rules.
2. [Architecture guide](Crest-Tracker-Architecture-v1.md): version 4, despite the historical filename; domain ownership, storage, cursors, timing, MIDI bridge, FX and event contracts.
3. [Implementation plan](implementation-plan.md): staged deliverables and completion gates, beginning with the Synth boundary audit and playable phrase.
4. [Open decisions](open-decisions.md): questions that need implementation evidence or design iteration.
5. [Implementation kickoff](implementation-kickoff.md): a reusable brief for the first coding session.

## Design and supporting material

- [Screen and board index](design/screen-index.md) links directly to the Figma frames, including architecture and event diagrams.
- [Event map and contracts](event-map.md) describes command, fact, snapshot and MIDI boundaries.
- [Design contracts export](design/design-contracts.md) captures the current Figma guide, contracts and flow text.
- `design/figma-*.json` captures all six Tracker pages as text, frame metadata and prototype links. `design/tokens.json` preserves variable definitions and typography. These are implementation snapshots, not a native `.fig` backup or application schema.
- [Song Minimap notes](design/song-minimap.md) explain the new exploratory mode. PNG previews are included for key screens and diagrams.
- [References](references/README.md) maps the user's screenshots and the OXI chord-mode reference to their intended use.
- [Historical design notes](archive/Crest-Tracker-Design-Notes.md) preserve the early discussion. They predate several corrections and are not the current specification.
- `manifest.json` lists package files and SHA-256 checksums.

## Non-negotiable starting points

- Crest Synth remains its own sound/MIDI library, with existing behavior preserved. Tracker domain code does not own Synth instruments, voices, DSP or mixer internals.
- One common timed NoteData representation drives Tracker, Drum, Euclidean and Random forms. Phrase MIDI FX belong in the first playable slice.
- Phrase-grid events select an instrument directly or an endpoint/MIDI channel. No hidden track route or phrase-settings instrument selector.
- Independent occurrence cursors share exact musical time. Rhythmic divisions through 1/64, subdivisions and meters are not a 64-channel requirement.
- Song, Expanded and Minimap are projections of the same shared chains and phrases; no second pattern model.
- Scale-aware entry/generation is the default. Notes/Intervals changes display only. Explicit chords retain their chosen quality and pitches.

## What is not yet verified

The Crest Synth repository, public APIs, language/build system and runtime have not been inspected here. Phase 0 must establish the MIDI addressing/control capabilities and reusable UI boundaries. This package is a design and architecture handoff, not a claim that the integration or application has been implemented. Drum, Euclidean, Random and Minimap visual details remain provisional where noted.
