//! The projection transport: generation-gated push of the accepted
//! projection's embedded semantic model to the page.
//!
//! On each tick where the fetched [`GraphicalShellProjection`] carries a new
//! accepted generation, the channel serializes the projection's embedded
//! [`SemanticGraphicalViewModel`] with `serde_json` and emits the document on
//! the [`PROJECTION_EVENT`] named event through the tauri `Emitter`. The
//! payload is exactly the projector's serialization — one schema, no
//! page-facing struct, no trimmed or selected field. If the page ever needs a
//! field this document lacks, the fix is in the projector and `DESIGN.md`, never
//! a wrapper here.
//!
//! # Generation gating
//!
//! Emission is gated on the projection's accepted `generation`, not on deep
//! comparison: the projector already stamps every accepted document with its
//! generation and state hash, so an unchanged generation means an unchanged
//! document. A tick that fetches the same generation emits nothing — the
//! ~85 KB serialization happens only when a reducer edit actually produced a
//! new accepted document. The gate advances only on a successful emit; a
//! failed emit leaves the gate unmoved so the document is not silently lost
//! while the window still lives.
//!
//! # Emit failures are typed, surfaced, and tolerated at teardown
//!
//! An emit failure is a typed [`ProjectionChannelError`] the window converts
//! onto its declared `WindowError` path — the same record-first-error-and-
//! close-once treatment the retired native adapter gave an invalid late frame. It is
//! never a panic; during teardown the window stops pushing once the webview
//! is destroyed, so a shutdown race cannot turn a clean close into a fatal
//! error (see `window.rs`).
//!
//! # Painted acks come back and become observations (WP02 T005)
//!
//! After the page paints a pushed document it echoes the document's semantic
//! identity plus its measured post-paint geometry on the [`PAINTED_EVENT`]
//! event. [`ProjectionChannel::forward_ack`] receives that ack, correlates
//! it against the bounded in-flight tracker this channel fills on each
//! successful push, and constructs exactly one [`ShellFrameObservation`] —
//! identity copied verbatim (the ack must match the pushed document field
//! for field), viewport and region rectangles taken from the ack's measured
//! geometry. An ack that matches no in-flight pushed document never becomes
//! an observation: expected region names or a pre-render layout plan alone
//! cannot construct a passing observation. Which of the two ways that
//! can happen decides the outcome: an ack claiming a generation newer than
//! anything ever pushed is fabricated evidence and stays a typed
//! [`PaintedAckError`], while a late ack for an already-superseded document
//! (dropped from the bounded tracker under sustained generation churn —
//! observed on the first webview-hosted live runs, mission
//! webview-shell-cutover WP03) is a lost frame consumed as
//! [`ForwardedAck::SupersededLate`], degrading observation only.
//!
//! A superseded document whose ack never arrives is dropped from tracking
//! when a successor acks (or when the bounded tracker is full) — lost frames
//! degrade observation only, exactly like the retired native adapter dropping an
//! unpainted frame.
//!
//! # The verbatim-copy rule holds in every window (FR-003)
//!
//! Being consumed as a lost frame is not being consumed unchecked. A dropped
//! document leaves its serialized identity behind in a second bounded window
//! ([`RETAINED_RETIRED_IDENTITIES`]), fed by both ways a document retires —
//! capacity eviction on push, and the drain of the acked document and every
//! older one on a successful ack. A late ack naming a generation still inside
//! that window is held to the same field-for-field comparison an in-flight ack
//! is, through the same single implementation, and a rewritten identity is the
//! same typed [`PaintedAckError::IdentityMismatch`] in either window. Only
//! once an ack outlives the retained window does the channel stop making a
//! claim: it holds nothing to compare against, so the ack stays a lost frame
//! rather than becoming a false rejection. What never changes is that a
//! superseded-late ack constructs no [`ShellFrameObservation`] at all,
//! validated or not.
//!
//! # Nothing here touches the real-time callback
//!
//! The channel runs on the window's event thread and reads only the immutable
//! control-side projection that the `ProjectionCallback` already hands every
//! `AppWindow` — the same value the retired native adapter painted. It installs
//! nothing into the audio callback, allocates nothing inside it, and shares
//! no lock the callback could contend on; the render path's measured bounds
//! are unchanged from the pre-cutover shell baseline.

use crate::control::{GraphicalShellProjection, SemanticGraphicalViewModel};
use crate::shell::app_window::WindowError;
use crate::shell::{
    ShellFrameObservation, ShellFrameObservationError, ShellRegionId, ShellRegionObservation,
    ShellRegionRect, StripPaintObservation,
};
use core::fmt;
use serde_json::Value;
use std::collections::VecDeque;

/// The named tauri event carrying each newly accepted projection's
/// serialized [`SemanticGraphicalViewModel`] to the page.
pub const PROJECTION_EVENT: &str = "crest://projection";

/// The named tauri event the page emits after both Rust→page transport
/// listeners have been registered. The window does not publish projection
/// or meter documents before this handshake, so the first accepted state
/// cannot be lost during webview startup.
pub const READY_EVENT: &str = "crest://ready";

/// The named tauri event on which the page acknowledges each painted
/// document: the document's semantic identity copied verbatim plus the
/// measured viewport and region geometry (the page's `paintedEvidence`).
pub const PAINTED_EVENT: &str = "crest://painted";

/// The named tauri event on which the page reports a thrown render failure
/// for a received document. The window converts it to the typed
/// `WebviewShellError::PageRenderFailed` and takes the fatal runtime path —
/// a page that loads then fails to render never leaves a frozen window
/// (WP02 T008).
pub const RENDER_ERROR_EVENT: &str = "crest://render-error";

/// How many pushed documents may await their painted ack at once.
///
/// The live cadence keeps this at one or two (a push happens only on a new
/// accepted generation and the page acks within a frame); the bound exists
/// so a page that stops acking can never grow an unbounded queue. When the
/// tracker is full the oldest unacked document is dropped — a lost frame
/// degrades observation only.
pub const MAX_IN_FLIGHT_DOCUMENTS: usize = 8;

/// How many retired documents' identities stay comparable after the document
/// itself is gone.
///
/// A document leaves [`ProjectionChannel::in_flight`] two ways — evicted by
/// the capacity bound, or drained when an ack consumes it and everything
/// older — and both throw the document away. Its *identity* is kept here so a
/// late ack naming an already-retired generation can still be held to the
/// verbatim-copy rule (FR-003). Defined as the in-flight bound because the
/// same generation churn drives both windows: retiring more than
/// [`MAX_IN_FLIGHT_DOCUMENTS`] identities means the acks for the oldest are
/// already hopeless. Bounded for the same reason `in_flight` is — retaining
/// every identity would grow without limit across a long session.
const RETAINED_RETIRED_IDENTITIES: usize = MAX_IN_FLIGHT_DOCUMENTS;

/// The serialized identity fields a painted ack must copy verbatim from its
/// document, in the order they are tracked. This copies semantic identity
/// exactly.
const ACK_IDENTITY_FIELDS: [&str; 6] = [
    "generation",
    "stateHash",
    "context",
    "activeSurface",
    "focusPath",
    "interactionMode",
];

