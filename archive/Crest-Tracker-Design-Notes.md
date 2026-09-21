# Crest Tracker design notes

Planning draft · September 18, 2026

Crest Tracker will use a reusable song, chain, and phrase model, with a controller-oriented hierarchical editor and an expanded multi-track editor over the same data. The expanded editor should provide the useful part of a conventional pattern tracker: seeing and editing concurrent parts together. Changing views must preserve the arrangement, references, timing, and playback state.

This document records the current requirements and proposes how the two views can work together. Requirements below come from the planning discussion. Details labeled as proposals remain design recommendations, not settled implementation decisions. Comparisons with LSDJ, Dirtywave M8, and Polyend describe the workflow distinction raised in that discussion, rather than a compatibility specification for those products.

The subsequent discussion confirmed the shared hierarchical model and expanded editing view. Alignment by musical time is a straightforward implementation detail. The design work should focus on the editing and grouping experience.

## Established requirements

- Crest Tracker is a separate library that imports Crest Synth and uses it as a library.
- Crest Tracker owns sequencing and MIDI emission. Tracker concerns must not enter Crest Synth.
- Crest Synth retains its existing functionality, including synthesis and mixing. It remains the service that receives MIDI and produces sound. “Service” describes its responsibility; it does not require a separate process or network protocol.
- A set contains the full saved state, including the existing Crest Synth state and the tracker state.
- Sets contain songs. A song supports up to 16 tracks.
- Each track has an ordered arrangement of chain references. Chains contain ordered phrase references. Phrases contain the musical note and FX information, or the mode-specific information that generates it.
- Chains and phrases are reusable. A chain can appear at multiple song positions, and a phrase can appear in multiple chains.
- Tracks advance independently through their arrangements. Playback tracks the song position, chain position, and phrase position separately for each track.
- Chain-level controls include repeats. Exact repeat placement and any additional controls remain to be defined.
- Both a hierarchical LSDJ-style experience and a wider desktop tracker experience should be available. Hierarchical editing remains the foundation for controller use.
- Phrases can have different lengths and phrase-level rhythmic grids, including triplets.
- Phrase modes will include normal tracker, drum machine, Euclidean, and randomizer.
- There will be no tables of the LSDJ/M8 kind. Phrase timing and length are explicit settings rather than table-driven workarounds.

## Interface and feature requirements

The following records the next feature pass as specified. It captures intended behavior without filling in controls or parameter semantics that have not yet been described.

### Figma design work next

Copy the existing Crest Synth design in Figma and expand that copy for Crest Tracker. Use the existing Synth interface as the starting point and Dirtywave M8 as the reference for the song, chain, and phrase views. This is the next design activity; the feature capture itself does not constitute a Figma edit.

### Default song view

- Song view is the default view.
- Show 16 track columns in the M8-style song arrangement.
- The screen width determines how many track columns are visible; additional columns extend beyond the visible area. The exact navigation behavior for off-screen tracks has not yet been specified.
- Selecting a chain and pressing Shift + Right opens its chain view.
- Selecting a phrase in the chain view and pressing Shift + Right opens its phrase view.

### Settings

From song view, Shift + Up opens settings. Preserve the existing Crest Synth settings and add the following tracker-facing controls:

- BPM.
- Scale.
- MIDI configuration, including clock input and clock output.
- Global volume.
- Default BPM.

BPM and default BPM are recorded as separate controls. Their initialization and reset behavior have not yet been specified. Shift + Down from settings returns to song view.

### Navigation controls

| Starting context | Input | Destination or action |
| --- | --- | --- |
| Song view | Shift + Up | Settings |
| Settings | Shift + Down | Song view |
| Song view with a chain selected | Shift + Right | Selected chain view |
| Chain view with a phrase selected | Shift + Right | Selected phrase view |
| Phrase view | Shift + Up | Phrase settings |
| Song view | Shift + Back | Navigation menu |
| ADD interval field | Shift + Up | Interval selection view |
| Interval selection view with Save focused | Select | Save the selected intervals to create the chord |

