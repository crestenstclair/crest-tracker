# Crest Tracker — open decisions and capability gates

21 September 2026. These are not missing user requirements; they are the remaining choices needed to turn those requirements into reliable code. Use small repository investigations and focused design reviews to resolve them. Do not reopen confirmed boundaries without a reason.

| Decision | Current position | Resolve before |
|---|---|---|
| Synth language, APIs and hosting | Sibling source inspected: Rust 2021, `AppState::apply`, AppLoop, graph workers and native saved-session version 4. Prove the actual host integration and existing test coverage before implementation. | Phase 0 |
| Direct instrument addressing | Source exposes Patch-targeted normalized `AppEvent::Midi`; channel fanout is separate. Prove end-to-end host use and any external wire profile. Do not bypass MIDI with a Tracker-owned voice API. | First audible phrase |
| Mixer/send/patch MIDI profile | Audit supported CC/NRPN/SysEx mappings and state observation. Resource loading/snapshot lifecycle must be distinguished from live musical controls. | Phase 0; full UI in Phase 6 |
| Scheduling contract | Choose lookahead, timestamps, queue bounds and live-edit activation based on the actual audio/MIDI stack. Already delivered output may be uncancellable. | Phase 0/1 |
| Note duration and overlap | Default gate, explicit off/cut entry and same-address/same-pitch retrigger policy. Architecture proposes owned releases and stale-off protection. | Phase 1 |
| Phrase bounds and divisions | Maximum phrase length, exact tuplet presets, permitted ratios and UI steps. Through 1/64 plus subdivisions is confirmed. | Phase 1 |
| Meter editing | Numerator/denominator/grouping, allowed change positions and retiming versus view-only changes. Preserve exact musical positions unless a deliberate edit retimes them. | Phase 1 timing model |
| FX catalog | Review documented M8 commands, choose the supported Crest subset, define units/ranges/stages/lifetimes and mark unsupported Synth controls. There are always three phrase slots and no tables. | Phase 1 contracts |
| Flow arbitration | Same-time tempo conflicts, jump precedence, empty Song slots, end/loop policies and transition budgets are proposed in A04. | Phase 1 local flow; Phases 3/4 hierarchy |
| Temporary send arbitration | Architecture proposes an override stack over the latest baseline. Decide external unknown-baseline and reconnect/reset policy. | Phase 1 control FX |
| Live regeneration | When Euclidean/Random parameters take effect, manual overrides, bake semantics and how cached output is keyed by context/revision/seed. | Phase 2 |
| Drum layout and trigger pitch | User intended a separate drum reference. No dedicated drum reference was identified among the five supplied images saved here. Current layout is provisional; preserve fixed drum trigger pitches. | Phase 2 UI |
| Euclidean and Random layouts | User explicitly reserved these for further thought. Separate models must still resolve to common NoteData. | Phase 2 UI |
| Scale catalog and spelling | Supported scales, enharmonic spelling, interval/octave labels and unusual-scale Default-bank derivation. Authored-note preservation on key change is an architecture proposal. | Phase 1 basic scales; Phase 2 full bank |
| Default/Custom bank scope | User requires immutable per-song Default and shared Custom resources. Finalize capacity, chord search criteria, revisions, publish/adopt policy and portable export. | Phase 2; persistence Phase 5 |
| Voicing and spread fixtures | Define exact pitches for each named open/inversion preset and numeric spread mapping. OXI behavior is verified at the feature level; exact algorithm equivalence is not claimed. | Phase 2 |
| Auto voicing | Optional future proposal. If added, specify prior-voicing memory, ties, reset rules and voice budgets; do not alter chord quality. | Only if scheduled |
| Remember shared-edit choice | Session-only is proposed. If persistent, make it visible and resettable; still protect shared containing chains. | Phase 3 |
| Navigation edge case | Later Shift+Down → Quick Jump outside Song conflicts with earlier Settings→Song direct return. Later rule is used in the handoff; verify final binding before implementation. | Navigation wiring |
| Minimap representation | Mode requested; activity encoding, viewport/zoom, narrow screens, controller shortcut, focus and follow are exploratory. | Phase 4 projection |
| Minimap future timeline | Define finite preview under repeats/jumps and whether generative marks show a realization or an estimate. Never evaluate an unbounded arrangement to draw a view. | Phase 4 projection |
| Expanded edit targeting | Preserve source occurrence identity in musical-time alignment; exact mixed-division cell focus/selection mechanics need interaction validation. | Phase 4 |
| Shared resources and portable Song files | Confirmed: one Song document, saved sound revisions retained until explicit update, song-local routing/mix. Choose revision identity, retention/deletion, embedded/exported assets and schema representation; preserve exact resources without duplicating authoritative Synth patch models. | Phase 5 |
| Input/clock recovery | Controller repeat thresholds, focus loss, input mappings, external clock loss, reconnect and panic/reset semantics. | Phase 1 essentials; Phase 7 full support |

## Confirmed choices not to revisit accidentally

One Song is the sole document; New/Open replaces it and Save/Save As preserves it. No parent container or child-song management. Shared sounds keep saved revisions until explicit update; routing/mix is song-local. Also confirmed: Synth separation; controller-first semantic input; Song→Track→Chain→Phrase hierarchy; 16 tracks; per-note instrument/channel target; shared definitions with independent occurrence cursors; one NoteData representation and musical timeline; subdivisions through 1/64; phrase MIDI FX from the start; three FX slots; no tables; decimal presentation; scale-aware note entry with explicit escape paths; literal chord quality; read-only Default and editable Custom chord banks; contextual popovers; main-menu Mixer/Sends/Settings; Expanded and Minimap as views of the same composition.