/// What one [`ProjectionChannel::push`] did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectionPush {
    /// A new accepted generation was serialized and emitted.
    Emitted,
    /// The projection's generation matched the last emitted one; nothing was
    /// serialized and nothing was emitted.
    Unchanged,
}

/// What one received painted ack became.
#[derive(Clone, Debug, PartialEq)]
// The observation is carried inline: this value lives for exactly one match
// arm on the window's event thread, so a second heap ownership step would
// buy nothing.
#[allow(clippy::large_enum_variant)]
pub enum ForwardedAck {
    /// The ack correlated with an in-flight pushed document and copied its
    /// identity verbatim: exactly one honest post-paint observation.
    Observation(ShellFrameObservation),
    /// A late ack for a document this channel pushed but had already
    /// dropped from its bounded tracker (superseded under sustained
    /// generation churn before the ack arrived). The paint happened, but
    /// the document it proves is gone — a lost frame, consumed without an
    /// observation and without an error (display evidence degrades only).
    SupersededLate {
        /// The already-superseded generation the late ack named.
        generation: u64,
    },
}

/// A typed projection transport failure, surfaced through the window's
/// declared `WindowError` path.
#[derive(Debug)]
pub enum ProjectionChannelError {
    /// The embedded semantic model failed to serialize. The projector's
    /// derives make this structurally unreachable; if it ever fires it is a
    /// projector defect, not a page concern.
    Serialization {
        /// The accepted generation that failed to serialize.
        generation: u64,
        /// The underlying serde failure.
        source: serde_json::Error,
    },
    /// The tauri emit itself failed (typically the window going away).
    Emit {
        /// The accepted generation whose document failed to emit.
        generation: u64,
        /// The underlying tauri failure.
        source: tauri::Error,
    },
}

impl fmt::Display for ProjectionChannelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Serialization { generation, source } => write!(
                formatter,
                "projection generation {generation} failed to serialize: {source}"
            ),
            Self::Emit { generation, source } => write!(
                formatter,
                "projection generation {generation} failed to emit: {source}"
            ),
        }
    }
}

impl std::error::Error for ProjectionChannelError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Serialization { source, .. } => Some(source),
            Self::Emit { source, .. } => Some(source),
        }
    }
}

/// Joins the typed transport failure onto the window's declared error path,
/// preserving the cause text.
impl From<ProjectionChannelError> for WindowError {
    fn from(error: ProjectionChannelError) -> Self {
        WindowError::new(error.to_string())
    }
}

/// A typed painted-ack rejection: the ack could not become a
/// [`ShellFrameObservation`]. Never a panic; the window gives it the same
/// record-first-error-and-close-once treatment the retired native adapter gave an
/// invalid late frame.
#[derive(Debug)]
pub enum PaintedAckError {
    /// The ack payload could not be understood as post-paint evidence.
    Malformed {
        /// What was wrong with the payload.
        detail: String,
    },
    /// The ack names a generation newer than anything this channel ever
    /// pushed — the observation the ack describes cannot have been painted
    /// from a pushed document, so it must not be constructible. (A late ack
    /// for a pushed-then-dropped document is not this error; it is consumed
    /// as [`ForwardedAck::SupersededLate`].)
    UnknownDocument {
        /// The generation the ack claimed.
        generation: u64,
    },
    /// The ack correlated with a pushed document but one of its identity
    /// fields is not a verbatim copy of that document's.
    IdentityMismatch {
        /// The correlated generation.
        generation: u64,
        /// The serialized identity field that differed.
        field: &'static str,
    },
    /// The ack correlated and its identity matched, but its measured
    /// geometry or labels fail the observation's own post-paint invariants.
    InvalidObservation {
        /// The correlated generation.
        generation: u64,
        /// The observation invariant that rejected the evidence.
        source: ShellFrameObservationError,
    },
}

impl fmt::Display for PaintedAckError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed { detail } => write!(formatter, "painted ack rejected: {detail}"),
            Self::UnknownDocument { generation } => write!(
                formatter,
                "painted ack for generation {generation} matches no in-flight pushed document"
            ),
            Self::IdentityMismatch { generation, field } => write!(
                formatter,
                "painted ack for generation {generation} does not copy {field} verbatim from its document"
            ),
            Self::InvalidObservation { generation, source } => write!(
                formatter,
                "painted ack for generation {generation} carries invalid painted evidence: {source}"
            ),
        }
    }
}

impl std::error::Error for PaintedAckError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidObservation { source, .. } => Some(source),
            Self::Malformed { .. }
            | Self::UnknownDocument { .. }
            | Self::IdentityMismatch { .. } => None,
        }
    }
}

/// Joins the typed ack rejection onto the window's declared error path,
/// preserving the cause text.
impl From<PaintedAckError> for WindowError {
    fn from(error: PaintedAckError) -> Self {
        WindowError::new(error.to_string())
    }
}

/// One pushed document awaiting its painted ack: the correlation key, the
/// serialized identity the ack must copy verbatim, and the semantic model
/// the resulting observation is constructed against.
#[derive(Debug)]
struct InFlightDocument {
    generation: u64,
    identity: [Value; ACK_IDENTITY_FIELDS.len()],
    semantic: SemanticGraphicalViewModel,
}

/// One retired document's correlation key and serialized identity, kept after
/// the document itself was dropped. The semantic model is deliberately not
/// kept: a retired generation can never construct a [`ShellFrameObservation`]
/// again, so the identity is exactly enough to hold its late ack to the
/// verbatim-copy rule and nothing more.
#[derive(Debug)]
struct RetiredIdentity {
    generation: u64,
    identity: [Value; ACK_IDENTITY_FIELDS.len()],
}

/// Generation-gated push channel from accepted projections to the page,
/// plus the receive side of the page's painted acks.
///
/// Push state is exactly one value — the last successfully emitted
/// generation; a push either emits the current document or does nothing.
/// The ack side tracks at most [`MAX_IN_FLIGHT_DOCUMENTS`] pushed documents
/// so each ack can be correlated with the document it claims to have
/// painted.
#[derive(Debug, Default)]
pub struct ProjectionChannel {
    last_emitted_generation: Option<u64>,
    in_flight: VecDeque<InFlightDocument>,
    /// The serialized identities of the most recently retired documents, in
    /// retirement order (oldest at the front).
    ///
    /// Every document that leaves `in_flight` — evicted by the capacity bound
    /// in `push`, or drained by the ack that consumed it and everything older
    /// in `forward_ack` — retires its identity here. That is what lets a late
    /// ack for an already-dropped document still be held to the verbatim-copy
    /// rule instead of being consumed unchecked (FR-003 / RISK-4).
    ///
    /// Bounded at [`RETAINED_RETIRED_IDENTITIES`] with the same `pop_front`
    /// eviction discipline as `in_flight`, so a long session cannot grow it.
    /// It lives entirely on the shell side of the boundary: `ProjectionChannel`
    /// runs on the window's event thread and nothing here is reachable from
    /// the real-time callback.
    retired_identities: VecDeque<RetiredIdentity>,
}

impl ProjectionChannel {
    /// Creates a channel that has emitted nothing yet, so the first accepted
    /// projection always emits.
    pub const fn new() -> Self {
        Self {
            last_emitted_generation: None,
            in_flight: VecDeque::new(),
            retired_identities: VecDeque::new(),
        }
    }