Other return paths and button assignments remain unspecified.

### Phrase view and settings

The normal tracker phrase view is a grid that scrolls vertically. Its columns are:

| Note | Velocity | FX 1 | FX 2 | FX 3 |
| --- | --- | --- | --- | --- |
| Named note | Velocity value | First effect | Second effect | Third effect |

Shift + Up opens settings for the current phrase:

- Local scale.
- Length.
- Division, including the previously specified triplet support.
- Type: normal tracker, drum machine, Euclidean, or randomizer.

The specialized phrase types retain their previously recorded scope; their individual layouts can be designed later.

### Values and note entry

- Use decimal values rather than hexadecimal notation.
- The requested numeric value convention is 1–100, displayed as a percentage.
- Notes have specialized entry and display their note names rather than numeric note identifiers.
- Exact parameter mappings and the treatment of values with musical units, such as BPM and intervals, have not yet been specified. Do not silently replace the requested range with 0–100 or introduce hexadecimal values.

### FX

- Include effects that modify BPM.
- Use the documented Dirtywave M8 FX as the reference for the effect repertoire and interaction. The detailed effect list and mappings will be captured in a later pass.
- Adapt reverb send to act as a temporary channel send. Related send effects should follow the same approach.
- The duration and restoration behavior of a temporary send remain to be specified.
- Keep the previously established sequencing and MIDI boundary, and the exclusion of tables, when mapping reference effects into Crest Tracker.

### Navigation menu

Shift + Back from song view opens a menu with these entries:

1. Song View.
2. Instruments.
3. Chord Bank.

### Chord Bank

Base the Chord Bank on the previous Crest Tracker implementation. It lets the user program a bank of chords and includes a large collection of standard chords. The previous implementation is a reference to inspect during design work; it has not been inspected for this feature capture.

### ADD effect and chord creation

- Add an **ADD** effect for modifying chords by adding intervals from the root.
- Selecting ADD and moving to its value field shows the interval from the root that the effect will add.
- Shift + Up from that field opens an interval selection view spanning one octave below and one octave above the root.
- The view can use a piano-like representation, but it does not need to resemble a literal keyboard.
- Up and Down navigate the available intervals, and the user can select multiple intervals to add. The exact interval-toggle button has not yet been specified.
- Focusing **Save** and pressing Select saves the interval selection to create the chord.
- Also provide a **Save Chord** button on that view. It computes the resulting chord from the selected intervals and saves it to the Chord Bank.

Save and Save Chord are recorded as the two actions described. Their precise distinction, and the chord naming behavior when an interval collection has multiple or no conventional names, remain for a later pass.

## The boundary with Crest Synth

The dependency points from Crest Tracker to Crest Synth. Crest Synth must remain useful without Crest Tracker and must not import tracker types or know about songs, chains, phrases, view groups, or generator modes.

Crest Tracker resolves the arrangement and emits timed MIDI events. Crest Synth receives those events through its existing MIDI-facing boundary and handles sound generation, voices, patches, and mixing. In-process use should preserve that same responsibility boundary.

Tracker FX belong in one of two categories: operations on sequencing, such as repeats or transposition, and operations that emit supported MIDI messages. Existing audio effects and mixer behavior remain owned by Crest Synth. A tracker command must not require Crest Synth to interpret phrase rows or chain state.

For persistence, the proposed set container composes tracker-owned state with Synth-owned state. The Synth portion is saved and restored through Synth's existing public facilities and retains its own schema. A complete set does not require adding tracker fields to a Synth-owned set type. The exact container and migration format should be decided after inspecting the existing Synth persistence API.

The choice of 16 musical tracks is a project constraint. Track identity and MIDI routing should be separate concepts; this document does not assume that one track means one voice or permanently binds a track number to a channel number.

## Proposed domain model

Distinguish reusable definitions from their placements. A phrase definition contains musical content; a phrase reference places that content within a chain. A chain definition contains phrase references; a chain reference places it within a track's arrangement.

