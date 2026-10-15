# Figma guide, contracts and flows — text snapshot

Captured 21 September 2026. Preserve the distinction between confirmed product behavior and proposals. See product-spec.md for later corrections and explicit decisions.

## Crest Tracker / Start here

[Open in Figma](https://www.figma.com/design/mvRQEFZc5Oi2ZbRkoHhXjH/Crest-Tracker?node-id=2030-3117)

CREST TRACKER  /  DEVELOPMENT REFERENCE

Crest Tracker / Start here

Source of truth for the Tracker experience. Start with Song, then follow the hierarchy or open the expanded tracker.

01 / Open Song ↗
Default entry. Order chains across 16 tracks. Shift Up opens Settings; Shift Down opens Mixer.

02 / Expand selected chains ↗
Enable multi-select, choose chain occurrences, then expand. Chain headers stay visible, phrase separators show structure, and every phrase scrolls on musical time.

03 / Phrase notes and destinations ↗
Note · Instrument · Velocity · FX 1 · FX 2 · FX 3. Choose a specific instrument or a whole MIDI channel in the grid.

04 / Contextual chord and FX popovers ↗
Edit + Up opens the picker. Edit + Left/Right cycles chords; release Edit, move down and Save. Default is read-only per song; copy or search into editable Custom. VOI / SPR preview the exact voiced notes.

05 / Shared references and copies ↗
Inspect every use. Before editing a reference, choose Create New or Edit Shared; the Remember option is included for review.

06 / Song file and shared libraries ↗
New, open, rename and save one Song document. Song File holds file actions; Song Settings edits the current Song. Sound definitions and Custom chords use the shared library. Sound revisions stay pinned until explicit update; routing/mix stays local. Default chords derive from the Song key/scale.

07 / Review the development contracts ↗
Confirmed requirements and proposed details are labeled separately. Drum awaits its reference; Euclidean and Randomizer layouts remain provisional.

08 / Prototype and visual flows ↗
Start from Song using the prototype link. Click controls and try Shift + arrows; E demonstrates Edit. Numeric editing and MIDI playback are illustrative.

09 / Architecture, events and implementation phases ↗
A00 defines boundaries; D01 maps domain events and D02 shows FX/cursors. A09–A10 are the implementation roadmap. A12–A14 cover scale entry, Default/Custom banks and chord voicing.

## 01 / Product boundary & data model

[Open in Figma](https://www.figma.com/design/mvRQEFZc5Oi2ZbRkoHhXjH/Crest-Tracker?node-id=2030-3653)

CREST TRACKER  /  DEVELOPMENT REFERENCE

01 / Product boundary & data model

Confirmed requirements and the storage relationships behind both views.

CONFIRMED / The boundary
Crest Tracker is a separate library importing Crest Synth. Tracker owns sequencing, time, the Song, chains, phrases, MIDI events and its UI. Crest Synth remains the MIDI-driven sound server, including instruments, effects and mixer. Tracker domain objects and sequencing behavior do not enter Synth.

CONFIRMED / Full state and shared banks
One Song contains the complete musical document: one arrangement, reusable Chain/Phrase definitions, pinned shared sound/Custom chord references and local Synth routing/mix. No parent container or additional songs exist inside it. New/Open replace the document; Save/Save As preserve it. Library sound edits require explicit revision adoption. Default chords derive from this Song key/scale.

CONFIRMED / Sequence model
A song has up to 16 tracks. Tracks order chain references; chains order phrase references and repeats. Phrases contain note events and three FX slots. Tracks advance independently through song, chain and phrase positions. Phrase length and division determine musical duration. All phrase forms share timed note and FX data on one musical clock. Divisions extend through 1/64 with triplets and other subdivisions; time signatures determine bars and beat grouping.

CONFIRMED / Destination belongs in the note grid
Each phrase note has an Instrument column. Its target is either a specific instrument or a whole MIDI channel. Selecting an instrument delivers directly to that instrument; selecting a channel emits to that channel. There is no track-routing page, and Instrument is not a phrase-settings field.

CONFIRMED / Reuse and edit choice
Shared References lists the song, track, song position, chain and phrase position of every use. Editing a direct reference offers Create New or Edit Shared, with a Remember choice. Create New leads to Make Unique for the current occurrence. The remembered-choice scope is a design detail still to settle.

IMPLEMENTATION CONTRACT / MIDI delivery
Event target = Instrument ID or MIDI Channel (plus endpoint when needed). Tracker resolves that target through existing Synth MIDI delivery capabilities and emits MIDI. It must not introduce a tracker opcode or sequencing API into Synth. If direct instrument MIDI addressing is missing, resolve that library-interface question before implementation.

## 02 / Two views, one sequence

[Open in Figma](https://www.figma.com/design/mvRQEFZc5Oi2ZbRkoHhXjH/Crest-Tracker?node-id=2030-3657)

CREST TRACKER  /  DEVELOPMENT REFERENCE

02 / Two views, one sequence

LSDJ-style editing is the core. The expanded tracker is a musical-time projection of the same content.

CONFIRMED / Enter from selected chains
Enable multi-select in Song, select multiple chain occurrences, then enter Expanded Tracker. Keep the top-level chain headers visible and break each chain out into all of its phrases. Small separators mark phrase boundaries; the complete sequence scrolls vertically. Shift + Left exits directly to Song.

CONFIRMED / One musical timeline
Align events by musical time even when phrase lengths, divisions and chain durations differ. Both views edit the same musical data. The expanded view does not introduce pattern objects or a second sequencing engine. The current canvas shows complete eight-beat chain examples in a scrolling viewport.

CONFIRMED / Group actions
Expand Together opens the selected chain occurrences. Save View Group remembers that selection for convenient reopening. Copy References preserves sharing; Duplicate as Unique makes independent content. Move and Repeat operate on the selected arrangement references.

CONFIRMED / Protect referenced phrases
Before changing a direct reference, offer to create a new phrase. Create New isolates the selected occurrence; Edit Shared changes all uses. Show References reveals every location. Remember Choice is present as a prototype option; the exact persistence and reset behavior remain open.

PROPOSED / Copy mechanics
If the selected phrase sits inside a shared chain, Make Unique duplicates that containing chain as well, then relinks only the selected song occurrence. Preserve the original definitions and every other reference. Show the proposed copies in the existing Make Unique menu before committing.

PROPOSED / Projection and focus
Retain source phrase ID, local row, FX slot and arrangement occurrence for every projected event. Keep exact musical times internally. Phrase boundary bands can align across the visible lanes without moving events. Continue the time ruler through chain sections, pin headers, and preserve editing focus independently from playback.

## 03 / Navigation & controller contract

[Open in Figma](https://www.figma.com/design/mvRQEFZc5Oi2ZbRkoHhXjH/Crest-Tracker?node-id=2030-3661)

CREST TRACKER  /  DEVELOPMENT REFERENCE

03 / Navigation & controller contract

Confirmed shortcuts, proposed complementary controls, and focus behavior.

CONFIRMED / Core hierarchy
Song is the default view. Shift + Right on a selected chain opens Chain; Shift + Right on a phrase reference opens Phrase. Shift + Up in Song opens global Settings. Shift + Up in Phrase opens Phrase Settings. Shift + Left from Expanded Tracker returns to Song.

CONFIRMED / Mixer, quick jump and Song File
Shift + Down in Song opens Mixer; elsewhere it opens Quick Jump. Shift + Back in Song opens Tracker / Instruments / Chord Bank / Mixer / Sends / Settings. Quick Jump includes Song File, Sends and Settings. Shift + Up in Song File opens settings for the current Song: name, meter and arrangement. File actions never add, reorder or delete child songs.

CONFIRMED / Contextual FX controls
Focus the FX command and hold Edit + Up to open the command popover; Up/Down browse quickly. Focus a chord FX value and hold Edit + Up to open the chord-bank popover. While holding Edit, Left/Right cycle chords. Release Edit, move down to Save, then Select. Temporary sends use a popover too.

PROPOSED / Note and destination editing
Edit modifies notes directly in the grid: Up/Down for pitch, Left/Right for octave. The compact named-note picker is a desktop convenience. The Instrument cell opens a picker for a specific instrument or a whole MIDI channel. Opening a picker does not yet mutate shared content; protect the reference before applying an edit.

PROTOTYPE / Review controls
Click cells, actions and tabs to follow the prototype. Shift + arrows cover the primary hierarchy. E stands in for the semantic Edit button in keyboard demonstrations; Shift + Backspace stands in for Shift + Back. Escape closes a popover or returns. These are demonstration bindings, not mandatory hardware mappings.

PROPOSED / Focus and return
Cyan indicates focus, magenta selection/context, and green playback or active intervals. Popovers retain the originating phrase cell underneath and restore it on close. Quick Jump preserves the current editing context when cancelled. Numeric values and generator controls are illustrated, not a live MIDI simulation.

## 04 / Phrases, modes & musical values

[Open in Figma](https://www.figma.com/design/mvRQEFZc5Oi2ZbRkoHhXjH/Crest-Tracker?node-id=2030-3665)

CREST TRACKER  /  DEVELOPMENT REFERENCE

04 / Phrases, modes & musical values

Note rows, phrase grids, generative modes, and display units.

CONFIRMED / Phrase columns
The grid contains Note or Interval, Instrument, Velocity and FX 1–3. Instrument chooses a specific instrument or a MIDI channel for each note. Scale-only input is the default; Settings and a phrase-view override permit chromatic entry. Notes/Intervals show the same underlying pitch data. OFS intentionally moves a note/root by signed semitones outside the scale.

CONFIRMED / Grid and length
No tables and no table editor. Phrase length is a step count; division controls each step’s musical duration, including triplets. Sixteen steps at 1/16 span four quarter-note beats. Twelve steps at 1/16 triplet span two beats.

CONFIRMED / Four phrase types
Normal Tracker, Drum Machine, Euclidean and Randomizer are supported phrase types. All modes produce timed MIDI events with phrase-level event targets. The Phrase Type picker is approved.

PENDING DESIGN / Drum machine
The current drum screen is provisional. Replace its layout with the user’s drum-machine reference when supplied. Preserve the per-event instrument/MIDI-channel model. Do not treat the existing four-lane sketch as the approved design.

PENDING DESIGN / Euclidean and Randomizer
The user is still considering these designs. Existing control layouts are exploratory examples only. Generator preview/commit behavior, random seed persistence, mode conversion, and exact control sets are not locked. Preserve authored material when testing conversions.

CONFIRMED + PROPOSED / Numeric values
No hexadecimal. Normalized amounts use 1–100 percentages and pitches use note names. Proposed exceptions retain meaningful units: BPM, MIDI channels 1–16, step counts, note fractions and signed semitones. Explicit off/zero behavior and MIDI rounding remain implementation decisions.

## 05 / FX & chord contract

[Open in Figma](https://www.figma.com/design/mvRQEFZc5Oi2ZbRkoHhXjH/Crest-Tracker?node-id=2030-3669)

CREST TRACKER  /  DEVELOPMENT REFERENCE

05 / FX & chord contract

M8-inspired sequencing effects, MIDI-safe execution, and the interval/chord workflow.

CONFIRMED / Popover behavior
FX selection and temporary sends are contextual popovers over the phrase. The command popover opens with Edit + Up on the command; Up/Down browse. The chord-value popover opens with Edit + Up on the chord value; Edit + Left/Right cycles chords. Release Edit and navigate down to Save.

CONFIRMED / Default and Custom chords
Default is a read-only per-song bank derived from Settings key/scale, showing fitting chord choices for each root interval. Custom is editable and shared through the library: copy Default or Custom entries, or save results from Chord Search. Selected chord quality and intervals stay explicit and are never scale-quantized. Edit + Up opens the contextual popover; Edit + Left/Right browses; release Edit, move down and select Save.

CONFIRMED / Interval editing and search
ADD shows the interval from the root. Edit + Up opens the interval selector spanning an octave either side. Save applies the selection; Save Chord creates an editable Custom chord with a suggested name. Chord Search filters chosen notes, intervals, root or quality, with an optional scale-fit filter. Unnamed sets remain valid custom chords.

CONFIRMED / MIDI FX from the start
Typed FX are part of shared NoteData and all phrase forms from phase 1. OFS is an explicit chromatic offset. CRD preserves the chosen shape; VOI and SPR transform its voicing without scale correction. Three slots remain explicit. Browser, temporary send and voicing controls are contextual popovers. See A08 and A12–A14 for stage order, cleanup and proposed precise ranges.

PROPOSED / Temporary send lifetime
The popover currently illustrates a 35% send for one step, then restoration. Duration is provisional. Resolve overlapping channel sends, stop/cancel restoration and manual mixer changes before implementation. A note’s selected target determines the relevant destination; no track-routing layer is added.

REFERENCE / Dirtywave M8
The official M8 manual documents the reference effects. The supplied Crest Tracker screenshots guide chord-bank and editor structure. Chord and send interaction rules in this file take precedence over M8 navigation behavior.

## 06 / Song lifecycle & implementation gates

[Open in Figma](https://www.figma.com/design/mvRQEFZc5Oi2ZbRkoHhXjH/Crest-Tracker?node-id=2030-3673)

CREST TRACKER  /  DEVELOPMENT REFERENCE

06 / Song lifecycle & implementation gates

Persistence, transport, inherited Synth functionality, and decisions to settle before coding.

CONFIRMED / Settings
Settings includes current BPM, song key/scale, default Scale-only/Chromatic note entry, Notes/Intervals display, MIDI clock in/out and devices, global volume, default BPM, sample starting directory and SoundFont starting directory. Phrase view can override inherited pitch policy; phrase settings retain local scale, length, division and type. Machine paths remain local preferences.

CONFIRMED / One Song document
Song is the sole saved document. Song File supports New/Open/Save/Save As; settings edit its name and meter. New/Open replace the current Song after Save/Discard/Cancel when dirty. Sound definitions and Custom chords use pinned library references; routing/mix remains song-local. Changing key refreshes read-only Default without rewriting Custom shapes.

IMPLEMENTATION PROPOSAL / Persistence and transport
Architecture A02 defines a versioned single-Song document with pinned shared-resource revisions and local sound/mix state. A04 defines independent cursor stacks; A08 defines note and parameter cleanup. Exact retention/export, timing and control protocols remain implementation gates. There is no multi-song container schema to implement or migrate.

IMPLEMENTATION PROPOSALS / Pitch and harmony
Scale-aware input, chromatic overrides, two pitch displays, immutable Default banks and editable Custom chords are confirmed. Proposed details: store absolute pitches; key changes leave authored notes unchanged; pin chosen chord shape revisions; derive fitting chord candidates for arbitrary scales. OXI-informed named voicings and octave-doubling spread are specified on A14. Exact open voicings and numeric spread mapping remain explicitly reviewable Crest contracts.

OPEN / Remaining product and integration decisions
Drum, Euclidean and Randomizer visual details; full chord catalog and M8 FX subset; exact Synth MIDI addressing/parameter profile; phrase limits and note overlap; external clock loss timeout. Architecture v1 supplies explicit proposals for shared banks, copy transactions, cursor jumps and temporary-send lifetime.

ACCEPTANCE / Verify before implementation
Synth-only consumers remain unchanged. Both Tracker views emit identical MIDI for the same data. Instrument and channel destinations resolve per note. Expanded chains reveal every phrase and preserve exact time. Referenced edits offer a copy; usage links identify actual locations. Popovers restore focus. Shift Down and Shift Up obey the context rules. No tables exist.

## 01 / Controller hierarchy

[Open in Figma](https://www.figma.com/design/mvRQEFZc5Oi2ZbRkoHhXjH/Crest-Tracker?node-id=2033-3652)

01 / Controller hierarchy

Default entry is Song. Navigation preserves the selected occurrence and local editing position.

Song

Default arrangement. Select chains; each track keeps its own play position.

SHIFT →

Chain

Ordered phrases with repeats. Shared definitions remain reusable.

SHIFT →

Phrase

Note · instrument · velocity · three FX columns.

SHIFT ↑

Phrase settings

Local scale, length, division, type and inherited pitch-entry policy.

SONG / Shift + Up → Settings. Shift + Down → Mixer. Shift + Back → navigation menu: Tracker · Instruments · Chord Bank · Mixer · Sends · Settings.

EVERY OTHER VIEW / Shift + Down → Quick Jump. Expanded Tracker → Song with Shift + Left.

SONG FILE / Shift + Up → current Song settings: name, meter and arrangement. New/Open replace the document through Save/Discard/Cancel.

## 02 / ADD, intervals & chord bank

[Open in Figma](https://www.figma.com/design/mvRQEFZc5Oi2ZbRkoHhXjH/Crest-Tracker?node-id=2033-3656)

02 / ADD, intervals & chord bank

An interval edit begins at the selected phrase FX cell and returns to that context.

Chord FX value

Focus the chord effect value in the phrase grid.

EDIT ↑

Chord popover

Browse Default / Custom with Edit + Left/Right. Default offers Copy to Custom.

EDIT INTERVALS

Interval editor

Root-centered interval toggles and note preview.

SAVE CHORD

Save chord

Save a new editable Custom chord; preserve the source Default entry.

SAVE / Release Edit, navigate down to Save and Select. Apply to the originating phrase cell.

FX COMMAND / Edit + Up on the command opens the FX list; Up/Down browse quickly.

SHARED EDIT / Before mutation, offer Create New or Edit Shared, with a Remember choice.

## 03 / Expanded editing & shared sources

[Open in Figma](https://www.figma.com/design/mvRQEFZc5Oi2ZbRkoHhXjH/Crest-Tracker?node-id=2033-3660)

03 / Expanded editing & shared sources

Both views edit one sequence. A view group adds organization without becoming a pattern object.

Multi-select chains

Select chain occurrences from Song. Save View Group remains available.

EXPAND

Expanded tracker

Chain headers, separated phrases, per-note destinations and one time ruler.

EDIT REFERENCE

Reference choice

Create New, Edit Shared, Remember, or inspect every use.

CREATE NEW

Make unique

Isolate this occurrence; copy the containing chain when necessary.

SHOW REFERENCES / See song, track, arrangement position, chain and phrase position for every use.

SCROLL / Continue through all phrases in each selected chain. Separators identify phrase boundaries.

RETURN / Shift + Left exits Expanded Tracker to Song. Group actions and Save View Group are retained.