    /// Pushes the projection's embedded semantic model through `emit` if its
    /// accepted generation differs from the last successfully emitted one.
    ///
    /// `emit` receives the complete `serde_json` document of the embedded
    /// [`SemanticGraphicalViewModel`] and performs the actual tauri emit on
    /// [`PROJECTION_EVENT`]; injecting it keeps the gating logic provable
    /// without a tauri runtime (the `Emitter` trait is sealed).
    ///
    /// [`SemanticGraphicalViewModel`]: crate::control::SemanticGraphicalViewModel
    pub fn push<E>(
        &mut self,
        projection: &GraphicalShellProjection,
        emit: E,
    ) -> Result<ProjectionPush, ProjectionChannelError>
    where
        E: FnOnce(serde_json::Value) -> tauri::Result<()>,
    {
        let generation = projection.generation();
        if self.last_emitted_generation == Some(generation) {
            return Ok(ProjectionPush::Unchanged);
        }
        let document = serde_json::to_value(projection.semantic_model())
            .map_err(|source| ProjectionChannelError::Serialization { generation, source })?;
        let identity = ack_identity_of(&document);
        emit(document).map_err(|source| ProjectionChannelError::Emit { generation, source })?;
        self.last_emitted_generation = Some(generation);
        // Track the emitted document so its painted ack can correlate. The
        // tracker is bounded: when full, the oldest unacked document is a
        // lost frame — dropped, degrading observation only. Retirement path
        // one: the dropped document's identity is retired, not discarded, so
        // its late ack is still held to the verbatim-copy rule (FR-003).
        if self.in_flight.len() == MAX_IN_FLIGHT_DOCUMENTS {
            if let Some(evicted) = self.in_flight.pop_front() {
                self.retire(evicted);
            }
        }
        self.in_flight.push_back(InFlightDocument {
            generation,
            identity,
            semantic: projection.semantic_model().clone(),
        });
        Ok(ProjectionPush::Emitted)
    }

    /// How many pushed documents currently await their painted ack.
    pub fn in_flight_documents(&self) -> usize {
        self.in_flight.len()
    }

    /// Receives one painted ack and constructs exactly one
    /// [`ShellFrameObservation`] for it, consumes it as a lost late frame
    /// ([`ForwardedAck::SupersededLate`]), or rejects it with a typed
    /// [`PaintedAckError`].
    ///
    /// `payload` is the raw JSON the page emitted on [`PAINTED_EVENT`]:
    /// the document's semantic identity — generation, stateHash, context,
    /// activeSurface, focusPath, interactionMode — copied verbatim, plus
    /// the measured viewport (`viewport.widthPx`/`heightPx`) and the five
    /// painted region rectangles with their visible labels (`regions[]`,
    /// ids spelled as `ShellRegionId` serialized names).
    ///
    /// The ack must correlate with an in-flight pushed document and copy its
    /// identity exactly; the observation is then constructed against that
    /// document's own semantic model with the ack's measured geometry, so an
    /// observation can never be built from expectations alone. A successful
    /// ack consumes its document and every older unacked one (superseded —
    /// lost frames degrade observation only), retiring each one's identity.
    ///
    /// An ack that correlates with no in-flight document is one of three
    /// things (FR-003):
    ///
    /// - newer than every emission — fabricated evidence, rejected as
    ///   [`PaintedAckError::UnknownDocument`];
    /// - naming a retired generation still inside the retained identity
    ///   window — held to the *same* verbatim-copy rule as an in-flight ack,
    ///   so a rewritten identity is [`PaintedAckError::IdentityMismatch`] and
    ///   a faithful one is [`ForwardedAck::SupersededLate`];
    /// - naming a generation older than that window — a lost frame the
    ///   channel can no longer speak to, consumed as
    ///   [`ForwardedAck::SupersededLate`].
    ///
    /// No superseded-late ack constructs an observation in any of those
    /// cases.
    pub fn forward_ack(&mut self, payload: &str) -> Result<ForwardedAck, PaintedAckError> {
        let ack: Value =
            serde_json::from_str(payload).map_err(|error| PaintedAckError::Malformed {
                detail: format!("payload is not JSON: {error}"),
            })?;
        let generation = ack
            .get("generation")
            .and_then(Value::as_u64)
            .ok_or_else(|| PaintedAckError::Malformed {
                detail: "ack carries no numeric generation".to_owned(),
            })?;
        let Some(index) = self
            .in_flight
            .iter()
            .position(|document| document.generation == generation)
        else {
            // Generations are monotone, so an ack at or below the newest
            // emitted generation names a document this channel pushed and
            // has since dropped (superseded before its ack arrived): a lost
            // frame, not fabricated evidence. Anything newer than every
            // emission cannot have been painted from a pushed document.
            return if self
                .last_emitted_generation
                .is_some_and(|newest| generation <= newest)
            {
                // The document is gone, but its identity may still be inside
                // the retained window. If it is, the ack answers to exactly
                // the same verbatim-copy rule as an in-flight one: the
                // guarantee holds in every window, not only where the channel
                // happens to still hold the document (FR-003).
                if let Some(retired) = self
                    .retired_identities
                    .iter()
                    .find(|retired| retired.generation == generation)
                {
                    verify_ack_identity(&ack, &retired.identity, generation)?;
                }
                // A miss means the ack outlived the retained window. That
                // window is finite by design — retaining every identity for
                // the life of the session was rejected — so the channel holds
                // nothing to compare against and makes no claim either way:
                // today's lost-frame behavior stands. Do not "tighten" this
                // into a rejection; an honest ack delayed past the window
                // would then fail a healthy run.
                Ok(ForwardedAck::SupersededLate { generation })
            } else {
                Err(PaintedAckError::UnknownDocument { generation })
            };
        };

        let document = &self.in_flight[index];
        verify_ack_identity(&ack, &document.identity, generation)?;

        let viewport = ack
            .get("viewport")
            .ok_or_else(|| PaintedAckError::Malformed {
                detail: "ack carries no measured viewport".to_owned(),
            })?;
        let viewport_width = measured_px(viewport, "widthPx")?;
        let viewport_height = measured_px(viewport, "heightPx")?;
        let supplied_regions = ack
            .get("regions")
            .and_then(Value::as_array)
            .ok_or_else(|| PaintedAckError::Malformed {
                detail: "ack carries no measured regions".to_owned(),
            })?;
        if supplied_regions.len() != ShellRegionId::ALL.len() {
            return Err(PaintedAckError::Malformed {
                detail: format!(
                    "ack carries {} measured regions, the shell declares {}",
                    supplied_regions.len(),
                    ShellRegionId::ALL.len()
                ),
            });
        }
        let regions = supplied_regions
            .iter()
            .map(measured_region)
            .collect::<Result<Vec<_>, _>>()?;
        let regions: [ShellRegionObservation; 5] = regions
            .try_into()
            .expect("the region count was checked against the declared shell regions");

        let observation = ShellFrameObservation::try_new_semantic(
            viewport_width,
            viewport_height,
            &document.semantic,
            regions,
        )
        .map_err(|source| PaintedAckError::InvalidObservation { generation, source })?;
        // The page's own strip-paint evidence, transported rather than
        // recomputed (F-44). Absent when the painting host reports none; a
        // present-but-malformed `strip` is a malformed ack, not a silent zero,
        // because a zero here is a measurement the witness reads.
        let observation = match ack.get("strip") {
            None => observation,
            Some(strip) => observation.with_strip_paint(measured_strip(strip)?),
        };

        // The ack consumed its document. Everything older was superseded
        // without ever acking — dropped, degrading observation only.
        // Retirement path two: this drops a *range*, so every document in it
        // retires its identity, oldest first, not just the acked one —
        // otherwise the superseded documents (the common retirement) would
        // escape validation entirely (FR-003).
        for _ in 0..=index {
            let retired = self
                .in_flight
                .pop_front()
                .expect("the drained range was located by position within this deque");
            self.retire(retired);
        }
        Ok(ForwardedAck::Observation(observation))
    }