| Object | Responsibility |
| --- | --- |
| Set | Full saved project state: Synth state, songs, reusable content, and tracker settings |
| Song | Transport settings and up to 16 tracks |
| Track | Musical lane, MIDI destination, and ordered chain references |
| Chain reference | A placement of a chain in a track, with occurrence identity and any supported placement settings |
| Chain | An ordered list of phrase references |
| Phrase reference | A placement of a phrase within a chain, with any supported repeat or transform settings |
| Phrase | Length, rhythmic grid, mode, and musical content or generation parameters |
| View group | Optional saved editor selection for opening related tracks and a time range together |

Proposed storage: reusable chains and phrases live in set-level collections with stable IDs. Songs and chains refer to those IDs. This supports reuse across the set, but the pool scope remains a recommendation; it is not essential to the hybrid view.

Each placement also needs a stable identity. Two placements may refer to the same phrase or chain while occupying different places in the arrangement. A repeated placement additionally has a playback iteration. These distinctions let the editor identify exactly what the user is looking at without duplicating musical content.

The authoritative musical structure ends at phrases. A conventional multi-track pattern is not an additional musical container in this proposal.

## How the two views fit together

The reference workflows differ in what they group. The hierarchical workflow orders reusable parts independently on each track. The conventional pattern workflow orders blocks that contain every track together.

Use the hierarchical model as the common representation. Obtain the desktop experience by expanding a bounded portion of that arrangement across multiple tracks. Both views use the same editing commands and the same playback engine.

### Hierarchical view

The song view shows tracks as columns and chain placements as ordered entries. Enter a placement to inspect its chain; enter a phrase reference to edit the phrase. Back returns to the parent while retaining the selection.

Each track has its own playback indicators at the song, chain, and phrase levels. A displayed song row is an arrangement index within a track. It must not silently become a synchronization barrier across all tracks.

### Expanded tracker view

From the song view, select a chain placement and invoke **Expand together**. That placement provides a musical time anchor. The editor opens a bounded time window and shows what the selected tracks actually play during it, expanding their chain and phrase references as necessary.

Recommended default: show the selected chain occurrence's time span, rounded or extended only by an explicit view setting. The user can change the displayed span to a bar, several bars, or another bounded selection. That view setting does not change playback.

Tracks appear beside one another; musical time runs down the page. Columns contain familiar note, velocity, and FX fields. Phrase IDs, local row numbers, and boundaries remain visible, so the user can understand where the displayed material comes from and open its source phrase.

The expanded view may cross several phrases or chains in one track while remaining inside a single phrase in another. It displays the actual continuation of the arrangement. It does not loop a short phrase, add silence, or restart a long phrase merely to fill a rectangle.

For an initial implementation, expand all enabled song tracks and allow the visible subset to be saved. This keeps the controller action simple. Desktop users can additionally select several columns before expanding.

## Shared musical time

Matching local row numbers across tracks is only correct when their rhythmic grids, starts, and lengths align. The expanded view must align musical time, while retaining local phrase coordinates.

Consider three illustrative parts, all starting together:

- Drums: a 16-row phrase at sixteenth-note spacing, lasting four quarter-note beats.
- Bass: a 12-row phrase at sixteenth-note spacing, lasting three beats.
- Lead: a 12-row phrase at eighth-note-triplet spacing, lasting four beats.

Assume each chain explicitly contains the next phrase reference. The following table samples phrase positions at whole beats; it does not show every note or subdivision. Row numbers are zero-based and decimal.

| Elapsed quarter-note beats | Drums | Bass | Lead |
| --- | --- | --- | --- |
| 0 | D0 row 0 | B0 row 0 | L0 row 0 |
| 1 | D0 row 4 | B0 row 4 | L0 row 3 |
| 2 | D0 row 8 | B0 row 8 | L0 row 6 |
| 3 | D0 row 12 | B1 row 0 | L0 row 9 |
| 4 | D1 row 0 | B1 row 4 | L1 row 0 |

At beat three, bass advances to B1 because its arrangement says to do so. Drums and lead continue their existing phrases. The combined view must show that boundary without moving the other tracks.

