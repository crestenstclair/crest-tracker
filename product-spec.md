# Crest Tracker — current product specification

21 September 2026. This consolidates the user's decisions, including corrections made after the original screens. Architecture mechanisms and unresolved interaction choices are separated in the architecture guide and decision log.

## Purpose and boundary

Crest Tracker is a separate sequencing library integrated with Crest Synth by the host application. Tracker emits MIDI; Synth owns sound generation, instruments, patches, samples, SoundFonts, mixer and audio FX. Synth must remain independently usable and must not acquire concepts such as phrase, chain, cursor, tracker FX or controller navigation. Existing Synth functionality remains intact.

The frontend renders the authoritative application state through projections. Editing dispatches commands; widgets do not keep competing copies of song or phrase content. Start with video game controllers. Later MIDI input devices and configurable mappings enter through the same semantic input boundary. Explicit state plus pure transitions and an event loop is the architectural direction; it does not imply that playback has no state.

## Musical hierarchy and reuse

| Object | Role |
|---|---|
| Set | Complete saved workspace, containing songs, composition references and the relevant Synth state/resource references. Instruments and Custom chord banks are shared resources, not duplicated per song. |
| Song | Up to 16 independently progressing tracks; key/scale, tempo and meter context; selected order of chains in each track. |
| Track | Ordered chain occurrences with its own playback cursor stack. It is not an instrument route. |
| Chain | Reusable ordered phrase references, with repeats and other sequencing controls. |
| Phrase | Reusable notes and three FX slots with length, division, optional local scale and sequencer type. |
| Occurrence | A particular use of a shared chain or phrase. Playback position and active effect state are isolated per occurrence. |

Song editing follows the LSDJ/M8 hierarchy. The desktop-style Expanded view and the Minimap do not introduce a second arrangement or an independently editable Pattern store. Different lengths and divisions align by musical time.

## Phrase view

The scrolling phrase grid contains **Note, Instrument/Channel target, Velocity, FX1, FX2 and FX3**. The precise column order follows the current Figma screen; the target belongs to each note/event row.

- Choose a specific instrument and route directly to that instrument, or choose a MIDI channel when the intent is to address the whole channel. External endpoint selection is needed where several MIDI destinations exist.
- Instrument is not a phrase-settings field. Track routing is not a separate feature.
- Notes show names such as C4, or interval labels in scale display mode. Do not expose raw MIDI note numbers as the primary editor label.
- Numeric editing uses decimal values, not hexadecimal. Percentage amounts use a 1–100 style presentation, with zero/off where meaningful. Tempo remains actual BPM and intervals retain musical units; not every FX parameter is a percentage.
- Empty note cells may still contain effects. Note-off/cut and note duration/gate must be represented explicitly; exact entry defaults remain an implementation decision.
- Phrase settings include local scale, length, division and type. All settings must work in the first phrase implementation, not merely be drawn.
- Editing a shared reference offers Make Unique or Edit Shared, with a remembered choice. Shared References shows where the definition is actually used. The exact remembered-choice scope is a proposal; the current architecture recommends session scope unless persistence is explicit.

## Phrase forms and time

Tracker, Drum Machine, Euclidean and Randomizer forms share one underlying note/event representation and the same ticking timeline. Each form may have its own authoring/generator model, but it resolves to the same NoteData and phrase FX contract. Merely switching a view must not regenerate or rewrite music.

Support quarter, eighth, sixteenth, thirty-second and sixty-fourth divisions, with triplets and other subdivisions, variable phrase lengths and time signatures. The 1/64 requirement is rhythmic resolution. Tracks remain capped at 16; MIDI channels are 1–16 per endpoint.

There are no tables. Phrase length and grid division provide the relevant rhythmic flexibility directly. Cursors must account for repeats, jumps and local phrase/chain/song progress independently from editor focus.

Drum Machine has a user-intended design reference still to be confirmed against the available materials. Euclidean and Randomizer layouts remain open. Current Figma representations are scaffolding for those modes, not approval of every control.

## Scale-aware music entry

- Select the song key/scale in Settings; phrases may override the local scale.
- Tracker, Euclidean and Random note entry/generation skip non-scale pitches by default.
- Notes and Intervals are alternate views of the same stored pitch. Preserve accidentals and octave information. A scale-degree ordinal is not always the same as a chromatic interval label.
- Allow intentional non-scale pitches through an offset FX, a global setting, or a phrase-view override.
- Fixed drum trigger mappings must remain usable; the proposal is to exempt explicitly fixed triggers from automatic repitching.
- The architecture proposes preserving existing authored pitches when changing scale, while affecting subsequent edits and generation. Do not silently transpose existing notes without a deliberate policy and command.

## Chords

The user chooses the intended quality: major, minor, diminished and other explicit shapes. Chord expansion is not scale-quantized, and the sequencer must not silently change chord quality to make it fit.

Each song has an immutable **Default** bank derived from its key/scale, showing fitting chords for each interval. “Immutable” means read-only for that song/key context; changing the song key derives a new matching bank. Chosen chord shapes must remain stable in saved music. **Custom** chords are editable, shareable resources and can be copied from existing chords or created through chord search. The exact catalog, bank capacities and exotic-scale derivation policy remain to be specified.

Use the previous Crest Tracker chord-bank/editor references as the design basis. Chord selection in an FX cell is contextual:

1. Focus the chord effect/value and hold **Edit + Up** to open the popover.
2. Hold Edit and use Left/Right to cycle chords.
3. Release Edit to navigate to bottom actions such as Save.

ADD exposes the interval relative to the root. Its interval editor spans an octave either side of the root and need not look literally like a piano. It supports selecting intervals and saving the result; Save Chord computes/saves the resulting chord. The later Edit+Up popover convention supersedes an earlier suggestion to use Shift+Up for this contextual editor.

Plan chord FX for voicing/inversions and spread from the start of the harmony work. OXI ONE is a behavior reference, not a license to change Crest's explicit-chord rule. The current proposal separates CRD shape, VOI named voicing/inversion, and SPR added octave-doubled voices. CRD + VOI + SPR occupies the three available slots. Exact open-voicing fixtures and numeric spread mapping still need a Crest decision. Auto voicing is a separate future proposal, with Manual as the proposed default.

## Phrase MIDI effects

MIDI/sequencer FX are foundational in Phase 1. Use three typed slots with meaningful units, targets, scope, lifetime and deterministic stage ordering. Effects may transform notes, schedule timing, emit MIDI controls or change sequencing flow; they do not perform DSP in Tracker.

Use documented M8 behavior as a reference and map an explicit supported subset to Crest. Do not promise wholesale parity before reviewing each command against the Synth MIDI capabilities. Initial families include pitch offset, chord/ADD/voicing/spread, velocity/gate/cut, delay/retrigger/probability, CC/bend/temporary sends, phrase jumps and tempo. Chain/song flow effects are implemented with those arrangement layers.

The effects browser opens as an **Edit + Up popover**, with fast Up/Down navigation. Chord bank selection and temporary-send editing are also popovers, not separate full-screen detours. A temporary reverb/send command temporarily overrides an addressed channel send; it does not change the saved mixer baseline. Scope, expiry, simultaneous override arbitration and cleanup on stop/seek are part of the model.

## Song views and editing

**Chains (default):** M8-style Song grid with up to 16 tracks, horizontally windowed as the screen requires. Each track orders chains. Shift+Right opens its selected chain; from Chain it opens the selected phrase. Playback can be at different chain and phrase positions in each track.

**Expanded:** enable multi-select, select multiple chain occurrences and open Expanded Tracker. Show the top-level chain context and every contained phrase, separated visibly and aligned by musical time. Retain source reference identity for edits. Save View Group stores a convenient selection. Shift+Left returns to Song. The prior “note interleave” approach was rejected; it must not be treated as a requirement.

**Minimap:** an additional top-level Song mode inspired by the supplied Stria screenshot. Show a compact overview of track activity. The current 16-lane, vertical-time mockup is exploratory; the user is still deciding the display. See [Minimap notes](design/song-minimap.md). It shares the same source data and cursors as other views.

## Navigation

| Context / gesture | Result |
|---|---|
| App starts | Song view |
| Song, Shift+Up | Settings |
| Settings, Shift+Down | Song through the Quick Jump destination; see the conflict note below |
| Song, Shift+Down | Mixer |
| Other views, Shift+Down | Quick Jump |
| Song chain cell, Shift+Right | Chain |
| Chain phrase cell, Shift+Right | Phrase |
| Phrase, Shift+Up | Phrase settings |
| Expanded, Shift+Left | Song |
| Song, Shift+Back | Main menu |
| FX/chord/send value, Edit+Up | Contextual popover |
| Chord popover, held Edit+Left/Right | Cycle chord |
| Release Edit | Allow ordinary navigation to Save/bottom actions |
| Set/song management, Shift+Up | Song settings and song actions |

The later instruction “Shift Down from any other view opens Quick Jump” supersedes the earlier direct Settings→Song shortcut. The direct-return behavior remains worth a final navigation review; do not implement both conflicting bindings silently. Semantic buttons are the contract; physical gamepad mapping is configurable and platform-specific.

The top-level menu has **Tracker, Instruments, Chord Bank, Mixer, Sends, Settings**. “Tracker” returns to the song workflow. The current Synth instruments/mixer/sends/settings pages are brought into the host, by reuse where possible or by presentation reimplementation if necessary.

## Settings and Set management

Retain the existing Synth settings. Add current BPM, default BPM, song key/scale, global volume, MIDI devices and clock in/out options, default sample starting directory and SoundFont starting directory. Current tempo, song initial tempo and the default for new work need distinct ownership. Starting directories are machine preferences, not portable absolute paths embedded in song content.

Set/song management supports current-set rename and save, opening a set, switching songs, and song edit/create/delete actions through Song settings. Instruments and Custom chord banks are shared; song Default chord banks are derived from each song's key. Persistence must retain references safely and preserve the complete workspace state.

## Interaction states to retain

Include empty, stopped, playing, narrow track-window and multi-select Song states; phrase Notes/Intervals; shared-reference inspection and Make Unique; Edit Shared / remembered choice; FX type/value selection; instrument versus MIDI-channel target selection; Default versus Custom chord banks, copy-to-Custom, chord search, voicing/ADD editors and Save Chord; set open/save-as/unsaved changes; song create/delete; temporary sends; missing resources and unsupported capabilities.

See the [screen index](design/screen-index.md) for the actual frames and [open decisions](open-decisions.md) for unsettled behavior. Screens are specifications and prototypes, not evidence of implemented runtime support.