    /// Moves one dropped document's identity into the bounded retained
    /// window, evicting the oldest retained identity when full — the same
    /// `pop_front` discipline `in_flight` uses.
    fn retire(&mut self, document: InFlightDocument) {
        // Generations are monotone in production, but a channel re-pushed at
        // an already-retired generation must not leave two entries for it:
        // a duplicate wastes a slot in the window and could let a stale
        // identity shadow the current one.
        if let Some(stale) = self
            .retired_identities
            .iter()
            .position(|retired| retired.generation == document.generation)
        {
            self.retired_identities.remove(stale);
        }
        if self.retired_identities.len() == RETAINED_RETIRED_IDENTITIES {
            self.retired_identities.pop_front();
        }
        self.retired_identities.push_back(RetiredIdentity {
            generation: document.generation,
            identity: document.identity,
        });
    }
}

/// Holds one ack to the verbatim-copy rule against the identity of the
/// document it named.
///
/// This is the single place the rule is spelled: both windows call it — the
/// in-flight window, where the document is still tracked, and the retained
/// retired window, where only the identity survives. One implementation is
/// the point; two copies of an identity rule drift (FR-003). A field the ack
/// omits entirely compares as `Value::Null`, so omission is a mismatch rather
/// than a pass.
fn verify_ack_identity(
    ack: &Value,
    identity: &[Value; ACK_IDENTITY_FIELDS.len()],
    generation: u64,
) -> Result<(), PaintedAckError> {
    for (field, expected) in ACK_IDENTITY_FIELDS.iter().zip(identity) {
        let supplied = ack.get(*field).unwrap_or(&Value::Null);
        if supplied != expected {
            return Err(PaintedAckError::IdentityMismatch { generation, field });
        }
    }
    Ok(())
}

/// Copies the serialized identity fields the painted ack must echo out of a
/// pushed document.
fn ack_identity_of(document: &Value) -> [Value; ACK_IDENTITY_FIELDS.len()] {
    ACK_IDENTITY_FIELDS.map(|field| document.get(field).cloned().unwrap_or(Value::Null))
}

/// Reads one measured pixel value from the ack's geometry.
fn measured_px(value: &Value, field: &str) -> Result<f32, PaintedAckError> {
    value
        .get(field)
        .and_then(Value::as_f64)
        .map(|px| px as f32)
        .ok_or_else(|| PaintedAckError::Malformed {
            detail: format!("ack geometry carries no numeric {field}"),
        })
}

/// Reads the page's own strip-paint evidence from the ack's `strip` object.
///
/// Both leaves are required once the object is present: the witness reads
/// these as measurements, so a missing leaf must be a malformed ack rather
/// than a fabricated zero.
fn measured_strip(value: &Value) -> Result<StripPaintObservation, PaintedAckError> {
    let groups_painted = value
        .get("groupsPainted")
        .and_then(Value::as_u64)
        .ok_or_else(|| PaintedAckError::Malformed {
            detail: "ack strip carries no numeric groupsPainted".to_owned(),
        })?;
    let flat_control_run = value
        .get("flatControlRun")
        .and_then(Value::as_bool)
        .ok_or_else(|| PaintedAckError::Malformed {
            detail: "ack strip carries no boolean flatControlRun".to_owned(),
        })?;
    Ok(StripPaintObservation::new(
        u32::try_from(groups_painted).map_err(|_| PaintedAckError::Malformed {
            detail: format!("ack strip groupsPainted {groups_painted} is out of range"),
        })?,
        flat_control_run,
    ))
}