Likewise, chain entry 2 on one track need not start at the same time as chain entry 2 on another. Selecting the same visual song row across columns does not prove those chain placements are concurrent. The expansion action should resolve a real time range from the anchor and display the concurrent content in that range.

### Straight and triplet subdivisions

Phrase length and row duration are distinct settings. State the rhythmic grid unambiguously, such as sixteenth notes or eighth-note triplets. Changing the grid changes phrase duration when the row count stays fixed; changing the length changes the number of rows. Any command that preserves duration by resampling should be separate and explicit.

The scheduler uses exact musical positions, expressed as rational beat values or an adequately defined integer timebase. The display can use a common subdivision when it stays readable. For example, sixteenth-note starts and eighth-note-triplet starts both fit on twelfths of a quarter-note beat. This is a display convenience, not a requirement that every phrase acquire that grid.

Where a display position is not a valid row on a phrase's native grid, show it as unavailable for row entry. Distinguish that from a valid empty row. Finer subdivisions may be expanded locally or displayed with precise offsets when a uniformly expanded grid would become unwieldy. Never quantize existing notes simply because the user changes views or zoom.

One playback line can show the current song time in the combined view, while every track independently displays its active chain, phrase, local row, and repeat iteration. The song time is shared; the structural positions are independent.

## Editing and reuse

The expanded grid must be a fully usable editor. It cannot become a flattened copy that later needs to be synchronized back into chains.

Every editable cell resolves to its source phrase and its occurrence path: track, chain placement, chain iteration, phrase placement, phrase iteration, local row, and field. Repeated appearances of a shared phrase remain appearances of the same source.

| User action | Proposed behavior |
| --- | --- |
| Edit a note or FX field | Edit the source phrase and update every linked appearance |
| Open source | Navigate to the phrase and row underlying the selected cell |
| Make phrase unique here | Clone and reconnect enough of the reference path to isolate this placement, then edit the clone |
| Change phrase length or grid | Change its musical duration and recompute affected occurrences on every track using it |
| Switch views | Preserve playback, source selection, references, and undo history |
| Paste into several tracks | Apply edits to their resolved phrase rows; preserve or explicitly handle reference sharing |

The UI should visibly identify shared content and provide **Make unique here** as a deliberate action. Ordinary edits keep the reuse behavior consistent in both views. A small link indicator or usage count is preferable to prompting on every edit.

Isolation must account for shared parents. If a phrase reference lives inside a chain that is reused at several song positions, simply replacing that reference changes all uses of the chain. Isolating one song occurrence requires cloning the containing chain as well as the target phrase, then reconnecting that chain placement.

A single playback iteration of a repeated reference is not automatically an independent editable object. To make only that iteration different, an explicit command must split the repeat into placements and isolate the desired one. The UI must not imply that a generated occurrence already has independent storage.

For edits that span repeated appearances of the same phrase, identical edits can target the shared source once. Conflicting edits to the same source row should be surfaced and require explicit isolation; they must not silently resolve by whichever visible cell was processed last. A multi-track gesture should remain one undoable action.

## Grouping without introducing a second arrangement

The useful convenience is **open these parts together**, with optional **edit or move these placements together** commands. It does not require a second ordered list of patterns.

Propose an optional saved **view group** containing a name, song identity, track selection, anchor occurrence, and viewing span. Avoid calling it a set because set already means the full project state. A group has no notes, no independent playback cursor, and no responsibility for advancing the song.

For example, a user can save “Verse” as a group that opens the relevant tracks around a particular chain occurrence. Editing the group edits the underlying phrases. The group does not make the selected chains start together. If its anchor is deleted, mark the group unresolved rather than silently jumping to another occurrence.

For convenient conventional composition, offer a **Create aligned block** command as a later addition. It creates ordinary chain and phrase placements with a chosen common duration. Multi-track copy or reorder commands operate on those placements as one edit. These are authoring conveniences over the existing model.

