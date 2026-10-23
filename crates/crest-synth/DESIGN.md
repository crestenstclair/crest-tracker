# Crest Synth — Architecture and Current Status

Figma is the product, visual, and interaction authority. This document records
current architecture, durable decisions, and known implementation gaps.
`AGENTS.md` is the short working contract; source and tests provide exact
implementation details and evidence.

## Authority and maintenance

- Read only the sections relevant to the task. Do not reload historical plans,
  archived specs, or Git history as routine context.
- Follow current user direction. Figma fixtures and existing implementation
  constants do not establish product limits.
- Update the relevant section when behavior changes. Do not append running
  test reports, copied constants/tokens, completed checklists, or phase plans.
- Use source, production-path tests, and measured output to establish what
  works. A planning document or an old passing run is not current proof.
- OpenSpec is available only when explicitly requested. Its change artifacts
  are temporary; incorporate durable decisions here and remove completed
  artifacts. Do not maintain a second collection of product specifications.
- Requested research reports are scoped decision inputs. They do not become
  product authorities or context that every task must read.

## Capacity and technology policy

Target modern desktop hardware. Instruments, effects, Patches, tracks,
parameters, and voices have no arbitrary product count caps. Resource budgets
must be configurable for the hardware. Prepare resources off the callback;
keep each callback's admitted work bounded by its prepared configuration.
Real-time safety does not require fixed compile-time product counts.

The runtime still contains fixed-size collections and admission checks. Those
are implementation debt, not product requirements. This documentation cleanup
does not remove them from code or prove configurable capacity has shipped.
Do not copy their values into new specs, use them to reject otherwise suitable
engines, or preserve them as architectural invariants.

Patch admission, scalar snapshots, graph racks and observations share one
build-time storage budget, defaulting to 32. Set `CREST_SYNTH_PATCH_CAPACITY`
when building for different hardware. It must be a positive integer. A saved session admission limit can be adjusted through `SetPatchCapacity` up to
that prepared budget. It cannot fall below installed count or change during
structural preparation. Old sessions default to the prepared budget. Tracker
exposes this setting in Song Settings. Runtime storage resizing remains
unimplemented; callback storage stays preallocated and Copy.
Patch count is independent of the sixteen MIDI channels and mixer lanes; new
Patch defaults cycle those routes after the first sixteen.

Source synthesizer and effect DSP from existing Rust, C++, and C projects.
Crest may provide wrappers, capability integration, preparation, and control
projection; it should not develop new custom synthesizers or effects.
Select self-contained instruments and effects with coherent native controls
that suit controller editing; imported preset-library instruments may focus
on browsing and performance. Embedding and redistribution license suitability,
including dependencies, is a selection gate rather than a deferred concern.
Prefer coherent upstream DSP collections and preserve musical coverage when
selecting simpler replacements.
The production Sample renderer uses sfizz; SoundFont uses the full RustySynth
renderer. Legacy Sample audition, Reverb, and Delay remain for compatibility.
New catalog entries wrap complete upstream DSP. Old renderer test helpers are
not evidence for the new production path.

## Current alignment status

The production UI is a functional webview projection. Readable controls,
usable workflows, truthful state, and stable focus govern completion. Exact
pixel matching is not a completion gate; claim Figma fidelity only after a
relevant native comparison. Previously accepted resizing needs another handoff
only when a change or regression affects it.

Overview, Instrument/FX Detail, registry choices, Sample Detail/Browser,
SoundFont loading, session lifecycle, physical MIDI, and page navigation have
production-path witnesses. Their presence does not prove complete visual or
workflow parity. Native Settings comparison, shared visual polish, broader
controller coverage, and packaged cross-platform acceptance remain separate gaps.
Multi-select remains unavailable and is not required for
current feature completeness; the reserved Select button defines no behavior.

Instrument/FX Detail uses one `detailShellHtml` composition over reducer-owned
subjects and descriptor-ordered sections. Choice uses one option composition:
registry identity/order, independent CURRENT/focus markers, enabled admission,
visible unavailability, exact origin, and correlated lifecycle. A current-value
selection closes as an accepted no-op. File Browser retains its own file and
Sample-preview composition.

Requested structural values stay separate from acknowledged active values
through Loading, Validating, Preparing, Activating, Ready, and typed failure.
Only graph activation acknowledgement commits the replacement. Compatible
scalar snapshots target the prepared revision while the source graph ignores
incompatible revisions. Nothing silently substitutes a capability or asset.

New Patch names use zero-based installation order (`00`, `01`, …), independent
of persistent Patch identities. Saved and imported names are preserved. Hosted
Tracker intercepts root Down/Back to open its Main Menu and Mixer Up to open
track detail; native detail/choice return ownership stays in Synth. Standalone
Synth retains its own top-level navigation.

The footer is the sole visual action-guide owner. Numeric presentation uses
projected ranges, steps, and units rather than arbitrary decimal precision.
PATCH Overview labels Return as opening Detail. The compatibility Detail action
does not advertise a duplicate Return binding; registry choices retain their
own confirmation guide.
The envelope SVG uses descriptor-linked durations with a stable `log1p` display
scale; sustain has level, not duration. Reflow changes presentation only.

## Visual and interaction reference

