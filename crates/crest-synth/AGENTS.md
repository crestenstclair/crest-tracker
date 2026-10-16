# Working in Crest Synth

Read the relevant sections of `DESIGN.md` before changing product behavior or architecture. The linked
Figma file is the normative product, visual, and interaction source of truth.
`DESIGN.md` is the master as-built architecture, invariant, and implementation
status reference; it records how the current production system realizes the
Figma contract without competing with it.

The current UI is a functional blockout, not a faithful Figma implementation.
Do not infer visual or workflow acceptance from the presence of surfaces,
components, or passing reducer/projection tests.

Preserve these invariants:

- physical input → semantic action/event → `AppState::apply` → view/audio
  projections;
- `AppState::apply` is the only product-state mutation path;
- a hard real-time callback with bounded, preallocated work and no allocation,
  deallocation, locking, blocking, I/O, logging, formatting, panic, unwinding,
  or destruction;
- separate RT transports for ordered discrete events, latest compatible scalar
  snapshots, prepared structural graph changes, and decimated observations;
- complete graph preparation off callback, block-boundary activation, and
  off-thread retirement/destruction;
- SoundFont, Sample, Braids, effects, assets, and devices behind capability
  ports, with typed failure and no silent fallback or substitution;
- a schema-driven controller UI with PATCH and MIXER as the only top-level
  contexts;
- exactly one stable semantic focus, with stable return identity across
  navigation, reprojection, and density changes;
- one canonical public type per concept and thin UI, MIDI, device, controller,
  asset, persistence, and serialization adapters;
- explicit status and control states in text or shape as well as color;
- measured, falsifiable proof through the production reducer, projector,
  worker, graph, and render path.

Figma example engines, effects, patches, files, values, and counts are design
fixtures, not an exhaustive feature list. Installed registries determine
production content.

Target modern desktop hardware. Do not impose arbitrary product caps on
instruments, effects, Patches, tracks, parameters, or voices. Resource budgets
must be configurable for the hardware; preallocation and bounded callback
work do not require fixed product counts. Existing hard-coded capacities are
implementation debt, never requirements to preserve or reintroduce.

Keep documentation small: Figma owns product intent; `DESIGN.md` owns current
architecture, durable decisions, and known gaps; source and tests own exact
implementation details and evidence. Update existing sections instead of
appending historical reports or copying constants, tokens, and test counts.
Do not reload retired plans, archived specs, or Git history as routine context.
Consult history only for a specific historical question.

Use OpenSpec only when explicitly requested. Keep its artifacts scoped and
temporary; remove completed artifacts after incorporating durable decisions
into `DESIGN.md`. Do not maintain a parallel product specification, phase
roadmap, planning kit, or another master design document. Requested research
reports are decision inputs, not product authorities or automatic context.