/// Reads one measured region — stable id, painted bounds, visible label —
/// from the ack's `regions[]` entry.
fn measured_region(value: &Value) -> Result<ShellRegionObservation, PaintedAckError> {
    let name =
        value
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| PaintedAckError::Malformed {
                detail: "ack region carries no id".to_owned(),
            })?;
    let id = ShellRegionId::ALL
        .into_iter()
        .find(|id| id.name() == name)
        .ok_or_else(|| PaintedAckError::Malformed {
            detail: format!("ack region id {name} names no declared shell region"),
        })?;
    let x = measured_px(value, "xPx")?;
    let y = measured_px(value, "yPx")?;
    let width = measured_px(value, "widthPx")?;
    let height = measured_px(value, "heightPx")?;
    let label = value.get("label").and_then(Value::as_str).unwrap_or("");
    Ok(ShellRegionObservation::new(
        id,
        ShellRegionRect::new(x, y, x + width, y + height),
        label,
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        ForwardedAck, PaintedAckError, ProjectionChannel, ProjectionChannelError, ProjectionPush,
        MAX_IN_FLIGHT_DOCUMENTS, PAINTED_EVENT, PROJECTION_EVENT, READY_EVENT, RENDER_ERROR_EVENT,
        RETAINED_RETIRED_IDENTITIES,
    };
    use crate::control::{
        GraphicalShellProjection, SemanticGraphicalViewModel, ShellContextLine, ShellFooter,
        ShellIdentityHeader, TextProjection, TopLevelContext,
    };
    use crate::shell::{ShellFrameObservationError, ShellRegionId};
    use serde_json::{json, Value};
    use std::cell::RefCell;

    fn projection(generation: u64, state_hash: &str) -> GraphicalShellProjection {
        GraphicalShellProjection::new(
            generation,
            state_hash,
            SemanticGraphicalViewModel::fixture(generation, state_hash, TopLevelContext::Mixer),
            ShellContextLine::new("CREST SYNTH", "MIXER", "READY"),
            ShellIdentityHeader::new("MIXER", "Transport fixture"),
            "MIXER WORKSPACE",
            "INSPECTOR",
            TextProjection::for_context(
                TopLevelContext::Mixer,
                "MIXER diagnostic".to_owned(),
                0,
                state_hash.to_owned(),
            ),
            ShellFooter::new("MIXER", vec!["1 MIXER".to_owned(), "2 PATCH".to_owned()]),
        )
        .expect("the transport test fixture is coherent")
    }

    #[test]
    fn projection_event_name_satisfies_the_tauri_event_charset() {
        assert!(PROJECTION_EVENT.chars().all(|c| c.is_alphanumeric()
            || c == '-'
            || c == '/'
            || c == ':'
            || c == '_'));
    }

    #[test]
    fn a_new_generation_emits_the_exact_projector_serialization_once() {
        let mut channel = ProjectionChannel::new();
        let projection = projection(7, "state-7");
        let emitted: RefCell<Vec<Value>> = RefCell::new(Vec::new());

        let outcome = channel
            .push(&projection, |document| {
                emitted.borrow_mut().push(document);
                Ok(())
            })
            .expect("a live emit succeeds");

        assert_eq!(outcome, ProjectionPush::Emitted);
        let emitted = emitted.into_inner();
        assert_eq!(emitted.len(), 1);
        // The payload is the projector's own serialization of the embedded
        // semantic model — the whole document, byte-for-byte equal, with the
        // declared top-level properties and nothing added or trimmed.
        let expected = serde_json::to_value(projection.semantic_model())
            .expect("the projector's model serializes");
        assert_eq!(emitted[0], expected);
        let Value::Object(document) = &emitted[0] else {
            panic!("the emitted payload must be the serialized document object");
        };
        let mut keys: Vec<&str> = document.keys().map(String::as_str).collect();
        keys.sort_unstable();
        let mut declared: Vec<&str> =
            SemanticGraphicalViewModel::SERIALIZED_PROPERTY_DESCRIPTOR.to_vec();
        declared.sort_unstable();
        assert_eq!(keys, declared);
        assert_eq!(document["generation"], Value::from(7));
        assert_eq!(document["stateHash"], Value::from("state-7"));
    }

    #[test]
    fn an_unchanged_generation_emits_nothing() {
        let mut channel = ProjectionChannel::new();
        let projection = projection(7, "state-7");
        let mut emits = 0_usize;

        for _ in 0..3 {
            let outcome = channel
                .push(&projection, |_| {
                    emits += 1;
                    Ok(())
                })
                .expect("pushing never fails with a succeeding emitter");
            if emits == 1 {
                continue;
            }
            assert_eq!(outcome, ProjectionPush::Unchanged);
        }

        assert_eq!(emits, 1, "same generation on every later tick: no emit");
    }

    #[test]
    fn a_changed_generation_emits_exactly_one_more_document() {
        let mut channel = ProjectionChannel::new();
        let first = projection(7, "state-7");
        let second = first
            .with_generation(
                8,
                "state-8".to_owned(),
                TextProjection::for_context(
                    TopLevelContext::Mixer,
                    "MIXER diagnostic".to_owned(),
                    0,
                    "state-8".to_owned(),
                ),
            )
            .expect("advancing the fixture generation stays coherent");
        let mut generations: Vec<u64> = Vec::new();

        for projection in [&first, &first, &second, &second] {
            channel
                .push(projection, |document| {
                    generations.push(document["generation"].as_u64().unwrap());
                    Ok(())
                })
                .expect("pushing never fails with a succeeding emitter");
        }

        assert_eq!(generations, [7, 8], "one emit per accepted generation");
    }

    #[test]
    fn a_failed_emit_is_typed_and_leaves_the_gate_unmoved() {
        let mut channel = ProjectionChannel::new();
        let projection = projection(7, "state-7");

        let error = channel
            .push(&projection, |_| {
                Err(tauri::Error::from(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "window gone",
                )))
            })
            .expect_err("a failing emitter must surface a typed error");
        match &error {
            ProjectionChannelError::Emit { generation, .. } => assert_eq!(*generation, 7),
            other => panic!("expected a typed emit failure, got {other:?}"),
        }
        assert!(error.to_string().contains("generation 7"));

        // The gate did not advance: the next tick with the same generation
        // retries instead of silently dropping the accepted document.
        let outcome = channel
            .push(&projection, |_| Ok(()))
            .expect("the retry emit succeeds");
        assert_eq!(outcome, ProjectionPush::Emitted);
    }

    // ------------------------------------------------------------------
    // Painted-ack forwarding (WP02 T005/T009)
    // ------------------------------------------------------------------

    fn advanced(
        projection: &GraphicalShellProjection,
        generation: u64,
    ) -> GraphicalShellProjection {
        let state_hash = format!("state-{generation}");
        projection
            .with_generation(
                generation,
                state_hash.clone(),
                TextProjection::for_context(
                    TopLevelContext::Mixer,
                    "MIXER diagnostic".to_owned(),
                    0,
                    state_hash,
                ),
            )
            .expect("advancing the fixture generation stays coherent")
    }

    fn push_ok(channel: &mut ProjectionChannel, projection: &GraphicalShellProjection) {
        let outcome = channel
            .push(projection, |_| Ok(()))
            .expect("the fixture emit succeeds");
        assert_eq!(outcome, ProjectionPush::Emitted);
    }

    /// The negotiated ack payload: the document's serialized identity copied
    /// verbatim, plus measured viewport and region geometry — exactly what
    /// the page's `paintedEvidence` emits after painting.
    fn ack_for(projection: &GraphicalShellProjection) -> Value {
        let document = serde_json::to_value(projection.semantic_model())
            .expect("the projector's model serializes");
        json!({
            "generation": document["generation"],
            "stateHash": document["stateHash"],
            "context": document["context"],
            "activeSurface": document["activeSurface"],
            "focusPath": document["focusPath"],
            "interactionMode": document["interactionMode"],
            "viewport": { "widthPx": 1920.0, "heightPx": 1080.0 },
            "regions": [
                { "id": "contextLine", "xPx": 0.0, "yPx": 0.0,
                  "widthPx": 1920.0, "heightPx": 48.0, "label": "CREST SYNTH" },
                { "id": "identityHeader", "xPx": 0.0, "yPx": 48.0,
                  "widthPx": 1920.0, "heightPx": 72.0, "label": "MIXER" },
                { "id": "mainWorkspace", "xPx": 0.0, "yPx": 120.0,
                  "widthPx": 1500.0, "heightPx": 896.0, "label": "MIXER WORKSPACE" },
                { "id": "persistentSideRegion", "xPx": 1500.0, "yPx": 120.0,
                  "widthPx": 420.0, "heightPx": 896.0, "label": "INSPECTOR" },
                { "id": "footer", "xPx": 0.0, "yPx": 1016.0,
                  "widthPx": 1920.0, "heightPx": 64.0, "label": "MIXER" },
            ],
        })
    }

    /// The page's own strip-paint evidence is transported verbatim, and a
    /// present-but-malformed `strip` is a malformed ack rather than a silent
    /// zero — a zero here is a measurement the witness reads (F-44).
    #[test]
    fn the_pages_strip_paint_evidence_is_transported_and_never_invented() {
        // Absent: the observation carries none, and none is not a zero.
        let mut channel = ProjectionChannel::new();
        let projection = projection(7, "state-7");
        push_ok(&mut channel, &projection);
        let ForwardedAck::Observation(observation) = channel
            .forward_ack(&ack_for(&projection).to_string())
            .expect("the ack forwards")
        else {
            panic!("an in-flight ack constructs an observation");
        };
        assert_eq!(observation.strip(), None);

        // Present: copied across exactly, with no Rust-side re-derivation.
        let mut channel = ProjectionChannel::new();
        push_ok(&mut channel, &projection);
        let mut ack = ack_for(&projection);
        ack["strip"] = json!({ "groupsPainted": 4, "flatControlRun": false });
        let ForwardedAck::Observation(observation) = channel
            .forward_ack(&ack.to_string())
            .expect("the ack forwards")
        else {
            panic!("an in-flight ack constructs an observation");
        };
        let strip = observation.strip().expect("the ack carried strip evidence");
        assert_eq!(strip.groups_painted(), 4);
        assert!(!strip.flat_control_run());

        // A flat run is reported as one, not smoothed away.
        let mut channel = ProjectionChannel::new();
        push_ok(&mut channel, &projection);
        let mut ack = ack_for(&projection);
        ack["strip"] = json!({ "groupsPainted": 0, "flatControlRun": true });
        let ForwardedAck::Observation(observation) = channel
            .forward_ack(&ack.to_string())
            .expect("the ack forwards")
        else {
            panic!("an in-flight ack constructs an observation");
        };
        let strip = observation.strip().expect("the ack carried strip evidence");
        assert_eq!(strip.groups_painted(), 0);
        assert!(strip.flat_control_run());

        // Half a strip object is malformed, never a defaulted zero.
        for partial in [
            json!({ "groupsPainted": 4 }),
            json!({ "flatControlRun": false }),
            json!({ "groupsPainted": "four", "flatControlRun": false }),
        ] {
            let mut channel = ProjectionChannel::new();
            push_ok(&mut channel, &projection);
            let mut ack = ack_for(&projection);
            ack["strip"] = partial;
            assert!(
                matches!(
                    channel.forward_ack(&ack.to_string()),
                    Err(PaintedAckError::Malformed { .. })
                ),
                "an incomplete strip object must be a malformed ack",
            );
        }
    }

    #[test]
    fn event_names_satisfy_the_tauri_event_charset() {
        for event in [PAINTED_EVENT, READY_EVENT, RENDER_ERROR_EVENT] {
            assert!(event.chars().all(|c| c.is_alphanumeric()
                || c == '-'
                || c == '/'
                || c == ':'
                || c == '_'));
        }
    }

    #[test]
    fn one_pushed_document_and_its_ack_become_exactly_one_verbatim_observation() {
        let mut channel = ProjectionChannel::new();
        let projection = projection(7, "state-7");
        push_ok(&mut channel, &projection);
        assert_eq!(channel.in_flight_documents(), 1);

        let payload = ack_for(&projection).to_string();
        let ForwardedAck::Observation(observation) = channel
            .forward_ack(&payload)
            .expect("a matching painted ack becomes an observation")
        else {
            panic!("a matching painted ack must construct the observation");
        };

        // Identity is copied verbatim from the pushed document.
        let model = projection.semantic_model();
        assert_eq!(observation.generation(), model.generation());
        assert_eq!(observation.state_hash(), model.state_hash());
        assert_eq!(observation.context(), model.context());
        assert_eq!(observation.active_surface(), model.active_surface());
        assert_eq!(observation.focus_path(), model.focus_path());
        assert_eq!(observation.interaction_mode(), model.interaction_mode());
        // Geometry and labels come from the ack's measured evidence.
        assert_eq!(observation.viewport_width(), 1920.0);
        assert_eq!(observation.viewport_height(), 1080.0);
        let footer = observation.region(ShellRegionId::Footer);
        assert_eq!(footer.rect().min_y(), 1016.0);
        assert_eq!(footer.rect().max_y(), 1080.0);
        assert_eq!(footer.visible_label(), "MIXER");
        assert!(observation.regions_are_non_overlapping());

        // Exactly one: the ack consumed its document, so replaying it can
        // construct nothing — it is consumed as a late superseded ack.
        assert_eq!(channel.in_flight_documents(), 0);
        let replay = channel
            .forward_ack(&payload)
            .expect("a replayed ack is consumed without constructing anything");
        assert_eq!(replay, ForwardedAck::SupersededLate { generation: 7 });
    }

    #[test]
    fn an_ack_with_no_pushed_document_is_a_typed_rejection() {
        let mut channel = ProjectionChannel::new();
        // Nothing was pushed: the ack describes a frame nobody painted from
        // a pushed document, so no observation is constructible from it.
        let payload = ack_for(&projection(7, "state-7")).to_string();

        let error = channel
            .forward_ack(&payload)
            .expect_err("an uncorrelated ack must be rejected");
        assert!(matches!(
            error,
            PaintedAckError::UnknownDocument { generation: 7 }
        ));
        assert!(error.to_string().contains("generation 7"));
        assert_eq!(channel.in_flight_documents(), 0);

        // With a document pushed, an ack claiming a generation newer than
        // every emission is still fabricated evidence — only late acks for
        // pushed-then-superseded documents are consumed quietly.
        let pushed = projection(7, "state-7");
        push_ok(&mut channel, &pushed);
        let future = channel
            .forward_ack(&ack_for(&advanced(&pushed, 9)).to_string())
            .expect_err("an ack newer than every emission must be rejected");
        assert!(matches!(
            future,
            PaintedAckError::UnknownDocument { generation: 9 }
        ));
    }

    #[test]
    fn an_ack_that_rewrites_identity_is_a_typed_mismatch() {
        let mut channel = ProjectionChannel::new();
        let projection = projection(7, "state-7");
        push_ok(&mut channel, &projection);

        let mut ack = ack_for(&projection);
        ack["stateHash"] = Value::from("state-invented");
        let error = channel
            .forward_ack(&ack.to_string())
            .expect_err("an identity-rewriting ack must be rejected");
        match error {
            PaintedAckError::IdentityMismatch { generation, field } => {
                assert_eq!(generation, 7);
                assert_eq!(field, "stateHash");
            }
            other => panic!("expected an identity mismatch, got {other:?}"),
        }
    }

    #[test]
    fn a_superseded_document_that_never_acks_is_dropped_when_its_successor_acks() {
        let mut channel = ProjectionChannel::new();
        let first = projection(7, "state-7");
        let second = advanced(&first, 8);
        push_ok(&mut channel, &first);
        push_ok(&mut channel, &second);
        assert_eq!(channel.in_flight_documents(), 2);

        // The first document's ack never arrives; the successor's does.
        channel
            .forward_ack(&ack_for(&second).to_string())
            .expect("the successor's ack becomes its one observation");

        // The superseded document was dropped from tracking — a lost frame
        // degrades observation only, and its late ack constructs nothing:
        // it is consumed quietly rather than failing the window (WP03).
        assert_eq!(channel.in_flight_documents(), 0);
        let late = channel
            .forward_ack(&ack_for(&first).to_string())
            .expect("a superseded document's late ack is consumed quietly");
        assert_eq!(late, ForwardedAck::SupersededLate { generation: 7 });
    }

    #[test]
    fn the_in_flight_tracker_is_bounded() {
        let mut channel = ProjectionChannel::new();
        let base = projection(1, "state-1");
        push_ok(&mut channel, &base);
        let count = MAX_IN_FLIGHT_DOCUMENTS as u64 + 3;
        for generation in 2..=count {
            push_ok(&mut channel, &advanced(&base, generation));
        }

        // No ack ever arrived, yet tracking never grew past the bound; the
        // oldest documents were dropped, the newest still correlates. The
        // evicted document's late ack is a lost frame consumed quietly —
        // sustained generation churn must never turn paced late acks into
        // a fatal runtime failure (WP03).
        assert_eq!(channel.in_flight_documents(), MAX_IN_FLIGHT_DOCUMENTS);
        let oldest = channel
            .forward_ack(&ack_for(&base).to_string())
            .expect("the evicted oldest document's late ack is consumed quietly");
        assert_eq!(oldest, ForwardedAck::SupersededLate { generation: 1 });
        let newest = channel
            .forward_ack(&ack_for(&advanced(&base, count)).to_string())
            .expect("the newest document still correlates");
        assert!(matches!(newest, ForwardedAck::Observation(_)));
    }

    // ------------------------------------------------------------------
    // The verbatim-copy rule in the superseded-late window (FR-003 / RISK-4)
    // ------------------------------------------------------------------

    /// Pushes `1..=newest` so the ack-consumption path can retire a range.
    fn pushed_through(
        channel: &mut ProjectionChannel,
        base: &GraphicalShellProjection,
        newest: u64,
    ) {
        push_ok(channel, base);
        for generation in 2..=newest {
            push_ok(channel, &advanced(base, generation));
        }
    }

    #[test]
    fn a_drained_document_still_holds_its_late_ack_to_the_verbatim_rule() {
        // Retirement path one: ack consumption. The successor's ack drains
        // its own document *and* the older unacked one, retiring both.
        let mut channel = ProjectionChannel::new();
        let first = projection(7, "state-7");
        let second = advanced(&first, 8);
        push_ok(&mut channel, &first);
        push_ok(&mut channel, &second);
        channel
            .forward_ack(&ack_for(&second).to_string())
            .expect("the successor's ack becomes its one observation");
        assert_eq!(channel.in_flight_documents(), 0);

        // Two different rewritten fields: the comparison is the whole
        // identity, not one privileged field.
        for (field, rewritten) in [
            ("stateHash", Value::from("state-invented")),
            ("activeSurface", Value::from("inventedSurface")),
        ] {
            let mut ack = ack_for(&first);
            ack[field] = rewritten;
            let error = channel
                .forward_ack(&ack.to_string())
                .expect_err("a superseded-late ack that rewrites identity must be rejected");
            match error {
                PaintedAckError::IdentityMismatch {
                    generation,
                    field: named,
                } => {
                    assert_eq!(generation, 7);
                    assert_eq!(named, field);
                }
                other => panic!("expected an identity mismatch on {field}, got {other:?}"),
            }
        }

        // The acked document retires too: replaying its own ack with a
        // rewritten identity is rejected on the same rule.
        let mut replay = ack_for(&second);
        replay["interactionMode"] = Value::from("inventedMode");
        let error = channel
            .forward_ack(&replay.to_string())
            .expect_err("the consumed document's identity is retired too");
        assert!(matches!(
            error,
            PaintedAckError::IdentityMismatch {
                generation: 8,
                field: "interactionMode"
            }
        ));
    }

    #[test]
    fn a_capacity_evicted_document_still_holds_its_late_ack_to_the_verbatim_rule() {
        // Retirement path two: capacity eviction. Pushing one past the bound
        // drops the oldest unacked document and retires its identity.
        let mut channel = ProjectionChannel::new();
        let base = projection(1, "state-1");
        pushed_through(&mut channel, &base, MAX_IN_FLIGHT_DOCUMENTS as u64 + 1);
        assert_eq!(channel.in_flight_documents(), MAX_IN_FLIGHT_DOCUMENTS);

        let mut ack = ack_for(&base);
        ack["context"] = Value::from("INVENTED");
        let error = channel
            .forward_ack(&ack.to_string())
            .expect_err("an evicted document's rewritten late ack must be rejected");
        match error {
            PaintedAckError::IdentityMismatch { generation, field } => {
                assert_eq!(generation, 1);
                assert_eq!(field, "context");
            }
            other => panic!("expected an identity mismatch, got {other:?}"),
        }
    }

    #[test]
    fn a_faithful_superseded_late_ack_is_consumed_exactly_as_before() {
        // Nothing this mission adds narrows what already worked: a late ack
        // that copies its retired document's identity verbatim is still the
        // quiet lost frame it was, from either retirement path.
        let mut evicted = ProjectionChannel::new();
        let base = projection(1, "state-1");
        pushed_through(&mut evicted, &base, MAX_IN_FLIGHT_DOCUMENTS as u64 + 1);
        let late = evicted
            .forward_ack(&ack_for(&base).to_string())
            .expect("a faithful late ack for an evicted document is consumed quietly");
        assert_eq!(late, ForwardedAck::SupersededLate { generation: 1 });

        let mut drained = ProjectionChannel::new();
        let first = projection(7, "state-7");
        let second = advanced(&first, 8);
        push_ok(&mut drained, &first);
        push_ok(&mut drained, &second);
        drained
            .forward_ack(&ack_for(&second).to_string())
            .expect("the successor's ack becomes its one observation");
        for (projection, generation) in [(&first, 7), (&second, 8)] {
            let late = drained
                .forward_ack(&ack_for(projection).to_string())
                .expect("a faithful late ack for a drained document is consumed quietly");
            assert_eq!(late, ForwardedAck::SupersededLate { generation });
        }
    }

    #[test]
    fn an_ack_older_than_the_retained_window_stays_a_lost_frame() {
        let mut channel = ProjectionChannel::new();
        let base = projection(1, "state-1");
        // Strictly more retirements than the window retains: this evicts
        // generations 1..=RETAINED_RETIRED_IDENTITIES + 1, so generation 1
        // has fallen out of the retained window and generation 2 — the
        // oldest still retained — has not.
        let count = (MAX_IN_FLIGHT_DOCUMENTS + RETAINED_RETIRED_IDENTITIES) as u64 + 1;
        pushed_through(&mut channel, &base, count);
        assert_eq!(channel.in_flight_documents(), MAX_IN_FLIGHT_DOCUMENTS);

        // Outside the window the channel holds nothing to compare against and
        // makes no claim either way. Both a faithful and a rewritten ack stay
        // lost frames: rejecting here would fail an honest ack merely delayed
        // past the window.
        let faithful = channel
            .forward_ack(&ack_for(&base).to_string())
            .expect("an ack older than the retained window is still a lost frame");
        assert_eq!(faithful, ForwardedAck::SupersededLate { generation: 1 });

        let mut rewritten = ack_for(&base);
        rewritten["stateHash"] = Value::from("state-invented");
        let outside = channel
            .forward_ack(&rewritten.to_string())
            .expect("outside the retained window a rewritten ack is not a rejection");
        assert_eq!(outside, ForwardedAck::SupersededLate { generation: 1 });
    }

    #[test]
    fn the_retained_window_holds_exactly_its_bound_of_identities() {
        // The same overflowing run as the test above, read from the other
        // side: generation 1 fell out of the window, and generation 2 — the
        // oldest identity still inside it — did not. That is where the bound
        // puts the boundary, and the window never grows past it.
        let mut channel = ProjectionChannel::new();
        let base = projection(1, "state-1");
        let count = (MAX_IN_FLIGHT_DOCUMENTS + RETAINED_RETIRED_IDENTITIES) as u64 + 1;
        pushed_through(&mut channel, &base, count);

        let mut oldest_retained = ack_for(&advanced(&base, 2));
        oldest_retained["stateHash"] = Value::from("state-invented");
        let error = channel
            .forward_ack(&oldest_retained.to_string())
            .expect_err("the oldest retained identity still validates its late ack");
        assert!(matches!(
            error,
            PaintedAckError::IdentityMismatch {
                generation: 2,
                field: "stateHash"
            }
        ));
    }

    /// `retire()`'s de-duplication guard, proved by the false rejection it
    /// prevents (WP03 T018/T019).
    ///
    /// The retained window is searched with `find`, which answers with the
    /// *first* entry carrying the acked generation. So a generation retired
    /// twice — once with an identity the channel has since replaced, once with
    /// the identity it actually pushed last — leaves the stale copy shadowing
    /// the current one at the front of the window. The ack that then arrives is
    /// a faithful verbatim copy of the document this channel really emitted,
    /// and without the guard it is rejected as an `IdentityMismatch`: the
    /// channel calls a page honest evidence a lie. That is strictly worse than
    /// the lost frame it should have been, because a typed ack rejection takes
    /// the window's record-first-error-and-close path.
    ///
    /// Removing the `retired_identities.remove(stale)` block in
    /// [`ProjectionChannel::retire`] makes the final assertion below fail with
    /// `IdentityMismatch { generation: 1, field: "stateHash" }`.
    #[test]
    fn a_re_pushed_generation_retires_once_so_no_stale_identity_shadows_the_current_one() {
        let mut channel = ProjectionChannel::new();

        // Round one: emit generation 1, supersede it with generation 2, and
        // let 2's ack drain the pair. Generation 1's *first* identity is now
        // in the retained window.
        let stale = projection(1, "state-1-stale");
        let superseding = advanced(&stale, 2);
        push_ok(&mut channel, &stale);
        push_ok(&mut channel, &superseding);
        channel
            .forward_ack(&ack_for(&superseding).to_string())
            .expect("the successor's ack becomes its one observation");
        assert_eq!(channel.in_flight_documents(), 0);

        // Round two: the channel emits generation 1 again, carrying a
        // different accepted document. This is the situation the guard names —
        // generations are monotone in production, so it takes a re-push to
        // reach it, and the guard exists precisely so reaching it is harmless.
        let current = projection(1, "state-1-current");
        push_ok(&mut channel, &current);
        let successor = advanced(&current, 3);
        push_ok(&mut channel, &successor);
        channel
            .forward_ack(&ack_for(&successor).to_string())
            .expect("the successor's ack becomes its one observation");
        assert_eq!(channel.in_flight_documents(), 0);

        // The window still speaks for the generations that retired only once.
        let mut rewritten = ack_for(&superseding);
        rewritten["stateHash"] = Value::from("state-invented");
        assert!(matches!(
            channel
                .forward_ack(&rewritten.to_string())
                .expect_err("an unrelated retired generation still validates its late ack"),
            PaintedAckError::IdentityMismatch {
                generation: 2,
                field: "stateHash"
            }
        ));

        // A rewritten identity for the twice-retired generation is still a
        // rejection, from either copy — so passing the assertion below cannot
        // be achieved by the window simply forgetting generation 1.
        let mut invented = ack_for(&current);
        invented["stateHash"] = Value::from("state-invented");
        assert!(matches!(
            channel
                .forward_ack(&invented.to_string())
                .expect_err("a rewritten late ack is rejected in either copy"),
            PaintedAckError::IdentityMismatch {
                generation: 1,
                field: "stateHash"
            }
        ));

        // The falsifying assertion: the page's honest ack for the document
        // this channel really pushed last. With the guard the window holds one
        // generation-1 identity — the current one — and the ack is the lost
        // frame it is. Without the guard `find` reaches the stale copy first
        // and rejects a truthful ack.
        let honest = channel
            .forward_ack(&ack_for(&current).to_string())
            .expect("a verbatim ack for the re-pushed document must not be rejected");
        assert_eq!(honest, ForwardedAck::SupersededLate { generation: 1 });
    }

    #[test]
    fn a_superseded_late_ack_never_constructs_an_observation() {
        // The invariant that keeps RISK-4 latent survives the new validation:
        // whatever a superseded-late ack carries, it is consumed or rejected,
        // never turned into painted evidence.
        let mut channel = ProjectionChannel::new();
        let first = projection(7, "state-7");
        let second = advanced(&first, 8);
        push_ok(&mut channel, &first);
        push_ok(&mut channel, &second);
        channel
            .forward_ack(&ack_for(&second).to_string())
            .expect("the successor's ack becomes its one observation");

        for ack in [ack_for(&first), ack_for(&second)] {
            match channel.forward_ack(&ack.to_string()) {
                Ok(ForwardedAck::SupersededLate { .. }) => {}
                other => panic!("a superseded-late ack must construct nothing, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_malformed_ack_is_a_typed_rejection() {
        let mut channel = ProjectionChannel::new();
        let projection = projection(7, "state-7");
        push_ok(&mut channel, &projection);

        for (payload, why) in [
            ("not json".to_owned(), "unparseable payload"),
            (json!({ "viewport": {} }).to_string(), "no generation"),
            (
                {
                    let mut ack = ack_for(&projection);
                    ack.as_object_mut().unwrap().remove("viewport");
                    ack.to_string()
                },
                "no measured viewport",
            ),
            (
                {
                    let mut ack = ack_for(&projection);
                    ack["regions"].as_array_mut().unwrap().pop();
                    ack.to_string()
                },
                "a missing region",
            ),
            (
                {
                    let mut ack = ack_for(&projection);
                    ack["regions"][0]["id"] = Value::from("inventedRegion");
                    ack.to_string()
                },
                "an undeclared region id",
            ),
        ] {
            let error = channel
                .forward_ack(&payload)
                .expect_err("a malformed ack must be rejected");
            assert!(
                matches!(error, PaintedAckError::Malformed { .. }),
                "{why}: got {error:?}"
            );
        }
        // Every rejection left the pushed document awaiting its honest ack.
        assert_eq!(channel.in_flight_documents(), 1);
    }

    #[test]
    fn measured_evidence_violating_observation_invariants_is_typed() {
        let mut channel = ProjectionChannel::new();
        let projection = projection(7, "state-7");
        push_ok(&mut channel, &projection);

        // The footer's measured bounds run past the measured viewport: the
        // observation's own post-paint invariants reject the evidence.
        let mut ack = ack_for(&projection);
        ack["regions"][4]["widthPx"] = Value::from(2000.0);
        let error = channel
            .forward_ack(&ack.to_string())
            .expect_err("out-of-viewport evidence must be rejected");
        match error {
            PaintedAckError::InvalidObservation { generation, source } => {
                assert_eq!(generation, 7);
                assert_eq!(
                    source,
                    ShellFrameObservationError::InvalidRegionBounds {
                        region: ShellRegionId::Footer
                    }
                );
            }
            other => panic!("expected invalid observation evidence, got {other:?}"),
        }
    }
}