The normative product, visual, and interaction source of truth is the live
[Crest Synth — Controller-First UI Redesign](https://www.figma.com/design/kdQMw8dYUZtv2UxJPo0sXU/Crest-Synth-%E2%80%94-Controller-First-UI-Redesign?node-id=0-1).
The primary authored nodes are:

- [Responsive Front-End Contract — 98:2](https://www.figma.com/design/kdQMw8dYUZtv2UxJPo0sXU/Crest-Synth-%E2%80%94-Controller-First-UI-Redesign?node-id=98-2)
- [Patch Overview — 95:202](https://www.figma.com/design/kdQMw8dYUZtv2UxJPo0sXU/Crest-Synth-%E2%80%94-Controller-First-UI-Redesign?node-id=95-202)
- [Instrument Detail — 37:7](https://www.figma.com/design/kdQMw8dYUZtv2UxJPo0sXU/Crest-Synth-%E2%80%94-Controller-First-UI-Redesign?node-id=37-7)
- [FX Detail — 38:60](https://www.figma.com/design/kdQMw8dYUZtv2UxJPo0sXU/Crest-Synth-%E2%80%94-Controller-First-UI-Redesign?node-id=38-60)
- [Sample Detail — 39:92](https://www.figma.com/design/kdQMw8dYUZtv2UxJPo0sXU/Crest-Synth-%E2%80%94-Controller-First-UI-Redesign?node-id=39-92)
- [Sample Browser — 41:138](https://www.figma.com/design/kdQMw8dYUZtv2UxJPo0sXU/Crest-Synth-%E2%80%94-Controller-First-UI-Redesign?node-id=41-138)
- [Mixer — 42:3](https://www.figma.com/design/kdQMw8dYUZtv2UxJPo0sXU/Crest-Synth-%E2%80%94-Controller-First-UI-Redesign?node-id=42-3)
- [Engine Options — 48:173](https://www.figma.com/design/kdQMw8dYUZtv2UxJPo0sXU/Crest-Synth-%E2%80%94-Controller-First-UI-Redesign?node-id=48-173)
- [Post FX Options — 48:207](https://www.figma.com/design/kdQMw8dYUZtv2UxJPo0sXU/Crest-Synth-%E2%80%94-Controller-First-UI-Redesign?node-id=48-207)
- [Interaction Map — 49:3](https://www.figma.com/design/kdQMw8dYUZtv2UxJPo0sXU/Crest-Synth-%E2%80%94-Controller-First-UI-Redesign?node-id=49-3)
- [Page Layout — 153:184](https://www.figma.com/design/kdQMw8dYUZtv2UxJPo0sXU/Crest-Synth-%E2%80%94-Controller-First-UI-Redesign?node-id=153-184)

The inspected Figma nodes contain no prototype reactions. Their annotations,
screen states, component specimens, and interaction map remain valid design
evidence, but click-through behavior cannot resolve every edge case. In
particular, the file says Select enters multi-select without defining selection
accumulation, batch operations, cancellation, or a complete visual treatment.
Product behavior resolves Sample Browser preview as holding Start on a focused
sample row and stopping on release; Start is reserved elsewhere.

## Product as built

Crest Synth is a standalone, controller-first MIDI instrument host. The
production composition is a Rust application using Tauri v2 with WKWebView on
macOS and WebKitGTK on Linux, plus CPAL stereo audio output. Normal startup
opens one clean, playable Sample
`00` document and starts a bounded repeating test MIDI pattern on channel 1:
the catalog demo's ascending/descending major-seventh arpeggio across three
octaves, played as eighth notes at 120 BPM with 200 ms gates.
`T` stops or restarts the pattern; the footer reports the available action.
Test playback pauses in Sample Browser and during session replacement. The
parsed `midi/Radiohead - Everything In Its Right Place - HiDef Compatible.mid`
fixture remains isolated to explicit demo and witness entry points. That
derived fixture differs from the retained source MIDI only by changing its
unavailable percussion bank-128/program-118 request to the installed
bank-128/program-0 drum kit; runtime capability fallback remains forbidden.
Crest Synth is not a DAW, arranger, or general-purpose sequencer.

The application currently provides:

- PATCH and MIXER as the only top-level contexts;
- sparse stable Patch identities; current runtime capacity debt is described above;
- SoundFont, Sample, Drum Rack, Braids, Mutable, DaisySP, STK, mda, MSFA DX7, and sfizz
  instruments in the production registry. SoundFont retains its HiDef identity
  and accepts local SF2 banks; DX7 imports single-voice and bank SysEx libraries;
- Sample installed by default with `Test Tone.wav` in
  `~/Music/Crest Synth/Samples`; the bundled WAV seeds a new library without
  overwriting an existing asset. `CREST_SAMPLE_LIBRARY_ROOT` and
  `CREST_SAMPLE_DEFAULT_ASSET` remain paired, validated overrides;
- one effect registry containing the legacy effects plus Airwindows, DaisySP,
  Mutable Clouds/Warps/Rings resonator, mda, Neural Amp Modeler, FFTConvolver,
  and Signalsmith pitch/formant processing;
- ordered Patch post-effect slots, named send-effect chains, and persistent Mixer tracks;
- descriptor-driven Patch Overview, Detail, generic Choice, shared File Browser,
  waveform/playhead projection, persistent Utility, and persistent Mixer
  Inspector surfaces;
- channel-based MIDI subscriptions in which any number of installed Patches
  may share one channel and each incoming message fans out to every matching
  Patch in stable installation order;
- physical gamepad discovery and button input through SDL3’s Gamepad API, with reducer-owned
  Settings button capture and per-user saved assignments;
- physical MIDI input discovery, exact opaque-identity selection, connection
  lifecycle, per-user selected-input preference, and a temporary Settings ·
  MIDI Devices system surface that suspends and restores the exact PATCH or
  MIXER focus without adding a third top-level context;
- level, pan, mute, solo, indexed sends, return occupancy/parameters/levels,
  pre-gate meters, Patch route/trim, and master gain;
- versioned saved state containing stable asset references and normalized
  control values, never decoded PCM or device state;
- native File-menu New, Open, Save, Save As, and guarded Close workflows over
  one shell-owned Untitled-or-path document identity and content-based clean
  baseline;
- retained deterministic, headless, controlled-negative, and physical-demo
  commands listed later in this file.

Normal `make run` resolves the composition-root-designated Sample capability
exactly and constructs Patch 1 `00`: MIDI channel 1, T00 at 0 dB Patch trim,
neutral envelope, capability-seeded voice settings, and empty post-effect
slots. It also installs the default Mixer, 0 dB master, and the
default bank of sixteen empty `INIT` send chains. Retained effects/buses diagnostic
scenes explicitly seed audible returns; new documents never consume that fixture.
The captured versioned session and its complete graph
are validated and prepared before audio or the window starts; a missing or
invalid designated capability is a typed fatal startup error, never registry
fallback. The initial graph receives the explicit test pattern through the same
control-side channel fan-out as physical MIDI. The pattern emits at most two
MIDI edges per window tick and never replays a catch-up burst. Its enabled flag
changes only through `AppState::apply`, stays outside saved-session content, and
pauses during browser audition and session replacement. `automaticMidi` remains
distinct from `physicalMidi` in input evidence. Neither source emits Patch targets.
The physical path uses `midir = 0.11.0`: CoreMIDI on Apple, ALSA on Linux, and
WinMM on Windows through default features. JACK and WinRT remain explicit
packaging-time opt-ins and are not enabled in the default build. Adapter
initialization, enumeration, port information, connection, retirement,
preference, and transport failures are typed; an unavailable backend leaves
the app open with truthful Settings state and never selects a similar name or
another port. The `demo-live-*` commands remain input-isolated bounded
autonomous witnesses and do not attach host MIDI devices.

## Architecture

```text
keyboard / controller / automatic or physical MIDI source
                       |
              physical input adapters
                       |
              SemanticAction / AppEvent
                       |
                 AppState::apply
                       |
          +------------+-------------+
          |                          |
 immutable semantic view      fixed audio projections
          |                          |
 Tauri webview projection    events / snapshots / graphs
                                     |
                              CPAL callback
                                     |
                           bounded stereo output
```

The non-negotiable mutation path is:

> physical input → semantic action/event → `AppState::apply` → view/audio
> projections

`AppState::apply` is the only product-state mutation path. Invalid events return
typed rejections and leave state and generation unchanged. UI, persistence,
MIDI, device, controller, asset, and serialization code are thin adapters.
Views receive immutable host-neutral data and emit semantic actions; they do
not own Patch, Mixer, focus, browser, or audio state.

The code is organized around hexagonal boundaries:

- domain types own Patches, capabilities, parameters, routing, Mixer state,
  assets, and their invariants;
- application/control code owns `AppState`, reducer transitions, semantic
  resolution, projection, orchestration ports, and correlation lifecycles;
- real-time code owns prepared render graphs, queues, voices, buses, and bounded
  scratch. Scalar snapshots own prepared vectors and transfer by swaps; retired
  snapshot ownership returns with the graph for destruction off callback;
- adapters own SoundFont/WAV parsing, filesystem access, MIDI fixtures,
  physical devices, native input, and Tauri integration;
- `webview-page/` paints the serialized semantic projection and emits no
  independent domain model;
- tests drive the same reducer, projection, preparation, and render seams used
  by the standalone binary.

`production_application` is the shared composition root for standalone Synth and a hosted application. `ApplicationHost` contributes a document and musical transport without replacing Synth's reducer, device authority or renderer. `HostedSynthControl` exposes validated operations, installed Patch identities, saved sound capture and correlated candidate restore; it exposes no mutable state, preparation workers or raw queues. Required Patch identities are checked before replacement graph staging, so a rejected host document leaves the prior sound and focus active.

During Tracker playback, native Synth actions join the host's ordered input queue.
Tracker refills scheduled audio before UI work and processes one input transaction
per control tick, including that input's child commands. Remaining input stays in
order for later ticks; native edits still reduce in Synth. This prevents queued
Mixer projections from consuming the transport window. If the internal producer
still misses its deadline, Tracker resumes from the first unscheduled beat with
fresh scheduling lead time. Notes, releases, expression and send lifetimes move
together; existing voices and effect tails continue without a cancellation or
an overdue attack burst. Deferred expression for already released, unpedaled
voices is discarded because their audio release tails may have finished during
the stall; lifetime cleanup and expression for held or sustained voices remain.
This introduces an audible timing delay, not a transport
stop. An active recording is interrupted with its take retained because its input
timestamps belong to the old timeline. External clock ownership and genuine
routing or voice-budget failures retain their existing stop behavior. Local graph
activation preserves the host timeline, queued events, unchanged engines and effect
tails. Only the edited engine is replaced; its next scheduled attack uses the new
engine. A complete document replacement still stops transport before activation.

`HostedSynthControl::set_browser_start` delivers a typed `SetBrowserStart` event for Sample or SoundFont browsing. Browser state remembers the last successful folder per asset kind and falls back to its configured starting directory or the existing default. Starting folders are shared machine preferences in `browser-starts.json` beside the Synth controller/MIDI configuration, not sound/session data. Interactive Synth loads them at startup; Tracker imports legacy local paths only when the shared file is absent and writes folder changes there atomically. Isolated event scenes omit this production preference load. The filesystem adapter exposes an opaque `@root` location on Unix; hosts validate accessible directories before persisting machine paths.

Tracker document replacement owns a blocking loading popover from file reading
and asset checks through sound preparation and acknowledged graph activation.
The host drops navigation, editing, transport and file commands during that work,
including queued inputs on the completion tick; held stick repeats require a
fresh edge afterward. The shared shell displays stage text and a progress spinner,
disables background controls, and honors reduced-motion preferences. Success or
failure releases the modal; missing assets return to the existing repair flow.

A host may provide a typed `WorkspaceProjection` containing rows, top tabs, a grid with selected/playing cell states and compact spacing, focus identity, text entry and hints. Lists scroll within the bounded content region; host hints reflect the current workspace and editing target. The existing page paints those through its shared shell bands, Patch rows, typography and tokens. The host cannot inject a second page, script, stylesheet or keyboard adapter. `WorkspaceEvent` admits focus, activation, text-draft changes, submission, and measured list-page capacity; its owner validates visible identities and operation tokens. List measurements are correlated to the first projected row and bounded by the current row count. Tracker owns paging and focus; the renderer only reports geometry on layout/resize and reveals the resulting selection. Native Synth projections and paint acknowledgments resume when the host workspace closes. Hidden Synth frames never produce painted evidence. Host-consumed semantic actions may replace their matching native footer hints through `HostView::action_overrides`; local Synth action hints remain intact. The host owns panel history and restoration of row identity, so sidebar operations do not replace the active editing target.

Keyboard and SDL input share the existing semantic vocabulary. The optional host action handler receives gestures after saved mapping, hold suppression and Edit arbitration; unconsumed actions retain the original Synth path. Hosted Shift+Q / Shift+Previous Patch emits Return for the parent menu. Standalone Synth retains Q/E behavior. No sequencing type enters Synth's state or callback. Tracker consumes Shift+Start for global settings and Shift+Down for its Main Menu or detail return, including while the native Synth workspace is visible. Native subordinate navigation remains available through a forwarded semantic action.

SDL initializes only the gamepad subsystem with its POSIX signal handlers disabled. Terminal SIGINT/SIGTERM retain the process's normal disposition instead of becoming unconsumed SDL Quit events. Window/menu Close still uses the document lifecycle and owned shutdown.

Tracker resolves queued semantic actions, workspace events, stick gestures and roll modifiers at its application tick. It finishes the commands generated by one input before resolving the next input against the resulting context. Pending host navigation therefore cannot race a subsequent detail/menu action. When native Synth owns the settled context, unconsumed actions return to its normal dispatch path; standalone Synth has no Tracker input queue.

Host timeline projections carry separate editing and playback coordinates. The shared renderer paints a dashed cyan editing cursor and a solid amber playback cursor; the host owns timing, viewport, selection and scrolling. Selected/playing cell states remain independent, so an arrangement can mark different rows in different columns. Timeline projections also admit duration/velocity marks, original outlines during previews, lane shading, a labeled ruler and optional normalized value traces. The host supplies every trace segment and raw-value label; the renderer never infers musical interpolation or owns expression data. Contextual fields carry enabled state and column spans shared with host navigation. Compact side panels, bottom action cards and a selection summary use the existing shell tokens and renderer.

Host footer guidance is projected after focus resolution. Tracker describes the focused field's directional edits, activation, page exits, transport and Piano modifiers, using the controller mapping received at its input boundary. Newlines group the controls in a wrapping, right-aligned footer; the renderer owns no action semantics. Native Synth pages retain their existing admitted-action guidance.

The complete source is forked into the Tracker workspace; `FORK.json` records the imported working-tree hashes and original commit. The Synth package still builds and runs independently with `cargo run -p crest-synth --bin crest-synth`.

There is one canonical public type per concept. Private transport values may
exist inside adapters, but duplicate public `Patch`, `MidiEvent`, `Parameter`,
focus, asset, or routing models are forbidden.

Control-side `AppState` snapshots share the Patch collection, Mixer and bus-return
bank through copy-on-write `Arc` storage. Accepted reducer edits detach only
the aggregate being changed; navigation and action-availability probes do not
deep-copy unchanged configurations. Saved-content comparison checks shared
identity before comparing detached values, so Mixer action probes do not repeatedly
scan unchanged Patch and return configurations. The immutable Patch-creation blueprint,
including installed defaults and structural preset choices, is also shared;
per-control availability probes never duplicate that catalog. No shared
control aggregate enters the callback. MIDI-only semantic projections replace
generation and hash while sharing immutable surface content. Their serialized
field order, values, and public projection contract remain unchanged. MIDI advances the last
published scalar snapshot's generation without rebuilding unchanged values,
including candidate values during graph activation.
Text-projection coherence checks compare shared body identity before scanning
text, while independently constructed projections retain full value equality.
The bounded event journal uses `VecDeque` for constant-time FIFO eviction;
chronological iteration and serialized records keep their existing contract.

## Canonical state and interaction invariants

Canonical state is divided conceptually into session, interaction, and runtime
data. Session data contains Patches, instrument/effect configurations, routes,
Mixer values, and stable asset references. Interaction data owns the top-level
context, singular semantic focus, mode, return path, and subordinate session.
Runtime data owns device/load status, graph generation, and correlated
structural-edit lifecycle—not device handles, decoders, prepared graphs, or
audio buffers.

- Focus uses stable semantic IDs, never rectangle coordinates, DOM positions,
  collection indices, or labels.
- Exactly one target is focused.
- Presentation reflow dispatches no action and cannot change track, row,
  Patch, subordinate subject, or return identity.
- PATCH and MIXER remember their prior semantic roots.
- One `PatchSubordinateSession` union owns Detail, Choice, or Sample Browser;
  subordinate surfaces do not stack.
- Choice/browser rows use stable choice or library-relative identities.
- Closing a subordinate surface restores the exact origin when valid; if a
  schema change removed it, the reducer selects the nearest enabled semantic
  sibling deterministically and exposes the repair in status.
- Requested and active structural values remain distinct through Loading,
  Validating, Preparing, Activating, Ready, Unavailable, Invalid, Cancelled, or
  typed failure states.
- Only one structural request is in flight application-wide. Failure, busy,
  early, stale, mismatched, or cancelled results never select a fallback.
- Correlated local graph changes carry unchanged engines and effects across the
  block-boundary swap. A replaced engine loses its own held voices; unrelated voices
  and effect tails continue. Whole-session replacement resets all live state.

The installed capability registries own available choices and ordered schema.
Reducers, projectors, racks, renderers, and demo orchestration must not switch
on concrete capability names to define fields or availability.
Instrument and effect providers classify capabilities by musical family or
processing type. Both pickers project only the focused option's group; no
second category cursor or browser-owned filtering state exists. Every available
capability remains reachable, with registry order preserved within each group.
Group navigation and item navigation within a group wrap in both directions,
skipping unavailable choices and groups. A single eligible target retains focus.
Browsing changes only focus; confirmation uses the
existing structural-edit path. EMPTY appears at the start of the first navigable
effect group, including when no effect providers are available.
Catalog validation detects duplicate choice IDs with a sorted borrowed index,
preserving declared option order and the first repeated-ID error without
quadratic scans of large asset catalogs.

## Hard real-time invariants

The audio callback performs bounded, preallocated work. It must never:

- allocate, deallocate, grow a collection, or drop the last reference to owned
  state;
- lock, wait, sleep, block, or invoke an API that may do so;
- perform file, console, network, MIDI-device, or UI I/O;
- parse, decode, resolve paths, resample, prepare effects/assets, or build a
  graph;
- log, format strings, panic, unwind, or destroy retired state;
- perform unbounded traversal, recursion, or capability discovery.

Different real-time data has different transport semantics:

| Data | Transport | Rule |
| --- | --- | --- |
| Immediate MIDI, preview, bounded commands | fixed-capacity SPSC event ring | bounded block-start delivery |
| Scheduled MIDI and temporary controls | separate fixed-capacity ordered event ring | absolute stereo frames; exact render offsets, graph revision and cancellation epoch |
| Scalar parameters | triple-buffered `ParameterSnapshot` | latest compatible complete snapshot wins |
| Structural changes | ownership-transfer queue of prepared graphs/assets | swap at block boundary; retire off-thread |
| Meters and RT health | atomics/latest `AudioObservationSnapshot` | decimated and polled by UI |
| Physical MIDI activity | latest compatible control-side snapshot | exact active revision, decimated to at most 30 Hz |

Scheduled deliveries retain future entries and share one per-block work budget
with immediate commands. The renderer splits at event sample offsets. A revision
fence accepts queued events across correlated local graph edits, whose Patch
identities remain stable. Whole-session and unscoped replacements advance the fence;
future revisions and events older than that fence remain rejected. Cancellation
invalidates old epochs at consumption and releases only Scheduled voices. Native
voice banks distinguish Immediate, Scheduled and Monitor sources, MIDI channels,
and optional attack identities; this protects overlapping same-pitch attacks and
owned expression. An identity-tagged AllNotesOff releases only that articulation, bypassing pedals while preserving its release envelope and the graph’s effect buffers. Raw channel controls remain channel-wide. Braids exposes an
engine-specific owned-expression profile: independent member/master raw14 bend
and RPN0 ranges add before exact native pitch admission, pressure gains multiply,
member CC74 is absolute, and scoped pedals combine without reviving released
voices. Unsupported engines and controls reject explicitly. A fixed-size
initialized-voice command carries the attack and both complete dimensions as one
budgeted callback operation; no partial initialization can render between packets.
The host owns MPE source/zone interpretation, allocation and musical lifetimes.
These native witnesses do not certify a physical MPE controller or endpoint.
Audio observations retain cumulative late-event count, maximum delivery lateness
in frames, and producer-sampled scheduled queue occupancy peak. Canceled or
wrong-graph events do not count as delivered late. Concurrent consumption can
make the sampled queue peak lower than its instantaneous maximum; these metrics
do not measure device or acoustic latency.

Optional shell performance recording (`CREST_PERF_DIR`) uses fixed atomic timing
histograms and a background JSONL writer. Audio callback instrumentation reads a
monotonic clock and updates atomics only; it never formats, allocates, locks, or
writes files. Native frame work, WebView DOM/layout, host work and projections have
separate timings, with playback separated from idle/loading. Histograms report
0.1 ms percentile upper bounds and exact maxima; snapshots are observational,
not product state. The writer flushes periodically and on normal shutdown.
Native IPC and paint acknowledgments are drained promptly, but full control ticks
and projections respect the shell's 16 ms cadence instead of running on every
acknowledgment. Audio rendering remains independent of that cadence.

Temporary send leases use configurable preallocated storage. The newest active
lease wins per Patch and bus; the mixer reads the current scalar baseline when no
lease applies, so manual saved edits remain visible after expiry. Fixed-frame
leases expire even when the command budget is exhausted. Hosts with musical
deadlines schedule an explicit owned end after tempo conversion. Publication or
capacity failure is visible and requires cancellation; Stop and whole-session replacement
clear leases without touching saved sends or killing return-effect tails.

CPAL publishes a bounded atomic audio clock correlated to callback and playback
host timestamps. Physical capture retains original MIDI bytes, polyphonic key
pressure, backend timestamp and connection revision before default monitoring.
On macOS, the host can map CoreMIDI timestamps against CoreAudio playback time;
unknown clock domains reject correlation. Allocation audits and native rendering
witnesses cover these paths; physical end-to-end latency remains a separate
measurement.

Graph-status and audio-observation snapshots have one callback writer. Their
versioned atomic payloads use a release fence after marking a write in progress
and an acquire fence before the reader's final version check. Readers retry on
overlap; publication uses bounded stores without waiting for readers.

Each asynchronous producer owns its SPSC queue or is merged on the control
side. A single-producer queue is never shared casually among UI, MIDI, and
workers. Queue pressure is explicit; note-off/all-notes-off has reserved
recovery behavior, replaceable scalars may be coalesced, and silent loss is
forbidden.
The renderer admits a configurable command budget per block, leaving excess
events queued in order so a concurrent producer cannot prolong the callback.

Each physical MIDI candidate owns a separate preallocated `rtrb` SPSC ingress,
and switching or losing that input releases only Immediate voices. Scheduled
accompaniment and host-owned monitoring remain independently owned. One reserved
recovery marker coalesces input cleanup and global panic, with global recovery
always taking precedence. The callback resolves the requested scope without
allocating or changing the event budget. Each connection also retains its
immutable connection revision, atomic enable gate, and fixed diagnostic
counters. The midir callback performs bounded status-byte classification and
normalizes only supported channel messages through `midly`; it never owns or
reads `AppState`, allocates, locks, logs, formats, blocks, or traverses a SysEx
payload. Control uses a bounded per-tick drain and rejects every event
whose revision is not the reducer-acknowledged active revision. Activity is a
presentation-only latest snapshot (count, last message/timestamps, diagnostics,
overflow epoch, revision); it never mutates or serializes into product state.

Connection preparation, discovery, retirement, and preference I/O run on one
bounded device worker. A candidate ingress stays disabled until matching
reducer acknowledgement. Switch, disconnect, loss, ingress overflow, audio
command saturation, and shutdown disable/discard ingress before recovery.
Input changes release Immediate voices; whole-subsystem shutdown uses global
all-notes-off. The audio command ring reserves one slot for the coalesced
recovery command. Backend handles move to the device worker for
consuming close/destruction, including shutdown-owned and stale prepared
handles.

Scalar publication must remain allocation- and destruction-free in the
callback. A worker constructs and
warms a complete graph—engines, effects, sample PCM/zones, routing, delay
memory, return topology, and scratch—before publication. The callback swaps at
a block boundary and transfers old graphs to a bounded return queue for worker
destruction. If retirement is back-pressured, callback-owned bounded retention
retries; the callback never destroys the graph.

Physical audio startup is two-stage: negotiate and validate a device config
without starting the stream, prepare the complete graph for its actual sample
rate and bounded block capacity, then let the owning device adapter start the
prepared renderer. Oversized native callbacks are rendered as consecutive
bounded blocks, never truncated. Device errors cross back through fixed atomic
status and are formatted/handled off callback.

## Audio/domain invariants

- A Patch owns a stable `PatchId`, label/MIDI mapping, instrument config,
  per-note `VoiceEnvelope`, ordered post-effect slots, output
  `MixerTrackId`, Patch-local trim and send amounts, and voice settings.
- A Patch MIDI channel is a subscription, never an exclusive owner. Multiple
  Patches may subscribe to the same channel; one incoming message fans out to
  every current subscriber in stable installation order. Changing a
  subscription is immediately effective control state and does not rebuild the
  structural graph. A channel with no subscribers emits no audio command.
- Mixer state owns persistent tracks independently of the
  Patch collection. Multiple Patches may share a track and empty tracks remain
  configurable.
- Patch trim, route, and send amounts are Patch controls. A Patch never owns
  track level, pan, mute, solo, or meter state. Mixer Inspector retains its
  separate sends from the summed track signal.
- Master gain has one canonical owner, projected in PATCH Utility and MIXER
  Inspector.
- Prepared storage, voice admission, and parameter layouts belong to the
  resource configuration. Existing fixed layouts remain implementation debt;
  they do not constrain the intended product or technology selection.
- Effect slot order is render order. Topology edits prepare and exchange a
  complete graph; there is no silent bypass or render-time graph mutation.
- Send returns start empty, named `INIT`, with sixteen destinations by default.
  The saved bank count and prepared vectors configure routing capacity; sixteen
  is not an admission limit. Each return owns an ordered chain drawn from the
  same effect registry as Patch post FX, plus its name and output level. Slot
  identities survive other slots being removed. Empty chains contribute silence.
  The existing decimated meter bank observes the first sixteen returns; queries
  beyond that report unavailable without limiting audio routing or chain count.
- The Mixer-owned Sends screen opens with `4` or `Ctrl+4`; Q/E selects a
  destination using the same bounded adjacent navigation as Patches. Return
  on Name opens keyboard text entry (Return commits, Escape cancels). Effect
  rows open the shared categorized registry choices; parameter rows use the
  existing descriptor controls and asset browser.
- Patch Utility exposes only occupied send chains and edits the focused
  Patch's independent send amounts. Each destination sums incoming audio,
  processes its ordered effects once, and adds the result at its own return
  volume to the master mix. Names, chains, and send amounts persist in sessions.
- Voice admission must honor the configured hardware budget and expose
  resource exhaustion. Existing admission and engine-native stealing behavior
  remain source-level behavior to account for when making that configurable.
- Every engine applies the Patch-owned ADSR to independent native note voices;
  one post-stem envelope is nonconforming.
  Repeated note-offs preserve an older voice's already-latched release rather
  than restarting its tail when another note on the same key ends.
- SoundFont uses one full upstream RustySynth synthesizer per prepared host
  voice, sharing an immutable parsed SoundFont. Each synthesizer admits the
  maximum overlapping regions for its selected preset. Native envelopes and
  filters remain upstream; the Patch ADSR retains independent note ownership.
  The vendored renderer preserves positive low-velocity signals instead of
  culling absolute gains below `0.001`; envelope-tail retirement is unchanged.
- Each Braids Patch owns independent voices and exposes upstream model choices;
  there is no global Braids voice pool.
  Model, Timbre, and Color declare `ScalarEdit`. In Detail, K+A/D steps the
  model without graph preparation; K+W opens the registry-backed Choice page.
  Timbre and Color support fine and coarse scalar edits. Shared ADSR and Utility
  controls retain their existing owners. Scalar Choice adjustment follows the
  descriptor's update category, so Sample Loop uses the same adjacent-choice
  path. Values still commit only through `AppState::apply` and reach the active
  renderer through compatible snapshots.
- Sample uses resident sfizz playback, with independently prepared host voices.
  The upstream voice bank and Braids refuse exhaustion without stealing another
  note. Voice-budget growth prepares a replacement engine through the structural
  lifecycle and commits only after activation; reductions change admission.
  Configurable engines preserve budgets across engine changes. Descriptor
  defaults seed new Patches and do not impose engine ceilings.
- Drum Rack composes sixteen independent Sample slots on MIDI notes 36–51
  (Ableton C1–D#2). Slots start empty; unmapped and empty notes do not consume
  note admission. Loaded pads play at Sample's reference pitch, with independent
  files, tuning, playback, and loop controls. The Patch envelope, voice budget,
  output, sends, and effects remain shared. All Sample banks prepare off callback;
  standalone Sample and Drum Rack share the worker-owned PCM cache, and graph
  admission accounts for every resident asset and private native copy.
  The leading Pad choice uses the existing scalar Choice workflow and persists
  with the Patch; it selects visible controls without changing MIDI routing.
  Visibility predicates govern presentation and preserve hidden values; enabled
  dependencies still validate control values.
- Instrument and effect scalar layouts use prepared vectors rather than fixed
  parameter counts. Callback ownership swaps avoid allocation or destruction
  when a compatible snapshot arrives. Render rejection becomes a typed routing
  failure observation instead of successful silence.
- Unsupported assets, engines, effects, devices, rates, and topology states
  produce typed visible errors. No engine/effect/asset silently substitutes or
  falls back.

The signal flow is fixed:

```text
MIDI
  ↓
Patch instrument
  ↓
ordered Patch post FX
  ↓
Patch trim
  ├──→ Patch sends through routed track level/pan/gate ─┐
  ↓
route and sum into a Mixer track
  ↓
track level / pan
  ├──→ pre-gate meter
  ↓
mute / solo gate                                       │
  ├──→ track sends ──→ sum each send input ←────────────┘
  │                          ↓
  │                   effects / return volume ─┐
  ↓                                           │
track dry mix    ←────────────────────────────┘
  ↓
master gain / master meter
  ↓
device sample clamp
  ↓
stereo device
```

Mute always wins. If any track is soloed, only soloed, non-muted tracks
contribute dry signal or sends. Patch sends tap individual post-FX/trim stems
through the routed track's fader, pan, and gate before track accumulation loses
Patch identity. Track sends tap the summed post-fader, post-gate track. Meters are
post-level/pan but pre-gate so muted tracks remain diagnosable. Feedback may
exist only inside bounded effect implementations, never as an arbitrary graph
cycle.

The master meter measures the final sum, including effect returns and master
gain, before the device clamps samples to full scale. Individual tracks can stay
below full scale while this sum overloads. Raw per-block measurements remain
available for diagnostics. Separate fixed-size display envelopes average RMS
energy and retain peaks before lossy observation delivery; a cumulative overload
count prevents the UI from missing clipped samples between updates. These
measurements never change audio. Mixer rails use a logarithmic dBFS scale, held
peak markers, slower numeric labels, and a persistent master CLIP indication.

Mixer processing skips tracks with no routed Patch stems and accumulates only
nonzero sends per source and bus. It retains the same sample and Patch
accumulation order, pre-gate meters, mute/solo rules, and independently processed
return tails. Audio command draining has a per-block work budget;
queue capacity alone cannot bound a concurrently replenished
producer. Excess commands, including note-offs and reserved recovery, remain
queued in order for following blocks.

## Asset and persistence boundaries

The selected physical MIDI identity is an application preference, not synth
session content. The shell resolves `midi-input.json` beneath the per-user
configuration directory (`~/Library/Application Support/crest-synth` on
macOS, `$XDG_CONFIG_HOME/crest-synth` or `~/.config/crest-synth` on Linux, and
`%APPDATA%\crest-synth` on Windows). Version 1 stores only the opaque identity
schema/value and last-known display name, using a temporary file plus rename.
Manual disconnect, runtime connection state, descriptors, handles, queues,
timestamps, diagnostics, and activity are never persisted. `SavedSession`
uses version 4 and contains none of those device fields. Versions 1 and 2
migrate their eight returns and send levels without changing effect values,
padding the bank with empty sends. Version 3 stores return names and ordered
effect chains and preserves configured bank sizes. Version 4 stores independent
Patch send amounts. Loading versions 1–3 copies each routed track's sends to its
Patches and clears those track sends to prevent doubling; unused-track sends
remain intact. Current documents require valid, bank-sized Patch send arrays.

SoundFont and Sample file work is off callback: resolve, validate, read, parse,
decode, resample/precompute, allocate/warm voices, then publish a complete
prepared graph. Callback objects retain upstream immutable asset data and
prepared voice state. Some upstream objects include names and parsed metadata;
that ownership does not authorize callback parsing, file access, mutation, or
destruction.

SoundFont preset identity is numeric bank/program; labels are exact authored
SF2 names. Ordering is bank then program, with source ordinal only for malformed
coordinate collisions. The application does not invent General MIDI names or
choose a nearby preset.

Each SoundFont Patch resolves immutable capability metadata against its own
asset reference. The installed engine descriptor remains stable; asset-scoped
descriptors supply that bank's authored preset choices. Selecting a new bank
requests its first playable preset in numeric bank/program order. Subsequent
preset edits keep the exact bank, and saved-session restore requires the exact
saved preset. Reducer and graph-request validation reject additional config
changes, including an unrequested preset during bank replacement. Worker-side
preparers resolve file metadata independently before complete-graph validation.

SF2 admission checks RIFF/list chunk arithmetic before parser allocation and
bounds numeric region expansion against the admitted resource budget.
Malformed and unavailable files produce explicit failures. Current numeric
admission thresholds remain in source and are not product requirements.
Worker-owned caches share active banks through weak references. The full
upstream SoundFont is shared by prepared voices; authored capability catalogs
remain control-side. The bundled `./sf2/HiDef.sf2` reference remains exact.
Installed macOS applications resolve that reference and the fixed MIDI fixture
inside `Contents/Resources`, independent of the working directory. Unbundled
development tools resolve built-in assets from Synth's compile-time source root,
including dependency hosts launched from another working directory. This adapter
accepts only relative built-in identities without parent traversal; imported
asset paths retain their separate catalog resolution. Missing bundle resources fail
explicitly rather than selecting a checkout copy.

The Sample adapter uses exact `hound = 3.5.1` (Apache-2.0, no optional
features). It accepts uncompressed RIFF/WAVE only: mono/stereo signed
PCM16/24/32 or IEEE float32 at supported rates. Source size, duration, prepared
PCM, deduplicated graph memory, and waveform-summary budgets are validated
before publication; their current thresholds live in source and must not
become fixed product limits. RF64, RIFX, compression, non-WAV data, extra
channels, malformed chunk arithmetic, non-finite samples, and assets exceeding
the admitted resources are typed failures.

Sample configuration stores a relative asset ID, root note 0–127 (default 60),
normalized playback/loop landmarks, OFF/FORWARD loop mode, a validated
crossfade within the loop window, and the shared envelope. Preview is original-pitch,
file-start, no-loop audio through the origin Patch's post-FX, trim, Mixer,
sends, returns, and gates. Stop uses a prepared 5 ms de-click. Preview never
commits an asset or mutates saved state.

The shared browser also selects DX7 SysEx, SFZ, NAM models, and WAV impulse
responses. Effect files open from Patch Detail or Mixer return parameters and
return to that exact origin. Imports validate before storing library-relative
references; assignment remains pending until the replacement graph activates.
Library folders are `SysEx`, `SFZ Libraries`, `NAM Models`, and `Impulse
Responses` beneath `~/Music/Crest Synth`.
Read-only choices with one option explain the available count (for example,
“1 preset available” for the bundled DX7 library). The shared projection derives
this text from the descriptor; importing a larger library enables preset selection.

SFZ import uses the upstream parser for includes and definitions, confines
referenced files to the selected folder, and embeds their samples into a
self-contained library. Unsupported opcodes, unresolved or corrupt samples,
and partial libraries fail explicitly. The library build uses resident sample
loading and removes streaming/garbage-worker paths from audio processing.
Crest maintains these adaptations because the selected sfizz repository is
archived. sfizz currently duplicates resident samples across host voices;
Sample admission accounts for private PCM copies separately from shared PCM.
This remains a memory optimization opportunity.
Delay-zero MIDI updates retain normalized event state without rewriting every
controller buffer each block. Delayed events still flush in timestamp order.
The embedded sfizz stage profiler is disabled; Crest owns callback observations
and external acceptance timing, avoiding clock reads in each native voice stage.
DaisySP caches unchanged SVF and modal coefficients and drum tone/decay
calculations, plus unchanged string/modal excitation and damping coefficients,
while advancing filter history, interpolation, noise, and gain every sample. Sample
filter methods remain visible for inlining; modal coefficient preparation runs
when controls change. Rings batches
independent modal filters for SIMD and reuses unchanged pickup weights, retaining
ordered modal sums. Elements reuses fixed center-pickup weights while advancing
its side-pickup LFO, batches the same modal equations, and caches coefficients
only after both alternating update phases agree, and accumulates four parallel
partial sums within the existing upstream-reference error tolerance. Its bowed
feedback and oscillator clocks are unchanged. STK BandedWG clears only the circular delay interval written
since its last clear and returns exact zero for a cleared, unexcited plucked
model; bowed models and nonzero tails always advance. Mesh2D computes each junction and its alternate-buffer
outgoing waves in one pass. Native witnesses compare these paths against the
retained upstream sources. Scalar snapshots revalidate changed values before
applying any edits; reset invalidates the validated cache. Idle host voices receive
the current pitch bend after reset at their next note-on; held and releasing
voices still receive every ordered bend. Prepared voice banks fill a reusable
envelope-gain block with scalar-equivalent stage transitions, then mix it without
per-sample stage dispatch; each voice retains its independent release history.

Catalog descriptors expose native parameters through existing generic lists.
Parameter descriptors attach names to categorical stepped values and normalized
selector bands without changing saved identities, scalar editing, or DSP encoding.
Adapters own names and boundaries from pinned upstream definitions, including
model/preset tables, waveforms, switches, and combined mode/amount controls.
Continuous bands resolve against the f32 value sent to DSP. Shared projections,
parameter rows, and demo status use these names; named rows omit raw index bounds.
Actual counts remain numeric, and continuous morphs retain continuous editing.
Catalog tests require names for every stepped control except explicit count
parameters and cover normalized selector boundaries. General upstream physical-unit
formatting for normalized numeric controls remains separate work.
Plaits includes all upstream models; Elements and Rings retain their native
physical models, and Rings also accepts effect input. The unstable DaisySP
analog and synthetic snare ports are replaced by the original Mutable Plaits
implementations, with explicit Mutable capability identities. No replacement
snare algorithm is authored by Crest.

Native sources and dependencies are pinned under `vendor/audio/sources.json`;
`vendor/audio/PROVENANCE.md` records build adaptations. Builds use local
vendored sources and require CMake plus a C++20 compiler. Native compilation
shares Cargo's job budget; complete native source/header trees and the bundled
SFZ input are tracked so Rust-only edits reuse native output. Keep the tracked
`capabilities/` directory even though Tauri permissions are inline: Tauri watches
it, and a missing directory forces a rebuild on every invocation. Redistribution notices
are bundled from `assets/licenses/UPSTREAM_AUDIO.txt`; Eigen's MPL-covered
source ships in the adjacent `EIGEN_SOURCE.tar.gz` resource. STK's upstream patent statements
remain documented qualifications, separate from its embedding license.

Native rate/block adapters use r8brain and prepared storage with contiguous
circular-buffer spans; upstream source blocks and converter clocks stay intact. Prepared FFT block
phases derived from stable Patch/voice identity spread convolution work across
voices while retaining the upstream
filter, output counts, latency, and native note/control clock. Exact silence
skips convolution; declared generators share their zero-input clock across
channels, and mono generators convert their output once. Reset restores the
prepared phase without priming DSP. STK renders at the prepared device rate
using its upstream rate-dependent calculations, without a rate adapter. Its
global rate never changes: each scoped native boundary selects the prepared
instance's rate and random state. BandedWG delay storage follows that rate and
the lowest supported note/bend across its presets. On Darwin, a pthread key allocated
during preparation selects that state without first-render C++ TLS allocation.
Current graph mixing does not compensate latency between parallel paths. Warps can use its
built-in carrier or explicitly labeled stereo input channels; mda TalkBox
expects separately authored carrier/modulator channels. Neither provides an
independent Patch/bus sidechain route. These routing constraints are not a
claim of complete plugin-host equivalence.

Return or Shift+W on Sample File or SoundFont File opens the same in-app FileBrowser page.
The page uses the existing semantic navigation, confirm, back, and preview
controls; no platform asset dialog is involved. W/S navigates rows, Return
enters folders or selects a file, Shift+S returns unchanged, and holding Space
on a Sample file auditions it. Parent and Cancel are reachable list rows.
`FileBrowserState` owns the exact origin and focus; the page remains subordinate
to the origin's PATCH or MIXER context, never a new top-level context.

Shared `AssetFileId`, `FileBrowserFolderId`, row, and listing types are defined
in `synth::file_browser`. `FilesystemFileBrowser` owns directory navigation and
WAV/SF2 filtering by `AssetKind`; engine adapters own decoding and metadata.
`FileLibraryRuntime` shares one capacity-one listing/import worker between
Sample and SoundFont. Listings are correlated by exact semantic origin, asset kind, and folder;
the reducer rejects stale imports by origin, generation, and graph revision.
SoundFonts use `~/Music/Crest Synth/SoundFonts`; SF2 rows show source size and
validation-on-selection. Hold-to-preview remains a Sample operation. SoundFont
playback uses the active Patch's MIDI route after graph acknowledgement.
The serialized projection carries immutable `assetDescriptors`,
`assetScopedChoices`, and source-specific `NavigatePage` actions. The event log
records page intent, asset-kind-correlated listings, and imported descriptor
payloads. Settings replaces the visible Patch page even when PATCH
remains the suspended context: full and MIDI generation-only StateTree
validation accept a missing Patch page only on a system surface in that context.

The library root also offers Home and, on macOS, Volumes locations. External
location identities are transient and cannot be restored as saved assets.
Selecting an external Sample or a SoundFont emits a reducer-owned import request correlated
to its origin, generation, and graph revision. A capacity-one shell filesystem
worker validates it and copies it into `Imported/` with content reuse and no
overwrite. Only its resulting library-relative identity enters structural
assignment; cancellation, failure, and stale completion preserve the active
asset. Browsing and importing never block the callback or window tick. Nested
folder results select their first row initially, then preserve stable focus
on refresh. The projected asset row shows `BROWSE`; it is not a scalar edit.
The footer remains the sole keyboard-guide owner.

Imported SoundFonts persist as library-relative references and exact preset
identities in the existing version-2 saved session. Restore bounds the saved
Patch count before asset work, resolves all bank descriptors off-thread, and
prepares the complete candidate before acknowledgement can replace the active
session. No catalogs, numeric banks, or external browser locations are saved.
Removing the original external file does not affect its imported copy.

On macOS, the filesystem adapter checks zero-byte files for Dropbox's legacy
placeholder attribute before WAV parsing. These report `DownloadRequired` and
show Make available offline guidance in the browser and after failed import;
the active asset remains unchanged. Empty local files report `EmptyFile` rather
than an unsupported container. Downloaded WAV bytes take precedence over any
retained placeholder attribute. Selecting again re-reads the file; leaving and
re-entering its folder refreshes metadata. Crest does not initiate cloud
downloads or change Dropbox storage settings. This correction addresses legacy
zero-byte Dropbox placeholders; other cloud-provider hydration is not claimed.
The regression sets the real macOS Dropbox attribute on a temporary empty
file, checks listing/read/import failures, then replaces it with WAV bytes and
verifies successful import even with retained metadata. Production-catalog
inspection of an existing Dropbox tracker WAV confirmed the typed result.
Focused Sample tests and native browser paint verified actionable text,
unavailable state, retry, and unchanged saved assignment/focus on failure.

Sample Detail renders bounded min/max waveform bars with HTML/CSS, including
both source channels, and current descriptor-linked playback/loop landmarks.
Missing or asset-incompatible summaries produce `WAVEFORM UNAVAILABLE` without
invented geometry. Prepared summaries become acknowledged control-side data
on both Engine selection and asset assignment; the previous active waveform
remains visible while replacement is pending. Scalar landmark edits do not
replace the decoded summary. The engine-choice regression in
`tests/phase7_sample_workflows.rs` exercises options, worker preparation,
audio block activation, acknowledgement, and Detail projection without first
assigning a file in the browser. Prepared session replacement also carries the
waveform summaries belonging to its complete graph, hydrating startup and Open through `AppState::apply` without
storing waveform caches in the saved document. Prepared instruments may supply
multiple asset summaries; Drum Rack switches the visible waveform with the
selected pad. Optional asset rows show EMPTY and open the same correlated
browser before their first assignment. Newly appended Sample Patches
retain the summary supplied by their topology preparation.

`SoundDefinition` is the canonical reusable sound payload: instrument configuration,
envelope, ordered effects and voice limit, separate from installed Patch identity,
channel, output, trim, sends and global mixer/returns. Capture and installation
reuse native validation and the complete candidate/session lifecycle. A host can
pin immutable definitions without reverse-engineering private saved-Patch JSON.
These definitions still reference installed assets; a portable hashed asset
resolver and package collection are not yet implemented.

Saved state is versioned and stores canonical control state and stable relative
asset references. It never stores decoded PCM, absolute library roots, browser
or preview sessions, document paths, dialog or dirty state, prepared graphs, or
device state. The shell owns `Untitled` or one exact path, an exact typed
`SavedSession` clean baseline, pending continuations, operation status, and
typed lifecycle failures. Its immutable window projection exposes only the
leaf document name, dirty marker, operation/status text, and error marker/text;
full paths remain inside native dialog and filesystem boundaries.

New and Open share one capacity-one candidate worker. Open reads bytes before
the worker performs decode → supported migration → validation → canonical
reconstruction/projection → complete graph preparation. The control side
admits navigation and performance during that work but refuses saved-field
actions once New, Open, or guarded Close has been authorized, preventing a
later edit from being discarded without another decision. Direct Save and
Save As continue to admit edits so the exact captured baseline can complete
while later work remains visibly dirty. The control side then
preflights the private one-shot persisted-content replacement against a cloned
state and the prepared graph's initial parameters. Session replacement then
uses the same application-wide structural coordinator as engine and topology
changes: MIDI admission is gated with bounded all-notes-off recovery, the
complete graph activates at a block boundary, and one correlated
`ReplacePersistedSession` event commits through `AppState::apply`. Failed,
busy, mismatched, or stale candidates are retired on the control side; the
prior capture, state projection, graph revision, render output, and input path
remain usable. No callback destroys a graph.

Save workers receive an immutable typed capture and a content token. The
filesystem adapter encodes version 2 JSON into a unique same-directory sibling,
writes, flushes, syncs, atomically renames, and syncs the parent directory.
Identity and baseline change only after correlated success, so a later edit
remains dirty against the exact written capture. Create/write/flush/sync/rename
and encoding failures stay typed and do not truncate an existing destination.
Dirty New, Open, and Close use one resumable Save/Discard/Cancel state machine;
cancellation is a non-error, while failures remain visible without fallback.

PATCH interaction now derives one trailing empty position after the created
Patch order. `PatchPositionId::Created(PatchId) | TrailingEmpty` is the
canonical interaction identity for focus, remembered roots, subordinate
sessions, return paths, and focus suspended by MIDI Settings. `TrailingEmpty`
is not a `Patch`, has no numeric ID, route, MIDI subscription, parameter entry,
or graph slot, and never enters `SavedSession`. E reaches it through
the same non-wrapping semantic Patch-navigation action used between created
Patches; Q returns to the final created Patch. Shift+Right remains a next-Patch
compatibility shortcut; Shift+Left on Overview now opens MIDI Settings. Navigation and
prospective Overview/Detail/Choice/Utility inspection do not change saved
capture, graph revision, or audio.

One injected immutable `PatchCreationBlueprint` resolves the designated
provider-authored Engine default used by both the initial-session factory and
prospective creation. A first accepted Patch-owned edit reserves the greatest
created `PatchId` plus one, derives `Patch {id}`, appended MIDI channel and
Mixer track, 0 dB trim,
neutral envelope, seeded voice settings, and empty effect positions, then
applies only the triggering edit. The pending candidate stays outside the
created Patch vector. Loading, validation, preparation, failure, activation,
and the final commit all pass through `AppState::apply`; only a matching graph
activation acknowledgement appends the candidate, rekeys any still-live empty
focus/return identity, advances the graph revision, and changes saved content.

Creation reuses the capacity-one structural worker and coordinator.
`GraphReplacementScope::AppendPatch` admits only an identical prior Patch,
return, and device layout plus one exact final candidate. The worker prepares
the complete graph off callback. Before staging, the control side applies the
future commit to a clone, compares exact revision/order/identity/scalar shape
and candidate values with the prepared graph, and refreshes prior scalar
values accepted during preparation. Existing live engines/effects/returns are
carried across the block-boundary swap where identities and layouts agree; the
new engine starts silent. Failure leaves the prior session and graph usable,
and prepared ownership retires off callback.

The current fixed-capacity creation checks are part of the runtime debt
identified in the capacity policy. They do not redefine Figma's effectively
unlimited Patch workflow. Creation must retain transactional preparation,
activation acknowledgement, exact focus identity, and typed failure when
capacity becomes configurable.

## Controller and focus contract

Physical bindings normalize to semantic actions before product logic:

| Input | Meaning |
| --- | --- |
| unmodified arrows/WASD/D-pad | spatial semantic focus movement |
| A / D or Left / Right in Instrument or Post FX Options | previous/next installed group, wrapping at both ends; W / S loops through items within that group |
| Edit + Left/Right | fine decrement/increment or adjacent valid choice |
| Edit + Up/Down | coarse increment/decrement |
| West / Square / `[` | open the focused edit popover or detailed choice picker |
| Edit | toggle/confirm |
| Q / E | previous/next created Patch or trailing empty endpoint, without wrapping |
| Shift + Up / W | Overview opens highlighted Detail; Mixer restores Overview; eligible Detail file control opens Browser |
| Shift + Down / S | Detail restores Overview; Overview restores Mixer; Choice/Browser cancels; existing Settings escape |
| Shift + Left / A | Overview opens Settings on its MIDI Devices page; previous-Patch compatibility only outside Overview where admitted |
| Shift + Right / D | Settings restores the suspended page and focus; next-Patch compatibility elsewhere where admitted |
| Shift + Start / Space | compatibility entry to temporary Settings · MIDI Devices (non-repeating) |
| Select | multi-select only when reducer semantics exist; currently unavailable |
| Start press/release | hold-to-preview in Sample Browser; reserved elsewhere |

Keyboard and controller Shift directions emit one canonical
`NavigatePage(Direction)` action/event. `AppState::apply` dispatches from the
source surface through existing Detail, context-root, Settings, and cancellation
helpers. A single accepted activation advances one generation and one edge:
Detail return stops at Overview, and Mixer return does not also open Detail.
Choice and file cancellation retain their exact origins and preview cleanup.
Overview entry resolves Instrument or effect subject by Patch and slot identity;
duplicate effect capabilities remain distinct, and empty effect slots reject
Detail entry unchanged. Context movement restores each remembered valid root
or exposes the existing deterministic focus repair.
On macOS both Shift keys normalize identically. Every native key carries its
aggregate Shift snapshot, which the window reconciles before routing to either
the host or Synth. Chords remain correct across focus and host handoff even when
the input source omits a separate modifier edge. Text entry clears held input
through the same focus-loss path for both owners.

Q/E reuse canonical `SelectPatch` order and admission. Both Patch cycling and
page navigation leave saved capture, dirty state, routes, subscriptions, and
active graph ownership unchanged; neither publishes a scalar snapshot merely
for navigation. Required browser-preview cancellation remains separate from
performance audio. Settings entry retains its existing discovery effect and
does not connect a device. Footer guidance comes from source-specific admitted
semantic actions, including arrow/WASD equivalents, with no browser-owned page
or return state.

PATCH Main is the non-wrapping Overview order: Engine, then ordered
effect-slot occupancy controls. Sibling order is every created Patch followed
by exactly one trailing empty position. The visual reading order is the same
single vertical sequence at every viewport width; wider compositions expand the row
interiors and the persistent Utility region rather than turning focus movement
into a horizontal card scan. Instrument, envelope, and configured-effect
parameters remain on descriptor-driven Detail surfaces. PATCH Utility contains
exactly master volume, Patch volume, MIDI input, output track, and voice limit.
Main and Utility remain mutually reachable, and subordinate return restores the
stable semantic origin or the nearest enabled sibling after schema change.

Settings contains MIDI Devices, Controller Buttons, and Save & Load system
surfaces, without adding a top-level context. Unmodified Left/Right or A/D
cycles between them; Up/Down or W/S moves within the current page. Save & Load
projects Save, Save As, and Load as stable semantic rows. Activate emits a
path-free reducer effect that the shell routes through the existing session
lifecycle, including native file dialogs and unsaved-change protection.
Filename, dirty state, progress, and failures arrive through a separate
read-only shell document channel, so save completion repaints without a
product-state change. Successful Load returns to the restored session’s Patch
Overview; cancellation or failure retains Settings focus and the prior session.
One `SettingsSession` suspends the exact performance focus, interaction mode,
return path, and Patch subordinate session. Shift+Right restores that identity after schema repair;
Shift+Down remains a compatibility escape. The user selected Shift+Right on
2026-09-06, now authored in Page Layout `153:184` at `155:208` and `157:200`,
with suspended PATCH/MIXER page and exact valid focus or nearest-enabled repair
specified at `155:205`. Surface-first dispatch prevents Settings return from
also selecting the next Patch. The footer advertises Shift+Right / Shift+D as
return to performance.
The projected list is registry-driven and carries display name, opaque identity
facts, Available/Connecting/Connected/Unavailable/Disconnected/Failed text,
and a structural marker in addition to color. The inspector is pinned only to
the reducer-acknowledged active revision. Its activity card accepts only a
matching latest snapshot and otherwise paints Waiting. The list and inspector
start near an 80/20 fraction, clamp to content minima, and stack only under a
container-width constraint; resize emits no semantic action and uses no
aspect-ratio or named-viewport switch. This as-built structure has deterministic
projection/renderer coverage; direct native comparison with Figma node `116:2`
and the physical-device visual handoff remain incomplete evidence.

The interactive composition uses SDL3's Gamepad API on the control thread.
SDL owns discovery, hotplug, hardware mappings, and D-pad normalization; Crest
ships no controller-specific mapping overrides. SDL is built from pinned source
and linked statically, without initializing its video or audio subsystems or
replacing Tauri's event loop. Standard button edges pass through the existing
controller translator into semantic actions with Controller provenance. Trigger
axes use press/release hysteresis for Patch cycling. Normalized signed stick motion
is available through `HostControllerInput::Axis`; a hosted application owns its
dead zones, editing semantics, repeats, and release cleanup. Standalone Synth
continues to leave stick motion inactive.
D-pad navigation and adjustment repeat from elapsed control ticks after 280 ms, then every 45 ms. Page transitions, transport and confirmation never repeat; release, quarantine, workspace changes and disconnect cancel repeat state. Each connected controller has independent transient holds; repeats cannot
duplicate an edge. Connection and SDL remapping suppress existing holds until
release. Remapping cancels capture; initialization/open failures are explicit
controller status and release owned input.
Keyboard and gamepad Edit holds share mode ownership, so releasing one input
preserves another held input and never dismisses a modal surface. Edit confirms
on tap release and becomes a modifier when combined with a direction. Page
chords retain their original interpretation until direction release, including
when the modifier releases first. Preview release stops only the request
accepted from that input. Disconnect clears that device's holds; keyboard focus
loss releases both keyboard Edit and preview. Autonomous demos do not attach devices.
Hosted event scenes can call `without_system_input_devices` on the production
application to retain its window, graph workers and audio device while excluding
physical MIDI/gamepad ingress. Scene events remain owned by the host; Synth has
no sequencing or Tracker scene dependency.

Controller Buttons configures directions, Edit, Shift, Sample preview, Patch
cycling, Settings entry, and edit popovers. One normalized layout applies to all connected
gamepads. Defaults use D-pad navigation and triggers for Patch cycling.
Shared face-button defaults are South/Cross for Edit, East/Circle for Shift,
North/Triangle for Start (sample preview or host transport), and West/Square for edit popovers. The physical
Start/Options button opens Settings in standalone Synth. Tracker reserves physical
Start/Options and the Preview role for global song transport on every page, including
with Shift held; release does not toggle again. Hosted Shift+Space also stays transport.
Tracker Global Settings is reached through Main Menu. Figma defines the semantic roles rather
than platform face shapes. The former generated L1-Shift/Triangle-Settings
defaults are repaired during preference loading only when the complete mapping
matches, retaining an exact backup; custom mappings retain their existing roles. Version 1 button files gain the popover role on West or another unused button; version 2 stores all eleven roles. A host
workspace transition cancels active edits and previews but preserves held Shift
in the shared semantic input path. Disconnect, remapping and capture still
quarantine every hold.
Select's reserved product meaning remains unavailable. Only backend-mapped standard
buttons participate; analog stick movement has no standalone semantic binding; hosted applications
may consume the normalized axis boundary. Unmapped vendor buttons remain unavailable. Legacy C/Z assignments remain readable and explicitly ask for
reassignment, since SDL has no corresponding standard buttons. Gamepads remain
active while the synth runs in the background.

Activate a setting to capture the next fresh button press. Assigning an occupied
button swaps its previous role, preserving a unique binding for every action.
Capture and mapping transitions suppress held input until release. Return or
Shift+Right cancels capture first; a subsequent return restores performance.
Keyboard access stays available during remapping. Restore defaults is a focusable
Settings action, including with no gamepad attached. Device loss cancels capture.
Mappings, capture, and device status mutate only through `AppState::apply` and
never change saved synth content or publish scalar audio parameters.

`controller-buttons.json` lives in the per-user configuration directory beside
the MIDI preference. A separate worker loads validated versioned mappings and
atomically replaces the file after edits; shutdown flushes the final accepted
mapping after audio stops. Save acknowledgements carry the complete mapping,
so stale results cannot mark a newer assignment saved. Read/decode failures
leave controller input unavailable until explicit recovery; write failures keep
current controls active and show unsaved status. Backend failures remain visible.
The 8BitDo Pro 3 in D mode has user-confirmed operation on macOS with SDL3.
Broader controller/platform coverage and native visual acceptance remain separate
from the deterministic production-path tests.

On macOS, Shift arrives through AppKit's `FlagsChanged` event rather than a key
down/up pair. The native input adapter treats modifier transitions as
non-repeatable and never queries key-repeat state from a modifier event; a UI
input callback must not unwind through the Objective-C event boundary. The
local monitor retains a bounded 128-signature window so WebKit's delayed
unhandled-key redispatch cannot double-feed the translator after a full input
burst. Native arrow keycodes normalize alongside WASD at this same boundary.
Keyboard page holds remain consumed until direction release or focus loss,
including when Shift is released first; Q/E also require release before another
Patch step. Bare focus/edit repeats retain their existing behavior. Controller
page gestures require release or disconnect before another activation.

Linux captures physical XKB keys with a GTK key controller on the owned
window, before WebKit dispatch. The shell decides whether capture consumes each
key, so send-name text entry reaches WebKit without synth actions. Ctrl+4 opens
Sends; other Ctrl/Alt/Super shortcuts remain native. Held-key tracking clears on
focus loss. The translator and reducer are shared
with macOS. Linux native witnesses use XTest on an isolated X11 desktop;
Wayland hardware input remains a separate acceptance check.

`make test-webview-page-navigation-native` exercises actual AppKit or Linux
XTest Q/E, Shift+arrows, repeats, and WASD through the production reducer and
native paint.
Its accepted journey includes Settings entry/return, exact Instrument/effect
Detail origins, remembered context roots, singular focus, footer guidance,
and owned shutdown. It does not establish an attached-gamepad handoff or broad visual parity.

MIXER Main uses one stable `(MixerTrackId, MixerTrackParameter)` path. Left/Right
changes the selected track while preserving Level/Pan/Mute/Solo row; Up/Down changes row
while preserving track. Inspector correlation is pinned to the selected track
and control, and its send → return → global body may scroll without using
scroll position as selection authority. The webview renders every visible
Inspector control through the shared parameter row, including sends; it must
not suppress a control kind that remains visible and focusable in the semantic
projection. Inspector entry therefore paints and reveals the reducer-selected
first send. The native webview witness checks the complete visible control
order, send labels, and exactly one painted target for the semantic focus path.
Compatible meter data must match the painted graph revision and arrive with an
advancing observation sequence. UI focus and queued MIDI can change parameter
generation independently of sounding audio, so generation equality does not
gate meters. Missing, mismatched, or aged data paints an explicit zero/stale
state and cannot mutate application state.

Exactly one focused control is shown. Focus, adjust, active/current,
disabled, loading, error, mute, solo, and selection states use text or shape in
addition to color. The footer shows the current semantic path and only valid
actions. It is the only visual action-legend owner; rows and panels do not
repeat its guidance.

## As-built visual vocabulary

[`src/shell/tokens.rs`](src/shell/tokens.rs) owns visual tokens;
`webview-page/tokens.css` is generated. Do not duplicate token values here.
The current interface is dark-only and uses Azeret Mono. Vendor provenance
retains font and upstream DSP licensing information.

One Grid/Flex composition uses content-aware Wide, Standard, and Compact
layouts. Main and Utility/Inspector tracks retain their semantic identities;
Compact stacks them. Intrinsic sizing, wrapping, independent scrolling,
scroll-to-focus, and accessible target sizes keep every projected control
reachable. Named viewport sizes are test fixtures, not fixed product canvases.

## Evidence retained in code

Production tests, witness implementations, and generated reports are the
reproducible evidence. Historical aggregate test counts and handoff narratives
live in Git history, not this document. Validate the changed path and report
what was actually run, including failures and environment skips.

Evidence must traverse the production reducer, projector, worker, graph, and
render path. Construction-only tests, parallel fake behavior, success tokens,
and silent audio do not prove the product works. Native rendering, physical
input/audio, and offline timing establish different facts; do not substitute
one for another or demand an unrelated repeat of already accepted work.

## Commands

`make run`, `make play`, and `make ui` use the optimized release profile for both
Rust and native DSP. `cargo run --bin crest-synth` remains an explicit debug
launch; unoptimized timing is not a supported real-time performance target.
Tour tests with real-time dispatch budgets run with `cargo test --release --lib
full_demo_`; debug test runs skip those timing checks.
The test profile optimizes execution while retaining debug assertions and native fault
injection seams. `CREST_AUDIO_BUFFER_FRAMES` explicitly selects a positive device
buffer size; unsupported requests fail instead of silently changing latency.
Without it, the existing preferred device configuration applies. Larger buffers
can accommodate VM scheduling at the cost of latency; graph preparation sizes
its storage from the selected buffer before starting audio.
The pinned CPAL source retries interrupted ALSA waits through its existing
worker loop. Signals do not become device failures; other backend errors keep
their typed reporting. The Linux device witness injects signals into the real
audio worker and verifies continued callbacks without runtime failures.

`make full-instrument-effect-demo` runs a sequential listening tour in the
production window and audio runtime, focused on the new audio catalog. The
composition root skips SoundFont, standalone Braids auditions, the initially
empty Drum Rack, and the legacy Chorus, Reverb, and Delay. Drum Rack has a
separate pad-loading, note-routing, and rendering witness. The changed Sample renderer remains included. The
installed registries supply the remaining entries: each instrument is
auditioned dry, then each effect occupies one post-effect position on the
composition-root-designated Braids
default. Every audition has one Patch, a prepared voice budget of one, a
neutral host envelope, no bus returns, and the same gated major-seventh
arpeggio across three octaves. Keyboard edits, file commands, automatic test
MIDI, and physical MIDI are isolated; closing the window cancels the tour.

Each entry gets eight bars total of 4/4 at 120 BPM, using eighth notes with
200 ms gates so the bundled Elements resampler can sound before note-off.
The first bar plays defaults; the remaining seven sample editable parameters
across the descriptor order. Smaller parameter sets revisit controls with
different values. Continuous values sample quantized quarter, midpoint, and
three-quarter positions; discrete controls and structural presets sample
representative choices. Default values are omitted from the variations where
alternatives exist; entries without editable alternatives stay at defaults.
This is a quick listening tour, not exhaustive parameter or preset coverage.
Other parameters reset to defaults between bars, except descriptor
prerequisites required to enable the selected control. File parameters use
bundled assets, and read-only fields and asset scope are reported explicitly;
the demo does not enumerate imported file libraries. In particular, the
bundled NAM test model and transparent convolution IR do not demonstrate a
library of amp captures or reverbs.

`make demo-live-drum-rack` runs an isolated Drum Rack listening scene in the
production window and audio runtime. It renders the bundled HiDef standard
percussion kit into a process-owned temporary WAV library before startup,
loads all sixteen pads through the Sample preparer, auditions each pad, then
plays an eight-bar layered groove at 120 BPM. The Sample editor follows the
pad selector through ordinary semantic actions. D#2 retains the requested
name `Right` and uses a ride cymbal. The scene shares the listening-tour
scheduler, graph activation, input isolation, and cleanup; it does not alter
the user's Sample library or the `demo-live` alias.

Scene navigation uses the production semantic resolver and reducer. Complete
session candidates use the existing background session worker and structural
coordinator, with block-boundary activation and off-callback retirement.
Scalar variations use ordinary semantic adjustments and scalar snapshots on
the existing graph. Previous values are restored before enabling the next
control's prerequisites, preserving the engine and effect history throughout
each entry. Note delivery runs before a small budget of control work each tick;
parameter changes progress during playback and must settle within their bar.
Navigation and scalar edits do not pause the musical clock or
replace the graph. Preparation pauses occur only between entries or for
structural changes; late window ticks stretch playback without replaying
missed notes. Terminal
and document status show the current audition, actual value, and bar; startup
prints the registry-derived duration. Preparation timeouts and audio failure
observations fail explicitly. This is a listening tool, with complete-note
headless coverage through the real worker, reducer, projector, and native
renderer; it does not claim a completed physical listening review.
The tour exposed an unsafe Elements modulation-offset endpoint in wrapper
metadata. Its admitted range is now 0–0.5, keeping the offset plus the internal
0–0.5 LFO within the upstream approximate cosine oscillator's supported domain.
No synthesis algorithm was changed.

`make performance-tools` installs the pinned Samply profiler locally.
`make test-performance` builds optimized code with line tables, then runs the
existing headless scenes, navigation, asset and runtime tests alongside the
production stress matrix. The matrix covers installed engines and Braids
models, Patch/voice/block-size fixtures, effects, returns, MIDI churn, live
edits, serialization and concurrent graph preparation/activation/retirement.
Fixture sizes are not product limits. Each case has independent unprofiled
timing trials and a separate Samply diagnostic execution. The existing audio,
note-delivery, timing and Rust callback allocation/destruction gates remain
authoritative; profiled matrix timings do not count as timing-budget proof.
The largest native stress cases still exceed callback budgets on current
development hardware, particularly fixed-rate resampling and resonant models.
These timing gaps remain unresolved; ordinary suite passes do not establish a
complete stress-matrix pass. Rapid retrigger fixtures prepare storage for held
notes and overlapping release tails through the production session restore
path. Voice admission still refuses exhaustion without truncating sounding voices.

Every run retains commands, build/host/source fingerprints, raw latency samples,
per-child CPU/RSS/fault/context-switch measurements, scene observations, logs,
Samply profiles and portable symbol sidecars under `target/performance/runs/`.
The HTML/Markdown report links those artifacts; `make performance-report` reads
the latest run. Sampled hot-function counts are not CPU percentages; use
`python3 scripts/performance_suite.py open <profile.json.gz>` for interactive
thread timelines, call trees and flame graphs. Rust allocation counters do not
intercept native C/C++ allocation. Process timings include fixture startup and
report serialization; operation timings surround the production calls.

`python3 scripts/performance_suite.py list` lists the manifest's existing source
targets. `PERFORMANCE_ARGS='--case stress.control'` selects a focused run;
`--baseline <previous-run.json>` rejects incompatible build/host/workload
metadata and fails material median regressions across repeated trials.
`--timings-only` explicitly records incomplete profiling coverage. Failed
workloads, zero-test filters, watchdog expiry and missing symbolized profiles
cannot produce a complete suite pass. `make test-performance-runner` exercises
these failure paths. Exact workloads and screening thresholds live in source.

`make test-performance-native` profiles the existing WKWebView paint and meter
witnesses. `make test-performance-physical` profiles the retained live scenes
with a real window and audio device. Both require an unlocked interactive
session; a locked display is reported as blocked. Offline timings do not prove
physical device scheduling or native paint latency.

Common entry points are defined in [`Makefile`](Makefile):

| Purpose | Command |
| --- | --- |
| Run the product | `make run` |
| Build, functional checks, lint, formatting | `make build`, `make test`, `make lint`, `make fmt-check` |
| SoundFont import and restore | `make test-soundfont-loading` |
| Physical MIDI discovery | `make test-midi-host` |
| Controller input and mapping | `make test-controller` |
| Native Controller Settings | `make test-controller-native` |
| SDL3 virtual gamepad input, hotplug, and mapping lifetimes | `make test-controller-sdl` |
| Native Detail/Mixer | `make test-webview-detail-native` |
| Native page navigation | `make test-webview-page-navigation-native` |
| Native SoundFont workflows | `make test-webview-soundfont-native` |
| Attached MIDI handoff | `make midi-device-handoff` |

Normal startup plays the bounded test pattern on MIDI channel 1; T toggles it.
Return opens highlighted Detail. Return on Sample/SoundFont File opens the
in-app browser. W/S navigates; Return enters or assigns; Shift+S cancels.
Hold Space on a Sample file to preview; SoundFont preview is unavailable.
K with W/S/A/D edits controls; K+W opens an eligible choice. Cmd+S/Cmd+O on
macOS and Ctrl+S/Ctrl+O on Linux use native session Save/Open dialogs.
Settings · Save & Load exposes the same workflow with controller navigation and
Return to activate. Assets use the in-app file page.

To choose an existing Sample library and initial asset, configure both
`CREST_SAMPLE_LIBRARY_ROOT` (absolute root) and `CREST_SAMPLE_DEFAULT_ASSET`
(relative asset) before startup. They are a validated pair.

Native window, physical-input, and physical-audio witnesses require an
interactive host. An unavailable environment is incomplete evidence, not a
pass. Building the bundled SDL3 library requires CMake and a C toolchain. Linux
builds require GTK3, WebKitGTK 4.1, ALSA and SDL platform development libraries.
Session file dialogs use the desktop portal (or Zenity); unsaved-change prompts
require Zenity. Optional JACK is an explicit MIDI packaging choice. Windows
MIDI defaults to WinMM; optional WinRT is also an explicit choice. Native keyboard
capture supports macOS and Linux; Windows is not a runnable release target yet.
Broader packaging remains unverified; maintain platform adapter boundaries
without assuming a small-device target.
`scripts/build_release_macos.sh` builds native Apple Silicon or Intel macOS app
and DMG bundles with the SoundFont, MIDI fixture, font/audio notices, and Eigen
source. The manually dispatched `Release binaries` workflow builds both targets
from one commit and checks the packaged production smoke path outside the checkout.
Bundles target macOS 11 or newer; builds and smoke checks do not establish physical
device acceptance on every OS version. Signing is ad-hoc; Developer ID signing and
notarization remain unavailable until credentials are configured. The workflow's
optional publish input tags the exact build commit and publishes verified artifacts
and SHA-256 checksums only after both native jobs pass.

`scripts/linux/Dockerfile` provides Ubuntu 24.04 and the pinned Rust toolchain.
`make test-linux` runs the ordinary suite, doc tests, Clippy, Python checks,
native DSP allocation/lock instrumentation, and native UI witnesses. Its
virtual desktop uses Xvfb/Openbox and a PulseAudio null sink; this validates
Linux integration without claiming physical speaker or Steam Deck acceptance.
`make test-linux-wayland` runs the Detail, Mixer, and Sample native paint and
resize witnesses through Weston. The compositor is nested in Xvfb to supply a
Wayland input seat without a GPU; application GTK windows use Wayland exclusively.
`make test-linux-session-native` builds the shipping binary and exercises audio,
transport stop, native Save As/New/Open, exact session round trips, dirty edits,
unsaved cancellation, Save, and owned shutdown on both window systems. AT-SPI
observes native focus before the next shortcut, including compositor animations;
session files, configuration, logs, and audio captures live in a fresh evidence
directory under the selected Cargo target directory's `linux-evidence` folder.
WebKit uses software compositing on this GPU-less desktop to avoid software-GPU
round trips; normal application launches retain the platform rendering defaults.
The virtual desktop defaults to an 8192-frame audio buffer to tolerate emulation
and container scheduling; it does not measure device latency.

On macOS, Docker Desktop supplies a Linux VM. `scripts/linux/dev.sh` builds the
Ubuntu development image on first use and runs commands as the checkout owner.
On Apple Silicon it defaults to ARM64; `--arch amd64` selects emulated x86-64
for architecture compatibility checks. Edit source with the Mac editor as
usual: the launcher bind-mounts this exact worktree into `/workspace`.

```sh
make linux-shell
make linux-desktop
# Or run commands directly from macOS:
scripts/linux/dev.sh cargo check --locked --all-targets
scripts/linux/dev.sh make test-linux
scripts/linux/dev.sh make test-linux-wayland
scripts/linux/dev.sh make test-linux-session-native
scripts/linux/dev.sh --arch amd64 cargo test --locked --lib
```

`make linux-desktop` builds the optional Xpra image and prints a password-bearing
localhost URL. Open it in a browser for a Linux desktop, development terminal,
and the shipping synth; the terminal shows the release build before launch and
returns to a shell when the app closes. Browser keyboard and mouse input reach
the real Linux app; Control keeps its Linux meaning instead of swapping with
Mac Command. Click the desktop to focus it. Xpra forwards the PulseAudio monitor
to browser audio; toggle Audio off/on if the browser blocks autoplay. This adds
streaming latency and does not measure physical device latency.
The desktop defaults to 1024×768;
set `CREST_LINUX_SCREEN_SIZE=1600x1000` for a larger display and use browser zoom
or fullscreen to fit it. Automated witnesses retain their existing resolution.

Each desktop gets its own random port bound to `127.0.0.1` and a fresh password.
Keep the launcher running; closing the browser only disconnects, while Ctrl+C
stops its container. The launcher also prints a `docker stop` command. Save
sessions under `/workspace` to retain them in this worktree; manual app settings
persist under the selected target directory's `linux-desktop/config` folder.

Containers are disposable; source and per-worktree, per-architecture build
caches persist on the Mac under `target/linux-arm64` and `target/linux-amd64`.
Cargo downloads persist in named Docker volumes shared by the same user.
`CREST_LINUX_TARGET_DIR` can select an existing cache; never share one between
architectures or concurrent worktrees. Caches from other container mount layouts
may need cleaning because CMake stores absolute paths. Run
`scripts/linux/dev.sh make cache-status`
to inspect the selected cache, or `scripts/linux/dev.sh cargo clean` to remove
only that cache's build output. Image identity includes its Dockerfile, host
user ID, and architecture; `make linux-image` rebuilds it explicitly.
The launcher defaults to four CPUs, 6 GiB RAM, and two build/test workers;
`--help` lists overrides. Docker Desktop must have enough VM resources and disk
space for the images; its VM limits bound the container allowances.

WebKit's sandbox remains enabled. Docker's default seccomp profile blocks the
user namespace creation it needs; the development container permits those
syscalls. Native Linux hosts must likewise permit unprivileged user namespaces.
The automated desktops are virtual and use software rendering and monitored
audio. Emulated x86-64 timings are functional evidence, not Steam Deck
performance measurements. Physical controls, audio latency, and GPU behavior
still require the target hardware.

## Change checklist

Every product or architecture change must preserve or deliberately amend the
following in one coherent commit:

1. physical input normalizes to semantic intent and mutates only through
   `AppState::apply`;
2. UI and audio are projections of canonical state;
3. the callback remains bounded, preallocated, lock-free, nonblocking,
   allocation/destruction/I/O/log/panic-free;
4. discrete events, latest scalars, and prepared structural graphs retain
   separate transports;
5. capability ports remain generic and failures never silently fall back;
6. PATCH and MIXER remain the only top-level contexts;
7. focus and return paths remain stable semantic identities across reflow;
8. control/status state remains explicit in text or shape as well as color;
9. proof uses the production reducer, projector, worker, graph, and render
   seams and includes a measurable negative/no-op distinction;
10. actual rendered output is compared directly with the live Figma reference
    before claiming interface fidelity.

Do not treat OpenSpec change artifacts or any other planning documentation as a
competing product authority. This file should change only when the as-built
architecture, durable invariant, verified capability, known gap, or reference
contract actually changes.