There is a real limit: arbitrary independent track arrangements cannot always be reordered as conventional patterns without splitting phrases, materializing repeats, or otherwise changing the arrangement. Do not promise lossless conversion into a separate pattern order. The agreed goal here is two editing views; a separate pattern-song playback mode is outside this proposal.

## Controller behavior

Keep a single vocabulary across both views: navigate, enter, back, edit, select, copy, and make unique. The confirmed button assignments are recorded in the navigation controls table above; additional assignments can follow the existing controller design later.

In the hierarchical view, entering and backing out traverse song, chain, and phrase. **Expand together** is an additional view action from an arrangement selection. In the combined view, horizontal navigation selects tracks and fields, while vertical navigation moves through musical time or valid rows. Opening a source phrase uses the same phrase editor as hierarchical navigation.

The selected column's native phrase grid governs note entry. Moving to a track with a different grid should preserve the time anchor and indicate whether it lands on a valid row. The user can then move explicitly to an adjacent valid row; the editor should not silently write to a rounded position.

Changing views never restarts playback or converts content. Keep playback-follow behavior separate from editing focus so the song can continue while the user inspects another location.

## Phrase modes reserved for later design

All four modes belong to Crest Tracker and participate in the same chain and song model. Their detailed parameters and interfaces are deliberately deferred.

| Mode | Recorded intent | Proposed combined-view treatment |
| --- | --- | --- |
| Normal tracker | Explicit note and FX rows | Direct row editing |
| Drum machine | A drum-oriented phrase workflow | Show timed hits; open the drum editor for mode-specific editing |
| Euclidean | Generate rhythmic events from phrase settings | Show generated events and identify the source mode |
| Randomizer | Generate musical events using randomization | Identify generated output and whether the preview is fixed or variable |

Each mode needs explicit duration and a common way to produce timed MIDI events. This keeps the arrangement and both views independent of the phrase's internal editing method.

Generated notes must not pretend to be directly authored tracker rows. Later design should choose whether individual edits become overrides or require an explicit **Bake to tracker phrase** command. Random seed persistence and repeat behavior also remain open. A preview and playback should agree whenever deterministic generation is selected.

## No tables

Do not add table objects, table editors, or table execution to the tracker. Phrase-level rhythmic grid and length are first-class settings. Repeats belong to the arrangement model at their defined scope.

This records a deliberate scope decision, not a claim that timing and length reproduce every behavior of tables in other trackers. Any future feature must be justified on its own and fit the established Synth boundary.

## Suggested first implementation

1. Define reusable phrase and chain definitions, their placements, phrase duration, and independent per-track traversal.
2. Implement a normal tracker phrase editor and the hierarchical song and chain views.
3. Add a bounded expansion function that resolves a song time range into occurrences with source identities. Use the same traversal and timing rules for playback and display.
4. Build the editable combined view over that expansion, including native-grid row entry, source navigation, and visible sharing.
5. Add reference isolation, saved view groups, and multi-track authoring conveniences.
6. Add the other phrase modes through the same timing and MIDI boundary.

The initial expansion function can cache resolved occurrences, but that cache is derived data. It must not become a second saved musical representation.

Before implementation, settle track-end and song-end behavior, the scope of repeat controls, note lifetime across boundaries and stop or seek, and how live length edits become effective during playback. The combined view cannot safely invent these rules; it must display the sequencer's actual behavior. Infinite loops and variable-duration constructs, if introduced later, require a bounded preview and explicit reachability handling.

## Review scenarios

- Switching between views during playback preserves the emitted MIDI sequence and track positions.
- Unequal phrase lengths advance independently, and the combined view displays the resulting boundaries.
- Straight and triplet grids preserve exact event times and distinguish empty native rows from unavailable display positions.
- Editing a reused phrase changes every linked occurrence in both views.
- Making one occurrence unique under a shared chain preserves the other uses.
- A combined edit involving multiple appearances of the same phrase handles conflicting source edits explicitly.
- Saving and loading a set retains tracker state and the complete existing Synth state without introducing tracker schema into Crest Synth.
