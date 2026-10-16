use crate::kernel::midi_message::{MidiMessage, MidiMessageKind};
use crate::kernel::MidiChannel;
use core::any::Any;
use core::fmt;
use serde::{de::Error as _, Deserialize, Deserializer, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;

pub const MIDI_INPUT_PREFERENCE_VERSION: u32 = 1;
pub const MAX_MIDI_IDENTITY_SCHEMA_BYTES: usize = 32;
pub const MAX_MIDI_DEVICE_ID_BYTES: usize = 512;
pub const MAX_MIDI_DISPLAY_NAME_BYTES: usize = 256;
pub const MAX_MIDI_PORT_FACT_BYTES: usize = 128;
pub const PHYSICAL_MIDI_QUEUE_CAPACITY: usize = 1024;
pub const PHYSICAL_MIDI_DRAIN_BUDGET: usize = 64;

/// A validation or monotonic-identity failure at the canonical MIDI boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MidiDeviceContractError {
    EmptyField(&'static str),
    FieldTooLong {
        field: &'static str,
        maximum_bytes: usize,
    },
    ControlCharacter(&'static str),
    InvalidIdentitySchema,
    ZeroIdentifier(&'static str),
    IdentifierExhausted(&'static str),
    UnsupportedPreferenceVersion(u32),
}

impl fmt::Display for MidiDeviceContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyField(field) => write!(formatter, "{field} must not be empty"),
            Self::FieldTooLong {
                field,
                maximum_bytes,
            } => write!(
                formatter,
                "{field} exceeds its {maximum_bytes}-byte acceptance bound"
            ),
            Self::ControlCharacter(field) => {
                write!(formatter, "{field} must not contain control characters")
            }
            Self::InvalidIdentitySchema => formatter
                .write_str("MIDI identity schema must be lowercase ASCII kebab-case and versioned"),
            Self::ZeroIdentifier(kind) => write!(formatter, "{kind} must be nonzero"),
            Self::IdentifierExhausted(kind) => write!(formatter, "{kind} range is exhausted"),
            Self::UnsupportedPreferenceVersion(version) => write!(
                formatter,
                "MIDI input preference version {version} is unsupported"
            ),
        }
    }
}

impl std::error::Error for MidiDeviceContractError {}

fn validate_text(
    value: &str,
    field: &'static str,
    maximum_bytes: usize,
) -> Result<(), MidiDeviceContractError> {
    if value.is_empty() {
        return Err(MidiDeviceContractError::EmptyField(field));
    }
    if value.len() > maximum_bytes {
        return Err(MidiDeviceContractError::FieldTooLong {
            field,
            maximum_bytes,
        });
    }
    if value.chars().any(char::is_control) {
        return Err(MidiDeviceContractError::ControlCharacter(field));
    }
    Ok(())
}

fn validate_identity_schema(value: &str) -> Result<(), MidiDeviceContractError> {
    validate_text(
        value,
        "MIDI identity schema",
        MAX_MIDI_IDENTITY_SCHEMA_BYTES,
    )?;
    let mut segments = value.split('-');
    if segments.clone().count() < 2
        || segments.any(|segment| {
            segment.is_empty()
                || !segment
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
    {
        return Err(MidiDeviceContractError::InvalidIdentitySchema);
    }
    Ok(())
}

/// The one canonical, opaque, positively matched MIDI input identity.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MidiInputDeviceId {
    identity_schema: String,
    identity: String,
}

impl MidiInputDeviceId {
    pub const SERIALIZED_PROPERTY_DESCRIPTOR: &'static [&'static str] =
        &["identitySchema", "identity"];

    pub fn new(
        identity_schema: impl Into<String>,
        identity: impl Into<String>,
    ) -> Result<Self, MidiDeviceContractError> {
        let identity_schema = identity_schema.into();
        let identity = identity.into();
        validate_identity_schema(&identity_schema)?;
        validate_text(&identity, "MIDI input identity", MAX_MIDI_DEVICE_ID_BYTES)?;
        Ok(Self {
            identity_schema,
            identity,
        })
    }

    pub fn identity_schema(&self) -> &str {
        &self.identity_schema
    }

    pub fn identity(&self) -> &str {
        &self.identity
    }
}

impl fmt::Display for MidiInputDeviceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.identity_schema, self.identity)
    }
}

impl<'de> Deserialize<'de> for MidiInputDeviceId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Wire {
            identity_schema: String,
            identity: String,
        }

        let wire = Wire::deserialize(deserializer)?;
        Self::new(wire.identity_schema, wire.identity).map_err(D::Error::custom)
    }
}

/// Positively reported transport classification; absence means not reported.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MidiInputTransport {
    Usb,
    Virtual,
    Bluetooth,
    Network,
    BuiltIn,
    Other,
}

impl MidiInputTransport {
    pub const ALL: [Self; 6] = [
        Self::Usb,
        Self::Virtual,
        Self::Bluetooth,
        Self::Network,
        Self::BuiltIn,
        Self::Other,
    ];

    pub const fn surface_descriptor() -> &'static [Self] {
        &Self::ALL
    }
}

/// Optional stable facts reported by an input backend without name inference.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MidiInputPortFacts {
    transport: Option<MidiInputTransport>,
    manufacturer: Option<String>,
    product: Option<String>,
}

impl MidiInputPortFacts {
    pub const SERIALIZED_PROPERTY_DESCRIPTOR: &'static [&'static str] =
        &["transport", "manufacturer", "product"];

    pub fn new(
        transport: Option<MidiInputTransport>,
        manufacturer: Option<String>,
        product: Option<String>,
    ) -> Result<Self, MidiDeviceContractError> {
        if let Some(value) = manufacturer.as_deref() {
            validate_text(value, "MIDI manufacturer", MAX_MIDI_PORT_FACT_BYTES)?;
        }
        if let Some(value) = product.as_deref() {
            validate_text(value, "MIDI product", MAX_MIDI_PORT_FACT_BYTES)?;
        }
        Ok(Self {
            transport,
            manufacturer,
            product,
        })
    }

    pub const fn transport(&self) -> Option<MidiInputTransport> {
        self.transport
    }

    pub fn manufacturer(&self) -> Option<&str> {
        self.manufacturer.as_deref()
    }

    pub fn product(&self) -> Option<&str> {
        self.product.as_deref()
    }
}

impl<'de> Deserialize<'de> for MidiInputPortFacts {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Wire {
            transport: Option<MidiInputTransport>,
            manufacturer: Option<String>,
            product: Option<String>,
        }

        let wire = Wire::deserialize(deserializer)?;
        Self::new(wire.transport, wire.manufacturer, wire.product).map_err(D::Error::custom)
    }
}

/// Backend-neutral discovery data. It never owns a backend port handle.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MidiInputDescriptor {
    id: MidiInputDeviceId,
    display_name: String,
    port_facts: Option<MidiInputPortFacts>,
}

impl MidiInputDescriptor {
    pub const SERIALIZED_PROPERTY_DESCRIPTOR: &'static [&'static str] =
        &["id", "displayName", "portFacts"];

    pub fn new(
        id: MidiInputDeviceId,
        display_name: impl Into<String>,
        port_facts: Option<MidiInputPortFacts>,
    ) -> Result<Self, MidiDeviceContractError> {
        let display_name = display_name.into();
        validate_text(
            &display_name,
            "MIDI input display name",
            MAX_MIDI_DISPLAY_NAME_BYTES,
        )?;
        Ok(Self {
            id,
            display_name,
            port_facts,
        })
    }

    pub const fn id(&self) -> &MidiInputDeviceId {
        &self.id
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    pub const fn port_facts(&self) -> Option<&MidiInputPortFacts> {
        self.port_facts.as_ref()
    }
}

impl<'de> Deserialize<'de> for MidiInputDescriptor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Wire {
            id: MidiInputDeviceId,
            display_name: String,
            port_facts: Option<MidiInputPortFacts>,
        }

        let wire = Wire::deserialize(deserializer)?;
        Self::new(wire.id, wire.display_name, wire.port_facts).map_err(D::Error::custom)
    }
}

macro_rules! monotonic_id {
    ($name:ident, $kind:literal) => {
        #[derive(
            Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
        )]
        #[repr(transparent)]
        #[serde(transparent)]
        pub struct $name(u64);

        impl $name {
            pub const FIRST: Self = Self(1);

            pub const fn new(value: u64) -> Result<Self, MidiDeviceContractError> {
                if value == 0 {
                    Err(MidiDeviceContractError::ZeroIdentifier($kind))
                } else {
                    Ok(Self(value))
                }
            }

            pub const fn value(self) -> u64 {
                self.0
            }

            pub const fn checked_next(self) -> Result<Self, MidiDeviceContractError> {
                match self.0.checked_add(1) {
                    Some(value) => Ok(Self(value)),
                    None => Err(MidiDeviceContractError::IdentifierExhausted($kind)),
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

monotonic_id!(MidiInputScanId, "MIDI input scan id");
monotonic_id!(MidiConnectionRequestId, "MIDI connection request id");
monotonic_id!(MidiConnectionRevision, "MIDI connection revision");

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MidiConnectionFailureClass {
    PermissionDenied,
    Busy,
    BackendUnavailable,
    Rejected,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[repr(u8)]
#[serde(rename_all = "camelCase")]
pub enum MidiMessageDiagnosticClass {
    Empty,
    InvalidLength,
    InvalidData,
    PolyphonicPressure,
    SystemExclusive,
    SystemCommon,
    SystemRealtime,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MidiTransportCapacityStage {
    PhysicalIngress,
    AudioCommand,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MidiStaleOperation {
    Scan,
    Connect,
    Activate,
    Disconnect,
    Loss,
    Retirement,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MidiIdentifierKind {
    Scan,
    Request,
    Revision,
}

/// Stable adapter-neutral failure vocabulary. Backend error types never escape.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum MidiDeviceFailure {
    InitializationUnavailable,
    EnumerationFailed,
    PortInformationFailed {
        identity: Option<MidiInputDeviceId>,
    },
    IdentityUnavailable {
        identity: MidiInputDeviceId,
    },
    ConnectionFailed {
        identity: MidiInputDeviceId,
        class: MidiConnectionFailureClass,
    },
    DisconnectionFailed {
        identity: MidiInputDeviceId,
        revision: MidiConnectionRevision,
    },
    DeviceLost {
        identity: MidiInputDeviceId,
        revision: MidiConnectionRevision,
    },
    MalformedMessage {
        class: MidiMessageDiagnosticClass,
    },
    UnsupportedMessage {
        class: MidiMessageDiagnosticClass,
    },
    TransportCapacity {
        stage: MidiTransportCapacityStage,
        dropped: u64,
    },
    PreferenceReadFailed,
    PreferenceDecodeFailed,
    PreferenceVersionUnsupported,
    PreferenceWriteFailed,
    StaleCorrelation {
        operation: MidiStaleOperation,
    },
    IdentifierExhausted {
        identifier_kind: MidiIdentifierKind,
    },
}

impl MidiDeviceFailure {
    pub const SERIALIZED_KIND_DESCRIPTOR: &'static [&'static str] = &[
        "initializationUnavailable",
        "enumerationFailed",
        "portInformationFailed",
        "identityUnavailable",
        "connectionFailed",
        "disconnectionFailed",
        "deviceLost",
        "malformedMessage",
        "unsupportedMessage",
        "transportCapacity",
        "preferenceReadFailed",
        "preferenceDecodeFailed",
        "preferenceVersionUnsupported",
        "preferenceWriteFailed",
        "staleCorrelation",
        "identifierExhausted",
    ];
}

/// Reducer-owned connection status for one row/correlation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum MidiInputConnectionStatus {
    Available,
    Connecting {
        identity: MidiInputDeviceId,
        request_id: MidiConnectionRequestId,
        revision: MidiConnectionRevision,
    },
    Connected {
        identity: MidiInputDeviceId,
        revision: MidiConnectionRevision,
    },
    Unavailable {
        identity: MidiInputDeviceId,
    },
    Disconnected {
        identity: MidiInputDeviceId,
    },
    Failed {
        identity: Option<MidiInputDeviceId>,
        failure: MidiDeviceFailure,
    },
}

impl MidiInputConnectionStatus {
    pub const SERIALIZED_KIND_DESCRIPTOR: &'static [&'static str] = &[
        "available",
        "connecting",
        "connected",
        "unavailable",
        "disconnected",
        "failed",
    ];
}

/// Process-local automatic-reconnection policy for the selected input.
///
/// This is reducer-owned but deliberately absent from the persisted
/// preference: every process starts enabled, while an explicit Disconnect
/// suppresses reconnects for the remainder of that process.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MidiInputConnectionIntent {
    #[default]
    Enabled,
    ManuallyDisconnected,
}

/// Correlation for the one input currently acknowledged as eligible to
/// deliver physical messages. Backend ownership lives outside AppState.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MidiActiveInputIdentity {
    identity: MidiInputDeviceId,
    revision: MidiConnectionRevision,
}

impl MidiActiveInputIdentity {
    pub const fn new(identity: MidiInputDeviceId, revision: MidiConnectionRevision) -> Self {
        Self { identity, revision }
    }

    pub const fn identity(&self) -> &MidiInputDeviceId {
        &self.identity
    }

    pub const fn revision(&self) -> MidiConnectionRevision {
        self.revision
    }
}

/// Reducer-owned scan lifecycle. A failed scan retains the last successful
/// registry and correlation and therefore cannot imply device loss.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum MidiInputScanState {
    #[default]
    Idle,
    Scanning {
        scan_id: MidiInputScanId,
        last_successful_scan_id: Option<MidiInputScanId>,
    },
    Ready {
        scan_id: MidiInputScanId,
    },
    Failed {
        scan_id: MidiInputScanId,
        last_successful_scan_id: Option<MidiInputScanId>,
        failure: MidiDeviceFailure,
    },
}

impl MidiInputScanState {
    pub const fn in_flight(&self) -> bool {
        matches!(self, Self::Scanning { .. })
    }

    pub const fn current_scan_id(&self) -> Option<MidiInputScanId> {
        match self {
            Self::Scanning { scan_id, .. }
            | Self::Ready { scan_id }
            | Self::Failed { scan_id, .. } => Some(*scan_id),
            Self::Idle => None,
        }
    }

    pub const fn last_successful_scan_id(&self) -> Option<MidiInputScanId> {
        match self {
            Self::Idle => None,
            Self::Scanning {
                last_successful_scan_id,
                ..
            }
            | Self::Failed {
                last_successful_scan_id,
                ..
            } => *last_successful_scan_id,
            Self::Ready { scan_id } => Some(*scan_id),
        }
    }

    pub const fn is_stale(&self) -> bool {
        matches!(self, Self::Failed { .. })
    }
}

/// One stable registry row. A retained selected/focused tombstone keeps its
/// last known descriptor while `present` reports availability truthfully.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MidiInputRegistryEntry {
    descriptor: MidiInputDescriptor,
    present: bool,
}

impl MidiInputRegistryEntry {
    pub const fn new(descriptor: MidiInputDescriptor, present: bool) -> Self {
        Self {
            descriptor,
            present,
        }
    }

    pub const fn descriptor(&self) -> &MidiInputDescriptor {
        &self.descriptor
    }

    pub const fn present(&self) -> bool {
        self.present
    }
}

/// Structural marker paired with every visible status word. Renderers may
/// style these markers, but may not rely on color alone.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MidiInputStatusMarker {
    OpenCircle,
    ProgressRing,
    FilledCircle,
    SlashedCircle,
    StopSquare,
    ErrorDiamond,
}

/// The sole row-local operation admitted for the focused Settings row.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MidiInputRowAction {
    Connect,
    Disconnect,
    Retry,
}

/// Exhaustive derived presentation/interaction state for one registry row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MidiInputRowState {
    connection: MidiInputConnectionStatus,
    status_text: &'static str,
    marker: MidiInputStatusMarker,
    action: Option<MidiInputRowAction>,
}

impl MidiInputRowState {
    pub const fn new(
        connection: MidiInputConnectionStatus,
        status_text: &'static str,
        marker: MidiInputStatusMarker,
        action: Option<MidiInputRowAction>,
    ) -> Self {
        Self {
            connection,
            status_text,
            marker,
            action,
        }
    }

    pub const fn connection(&self) -> &MidiInputConnectionStatus {
        &self.connection
    }

    pub const fn status_text(&self) -> &'static str {
        self.status_text
    }

    pub const fn marker(&self) -> MidiInputStatusMarker {
        self.marker
    }

    pub const fn action(&self) -> Option<MidiInputRowAction> {
        self.action
    }
}

/// One exact connection command admitted by the reducer.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectMidiInput {
    identity: MidiInputDeviceId,
    request_id: MidiConnectionRequestId,
    revision: MidiConnectionRevision,
}

impl ConnectMidiInput {
    pub const SERIALIZED_PROPERTY_DESCRIPTOR: &'static [&'static str] =
        &["identity", "requestId", "revision"];

    pub const fn new(
        identity: MidiInputDeviceId,
        request_id: MidiConnectionRequestId,
        revision: MidiConnectionRevision,
    ) -> Self {
        Self {
            identity,
            request_id,
            revision,
        }
    }

    pub const fn identity(&self) -> &MidiInputDeviceId {
        &self.identity
    }

    pub const fn request_id(&self) -> MidiConnectionRequestId {
        self.request_id
    }

    pub const fn revision(&self) -> MidiConnectionRevision {
        self.revision
    }
}

/// Typed control-side work emitted only after a MIDI-device reducer event is
/// accepted. Opaque handles and queue endpoints never cross this boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MidiDeviceEffect {
    Scan {
        scan_id: MidiInputScanId,
    },
    Connect {
        request: ConnectMidiInput,
    },
    Activate {
        request: ConnectMidiInput,
    },
    Recover {
        identity: MidiInputDeviceId,
        revision: MidiConnectionRevision,
    },
    Retire {
        identity: MidiInputDeviceId,
        request_id: Option<MidiConnectionRequestId>,
        revision: MidiConnectionRevision,
    },
    CancelCandidate {
        request: ConnectMidiInput,
    },
    Persist {
        preference: MidiInputPreference,
    },
    Shutdown,
}

/// Complete reducer-owned physical MIDI configuration and lifecycle state.
///
/// It contains identity/correlation facts only. Backend clients, connections,
/// callback gates, queues, timestamps, and activity observations remain in
/// control/adapter runtime objects.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MidiInputState {
    selected: Option<MidiPreferredInput>,
    connection_intent: MidiInputConnectionIntent,
    requested: Option<ConnectMidiInput>,
    request_prepared: bool,
    active: Option<MidiActiveInputIdentity>,
    scan: MidiInputScanState,
    registry: Vec<MidiInputRegistryEntry>,
    operation_failure_identity: Option<MidiInputDeviceId>,
    operation_failure: Option<MidiDeviceFailure>,
    preference_failure: Option<MidiDeviceFailure>,
    preference_loaded: bool,
    last_scan_id: Option<MidiInputScanId>,
    last_request_id: Option<MidiConnectionRequestId>,
    last_revision: Option<MidiConnectionRevision>,
    shutting_down: bool,
}

impl MidiInputState {
    pub const fn selected(&self) -> Option<&MidiPreferredInput> {
        self.selected.as_ref()
    }

    pub const fn connection_intent(&self) -> MidiInputConnectionIntent {
        self.connection_intent
    }

    pub const fn requested(&self) -> Option<&ConnectMidiInput> {
        self.requested.as_ref()
    }

    pub const fn request_is_prepared(&self) -> bool {
        self.request_prepared
    }

    pub const fn active(&self) -> Option<&MidiActiveInputIdentity> {
        self.active.as_ref()
    }

    pub const fn scan(&self) -> &MidiInputScanState {
        &self.scan
    }

    pub fn registry(&self) -> &[MidiInputRegistryEntry] {
        &self.registry
    }

    pub const fn operation_failure(&self) -> Option<&MidiDeviceFailure> {
        self.operation_failure.as_ref()
    }

    pub const fn preference_failure(&self) -> Option<&MidiDeviceFailure> {
        self.preference_failure.as_ref()
    }

    pub const fn preference_loaded(&self) -> bool {
        self.preference_loaded
    }

    pub const fn shutting_down(&self) -> bool {
        self.shutting_down
    }

    pub fn entry(&self, identity: &MidiInputDeviceId) -> Option<&MidiInputRegistryEntry> {
        self.registry
            .iter()
            .find(|entry| entry.descriptor().id() == identity)
    }

    pub fn ordered_identities(&self) -> Vec<MidiInputDeviceId> {
        self.registry
            .iter()
            .map(|entry| entry.descriptor().id().clone())
            .collect()
    }

    /// Derives the exhaustive row status, structural marker, and sole action.
    pub fn row_state(&self, identity: &MidiInputDeviceId) -> Option<MidiInputRowState> {
        let entry = self.entry(identity)?;
        let busy = self.requested.is_some();
        if self
            .requested
            .as_ref()
            .is_some_and(|request| request.identity() == identity)
        {
            let request = self.requested.as_ref().expect("checked above");
            return Some(MidiInputRowState::new(
                MidiInputConnectionStatus::Connecting {
                    identity: identity.clone(),
                    request_id: request.request_id(),
                    revision: request.revision(),
                },
                "CONNECTING",
                MidiInputStatusMarker::ProgressRing,
                None,
            ));
        }
        if self
            .active
            .as_ref()
            .is_some_and(|active| active.identity() == identity)
        {
            let active = self.active.as_ref().expect("checked above");
            return Some(MidiInputRowState::new(
                MidiInputConnectionStatus::Connected {
                    identity: identity.clone(),
                    revision: active.revision(),
                },
                "CONNECTED",
                MidiInputStatusMarker::FilledCircle,
                (!busy).then_some(MidiInputRowAction::Disconnect),
            ));
        }
        let selected = self
            .selected
            .as_ref()
            .is_some_and(|selected| selected.device_id() == *identity);
        if !entry.present() {
            return Some(MidiInputRowState::new(
                MidiInputConnectionStatus::Unavailable {
                    identity: identity.clone(),
                },
                "UNAVAILABLE",
                MidiInputStatusMarker::SlashedCircle,
                None,
            ));
        }
        if self.operation_failure_identity.as_ref() == Some(identity) {
            let failure = self
                .operation_failure
                .clone()
                .expect("a failure identity is always paired with a failure");
            return Some(MidiInputRowState::new(
                MidiInputConnectionStatus::Failed {
                    identity: Some(identity.clone()),
                    failure,
                },
                "FAILED",
                MidiInputStatusMarker::ErrorDiamond,
                (entry.present() && !busy).then_some(MidiInputRowAction::Retry),
            ));
        }
        if selected && self.connection_intent == MidiInputConnectionIntent::ManuallyDisconnected {
            return Some(MidiInputRowState::new(
                MidiInputConnectionStatus::Disconnected {
                    identity: identity.clone(),
                },
                "DISCONNECTED",
                MidiInputStatusMarker::StopSquare,
                (!busy).then_some(MidiInputRowAction::Connect),
            ));
        }
        Some(MidiInputRowState::new(
            MidiInputConnectionStatus::Available,
            "AVAILABLE",
            MidiInputStatusMarker::OpenCircle,
            (!busy && entry.present()).then_some(MidiInputRowAction::Connect),
        ))
    }

    pub(crate) fn next_scan_id(&self) -> Result<MidiInputScanId, MidiDeviceContractError> {
        self.last_scan_id
            .map_or(Ok(MidiInputScanId::FIRST), MidiInputScanId::checked_next)
    }

    pub(crate) fn next_connection_correlation(
        &self,
    ) -> Result<(MidiConnectionRequestId, MidiConnectionRevision), MidiDeviceContractError> {
        let request_id = self.last_request_id.map_or(
            Ok(MidiConnectionRequestId::FIRST),
            MidiConnectionRequestId::checked_next,
        )?;
        let revision = self.last_revision.map_or(
            Ok(MidiConnectionRevision::FIRST),
            MidiConnectionRevision::checked_next,
        )?;
        Ok((request_id, revision))
    }

    pub(crate) fn set_last_scan_id(&mut self, scan_id: MidiInputScanId) {
        self.last_scan_id = Some(scan_id);
    }

    pub(crate) fn set_last_connection_correlation(
        &mut self,
        request_id: MidiConnectionRequestId,
        revision: MidiConnectionRevision,
    ) {
        self.last_request_id = Some(request_id);
        self.last_revision = Some(revision);
    }

    pub(crate) fn set_scan(&mut self, scan: MidiInputScanState) {
        self.scan = scan;
    }

    pub(crate) fn set_registry(&mut self, registry: Vec<MidiInputRegistryEntry>) {
        self.registry = registry;
    }

    pub(crate) fn set_selected(&mut self, selected: Option<MidiPreferredInput>) {
        self.selected = selected;
    }

    pub(crate) fn set_connection_intent(&mut self, intent: MidiInputConnectionIntent) {
        self.connection_intent = intent;
    }

    pub(crate) fn set_requested(&mut self, requested: Option<ConnectMidiInput>) {
        self.requested = requested;
        self.request_prepared = false;
    }

    pub(crate) fn set_request_prepared(&mut self, prepared: bool) {
        self.request_prepared = prepared;
    }

    pub(crate) fn set_active(&mut self, active: Option<MidiActiveInputIdentity>) {
        self.active = active;
    }

    pub(crate) fn set_operation_failure(
        &mut self,
        identity: Option<MidiInputDeviceId>,
        failure: Option<MidiDeviceFailure>,
    ) {
        debug_assert_eq!(identity.is_some(), failure.is_some());
        self.operation_failure_identity = identity;
        self.operation_failure = failure;
    }

    pub(crate) fn set_preference_failure(&mut self, failure: Option<MidiDeviceFailure>) {
        self.preference_failure = failure;
    }

    pub(crate) fn set_preference_loaded(&mut self, loaded: bool) {
        self.preference_loaded = loaded;
    }

    pub(crate) fn set_shutting_down(&mut self, shutting_down: bool) {
        self.shutting_down = shutting_down;
    }
}

/// Fixed-size callback-to-control event. It owns no allocation or handle.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[repr(C)]
#[serde(rename_all = "camelCase")]
pub struct PhysicalMidiEvent {
    revision: MidiConnectionRevision,
    timestamp_micros: u64,
    message: MidiMessage,
    raw: [u8; 3],
    raw_len: u8,
    control_receipt_micros: Option<u64>,
}

impl PhysicalMidiEvent {
    /// Creates a synthetic event without claiming an original wire packet or
    /// a control receipt time. Physical ingress retains both forms separately.
    pub const fn new(
        revision: MidiConnectionRevision,
        timestamp_micros: u64,
        message: MidiMessage,
    ) -> Self {
        Self {
            revision,
            timestamp_micros,
            message,
            raw: [0; 3],
            raw_len: 0,
            control_receipt_micros: None,
        }
    }

    fn from_raw(
        revision: MidiConnectionRevision,
        timestamp_micros: u64,
        raw: &[u8],
    ) -> Result<Self, (bool, MidiMessageDiagnosticClass)> {
        let message = normalize_raw_midi(raw)?;
        let (raw, raw_len) = match raw {
            [status, first] => ([*status, *first, 0], 2),
            [status, first, second] => ([*status, *first, *second], 3),
            _ => return Err((false, MidiMessageDiagnosticClass::InvalidLength)),
        };
        Ok(Self {
            raw,
            raw_len,
            ..Self::new(revision, timestamp_micros, message)
        })
    }

    pub const fn revision(self) -> MidiConnectionRevision {
        self.revision
    }

    /// Backend timestamp in microseconds. Its clock origin belongs to this
    /// connection revision; it is not an audio-frame or application-clock time.
    pub const fn timestamp_micros(self) -> u64 {
        self.timestamp_micros
    }

    pub const fn message(self) -> MidiMessage {
        self.message
    }

    /// Exact original channel packet, including status and unmodified data.
    /// Synthetic events created with `new` return an empty slice.
    pub fn raw_bytes(&self) -> &[u8] {
        &self.raw[..usize::from(self.raw_len)]
    }

    /// Application control-clock receipt time, populated only after the active
    /// connection revision has been accepted by the control-side drain.
    pub const fn control_receipt_micros(self) -> Option<u64> {
        self.control_receipt_micros
    }

    pub(crate) const fn received_on_control(mut self, now_micros: u64) -> Self {
        self.control_receipt_micros = Some(now_micros);
        self
    }
}

/// Clock/transport packets remain distinct from instrument channel messages.
/// The backend timestamp and connection revision share physical ingress order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysicalMidiTransportEvent {
    revision: MidiConnectionRevision,
    timestamp_micros: u64,
    raw: [u8; 3],
    len: u8,
}
impl PhysicalMidiTransportEvent {
    pub const fn revision(self) -> MidiConnectionRevision {
        self.revision
    }
    pub const fn timestamp_micros(self) -> u64 {
        self.timestamp_micros
    }
    pub fn raw_bytes(&self) -> &[u8] {
        &self.raw[..usize::from(self.len)]
    }
}

/// One bounded, ordered ingress preserves transport/performance interleaving.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhysicalMidiPacket {
    Performance(PhysicalMidiEvent),
    Transport(PhysicalMidiTransportEvent),
}
impl PhysicalMidiPacket {
    pub const fn revision(self) -> MidiConnectionRevision {
        match self {
            Self::Performance(event) => event.revision(),
            Self::Transport(event) => event.revision(),
        }
    }
    fn from_raw(
        revision: MidiConnectionRevision,
        timestamp_micros: u64,
        raw: &[u8],
    ) -> Result<Self, (bool, MidiMessageDiagnosticClass)> {
        let length = match raw.first() {
            Some(0xf8 | 0xfa | 0xfb | 0xfc) => 1,
            Some(0xf2) => 3,
            _ => {
                return PhysicalMidiEvent::from_raw(revision, timestamp_micros, raw)
                    .map(Self::Performance)
            }
        };
        if raw.len() != length {
            return Err((false, MidiMessageDiagnosticClass::InvalidLength));
        }
        if raw[1..].iter().any(|byte| *byte > 127) {
            return Err((false, MidiMessageDiagnosticClass::InvalidData));
        }
        let mut bytes = [0; 3];
        bytes[..length].copy_from_slice(raw);
        Ok(Self::Transport(PhysicalMidiTransportEvent {
            revision,
            timestamp_micros,
            raw: bytes,
            len: length as u8,
        }))
    }
}

/// Fixed diagnostic totals copied from callback atomics on the control side.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[repr(C)]
#[serde(rename_all = "camelCase")]
pub struct MidiCallbackDiagnostics {
    pub inactive: u64,
    pub malformed: u64,
    pub unsupported: u64,
    pub capacity_failures: u64,
    pub latest_class_code: u8,
}

/// Fixed latest-compatible activity observation; never reducer-owned state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[repr(C)]
#[serde(rename_all = "camelCase")]
pub struct MidiActivitySnapshot {
    revision: MidiConnectionRevision,
    accepted_count: u64,
    last_event: Option<PhysicalMidiEvent>,
    last_control_receipt_micros: u64,
    diagnostics: MidiCallbackDiagnostics,
    overflow_epoch: u64,
}

/// Presentation-only join of the latest compatible activity snapshot and
/// the control clock's 500 ms Receiving derivation. This value travels on a
/// latest-only observation transport; it is never serialized into AppState,
/// SavedSession, StateSnapshot, or StateTree.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MidiActivityObservation {
    snapshot: Option<MidiActivitySnapshot>,
    receiving: bool,
}

impl MidiActivityObservation {
    pub const fn new(snapshot: Option<MidiActivitySnapshot>, receiving: bool) -> Self {
        Self {
            snapshot,
            receiving,
        }
    }

    pub const fn snapshot(self) -> Option<MidiActivitySnapshot> {
        self.snapshot
    }

    pub const fn receiving(self) -> bool {
        self.receiving
    }
}

impl MidiActivitySnapshot {
    pub const fn new(
        revision: MidiConnectionRevision,
        accepted_count: u64,
        last_event: Option<PhysicalMidiEvent>,
        last_control_receipt_micros: u64,
        diagnostics: MidiCallbackDiagnostics,
        overflow_epoch: u64,
    ) -> Self {
        Self {
            revision,
            accepted_count,
            last_event,
            last_control_receipt_micros,
            diagnostics,
            overflow_epoch,
        }
    }

    pub const fn revision(self) -> MidiConnectionRevision {
        self.revision
    }

    pub const fn accepted_count(self) -> u64 {
        self.accepted_count
    }

    pub const fn last_event(self) -> Option<PhysicalMidiEvent> {
        self.last_event
    }

    pub const fn last_control_receipt_micros(self) -> u64 {
        self.last_control_receipt_micros
    }

    pub const fn diagnostics(self) -> MidiCallbackDiagnostics {
        self.diagnostics
    }

    pub const fn overflow_epoch(self) -> u64 {
        self.overflow_epoch
    }
}

struct MidiIngressShared {
    enabled: AtomicBool,
    inactive: AtomicU64,
    malformed: AtomicU64,
    unsupported: AtomicU64,
    capacity_failures: AtomicU64,
    latest_class_code: AtomicU8,
    overflow_epoch: AtomicU64,
}

impl Default for MidiIngressShared {
    fn default() -> Self {
        Self {
            enabled: AtomicBool::new(false),
            inactive: AtomicU64::new(0),
            malformed: AtomicU64::new(0),
            unsupported: AtomicU64::new(0),
            capacity_failures: AtomicU64::new(0),
            latest_class_code: AtomicU8::new(0),
            overflow_epoch: AtomicU64::new(0),
        }
    }
}

/// Fixed callback outcome used only by instrumentation and adapter tests.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhysicalMidiIngressOutcome {
    Accepted,
    Inactive,
    Malformed(MidiMessageDiagnosticClass),
    Unsupported(MidiMessageDiagnosticClass),
    CapacityFailure,
}

/// Control-owned consumer/gate for exactly one candidate connection.
pub struct PhysicalMidiIngressControl {
    revision: MidiConnectionRevision,
    consumer: rtrb::Consumer<PhysicalMidiPacket>,
    shared: Arc<MidiIngressShared>,
}

impl PhysicalMidiIngressControl {
    pub const fn revision(&self) -> MidiConnectionRevision {
        self.revision
    }

    pub fn enable(&self) {
        self.shared.enabled.store(true, Ordering::Release);
    }

    pub fn disable(&self) {
        self.shared.enabled.store(false, Ordering::Release);
    }

    pub fn is_enabled(&self) -> bool {
        self.shared.enabled.load(Ordering::Acquire)
    }

    /// Legacy channel-only consumer. Clock packets are skipped, never sent to DSP.
    pub fn try_pop(&mut self) -> Option<PhysicalMidiEvent> {
        while let Some(packet) = self.try_pop_packet() {
            if let PhysicalMidiPacket::Performance(event) = packet {
                return Some(event);
            }
        }
        None
    }
    pub fn try_pop_packet(&mut self) -> Option<PhysicalMidiPacket> {
        self.consumer.pop().ok()
    }

    pub fn queued(&self) -> usize {
        self.consumer.slots()
    }

    pub fn diagnostics(&self) -> MidiCallbackDiagnostics {
        MidiCallbackDiagnostics {
            inactive: self.shared.inactive.load(Ordering::Relaxed),
            malformed: self.shared.malformed.load(Ordering::Relaxed),
            unsupported: self.shared.unsupported.load(Ordering::Relaxed),
            capacity_failures: self.shared.capacity_failures.load(Ordering::Relaxed),
            latest_class_code: self.shared.latest_class_code.load(Ordering::Relaxed),
        }
    }

    pub fn overflow_epoch(&self) -> u64 {
        self.shared.overflow_epoch.load(Ordering::Acquire)
    }

    pub fn discard_all(&mut self) -> usize {
        let mut discarded = 0;
        while self.consumer.pop().is_ok() {
            discarded += 1;
        }
        discarded
    }
}

impl fmt::Debug for PhysicalMidiIngressControl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PhysicalMidiIngressControl")
            .field("revision", &self.revision)
            .field("enabled", &self.is_enabled())
            .finish_non_exhaustive()
    }
}

/// Opaque producer/gate ownership passed into one platform connection.
pub struct PhysicalMidiIngress {
    revision: MidiConnectionRevision,
    producer: Option<rtrb::Producer<PhysicalMidiPacket>>,
    shared: Arc<MidiIngressShared>,
    backend: Option<Box<dyn Any + Send>>,
}

impl PhysicalMidiIngress {
    /// Allocates one distinct disabled 1024-event SPSC ring off callback.
    pub fn bounded(revision: MidiConnectionRevision) -> (Self, PhysicalMidiIngressControl) {
        let (producer, consumer) = rtrb::RingBuffer::new(PHYSICAL_MIDI_QUEUE_CAPACITY);
        let shared = Arc::new(MidiIngressShared::default());
        (
            Self {
                revision,
                producer: Some(producer),
                shared: Arc::clone(&shared),
                backend: None,
            },
            PhysicalMidiIngressControl {
                revision,
                consumer,
                shared,
            },
        )
    }

    /// Test/capability-seam constructor for opaque ownership without a ring.
    pub fn from_backend<T: Any + Send>(revision: MidiConnectionRevision, backend: T) -> Self {
        Self {
            revision,
            producer: None,
            shared: Arc::new(MidiIngressShared::default()),
            backend: Some(Box::new(backend)),
        }
    }

    pub const fn revision(&self) -> MidiConnectionRevision {
        self.revision
    }

    pub fn into_backend<T: Any + Send>(self) -> Result<Box<T>, Self> {
        let Self {
            revision,
            producer,
            shared,
            backend,
        } = self;
        let Some(backend) = backend else {
            return Err(Self {
                revision,
                producer,
                shared,
                backend: None,
            });
        };
        backend.downcast::<T>().map_err(|backend| Self {
            revision,
            producer,
            shared,
            backend: Some(backend),
        })
    }

    /// Bounded callback entry: preclassify, parse at most three bytes,
    /// normalize, and attempt one nonblocking push.
    pub fn receive_raw(&mut self, timestamp_micros: u64, raw: &[u8]) -> PhysicalMidiIngressOutcome {
        if !self.shared.enabled.load(Ordering::Acquire) {
            self.shared.inactive.fetch_add(1, Ordering::Relaxed);
            return PhysicalMidiIngressOutcome::Inactive;
        }
        let Some(producer) = self.producer.as_mut() else {
            self.shared.inactive.fetch_add(1, Ordering::Relaxed);
            return PhysicalMidiIngressOutcome::Inactive;
        };
        let event = match PhysicalMidiPacket::from_raw(self.revision, timestamp_micros, raw) {
            Ok(event) => event,
            Err((false, class)) => {
                self.shared.malformed.fetch_add(1, Ordering::Relaxed);
                self.shared
                    .latest_class_code
                    .store(class as u8, Ordering::Relaxed);
                return PhysicalMidiIngressOutcome::Malformed(class);
            }
            Err((true, class)) => {
                self.shared.unsupported.fetch_add(1, Ordering::Relaxed);
                self.shared
                    .latest_class_code
                    .store(class as u8, Ordering::Relaxed);
                return PhysicalMidiIngressOutcome::Unsupported(class);
            }
        };
        match producer.push(event) {
            Ok(()) => PhysicalMidiIngressOutcome::Accepted,
            Err(rtrb::PushError::Full(_event)) => {
                self.shared
                    .capacity_failures
                    .fetch_add(1, Ordering::Relaxed);
                self.shared.overflow_epoch.fetch_add(1, Ordering::Release);
                self.shared.enabled.store(false, Ordering::Release);
                PhysicalMidiIngressOutcome::CapacityFailure
            }
        }
    }
}

fn normalize_raw_midi(raw: &[u8]) -> Result<MidiMessage, (bool, MidiMessageDiagnosticClass)> {
    let Some(status) = raw.first().copied() else {
        return Err((false, MidiMessageDiagnosticClass::Empty));
    };
    if status >= 0xf8 {
        return Err((true, MidiMessageDiagnosticClass::SystemRealtime));
    }
    if status >= 0xf0 {
        return Err((
            true,
            if status == 0xf0 {
                MidiMessageDiagnosticClass::SystemExclusive
            } else {
                MidiMessageDiagnosticClass::SystemCommon
            },
        ));
    }
    if status < 0x80 {
        return Err((false, MidiMessageDiagnosticClass::InvalidData));
    }
    let expected = if matches!(status >> 4, 0x0c | 0x0d) {
        2
    } else {
        3
    };
    if raw.len() != expected {
        return Err((false, MidiMessageDiagnosticClass::InvalidLength));
    }
    let event = midly::live::LiveEvent::parse(raw)
        .map_err(|_| (false, MidiMessageDiagnosticClass::InvalidData))?;
    let midly::live::LiveEvent::Midi { channel, message } = event else {
        return Err((true, MidiMessageDiagnosticClass::SystemCommon));
    };
    let channel = MidiChannel::new(channel.as_int())
        .map_err(|_| (false, MidiMessageDiagnosticClass::InvalidData))?;
    let (kind, data1, data2) = match message {
        midly::MidiMessage::NoteOff { key, vel } => {
            (MidiMessageKind::NoteOff, key.as_int(), vel.as_int())
        }
        midly::MidiMessage::NoteOn { key, vel } if vel.as_int() == 0 => {
            (MidiMessageKind::NoteOff, key.as_int(), 0)
        }
        midly::MidiMessage::NoteOn { key, vel } => {
            (MidiMessageKind::NoteOn, key.as_int(), vel.as_int())
        }
        midly::MidiMessage::Aftertouch { key, vel } => {
            (MidiMessageKind::PolyPressure, key.as_int(), vel.as_int())
        }
        midly::MidiMessage::Controller {
            controller,
            value: _,
        } if matches!(controller.as_int(), 120 | 123) => {
            return Ok(MidiMessage::all_notes_off(channel))
        }
        midly::MidiMessage::Controller { controller, value } => (
            MidiMessageKind::ControlChange,
            controller.as_int(),
            value.as_int(),
        ),
        midly::MidiMessage::ProgramChange { program } => {
            (MidiMessageKind::ProgramChange, program.as_int(), 0)
        }
        midly::MidiMessage::ChannelAftertouch { vel } => {
            (MidiMessageKind::ChannelPressure, vel.as_int(), 0)
        }
        midly::MidiMessage::PitchBend { .. } => (MidiMessageKind::PitchBend, raw[1], raw[2]),
    };
    MidiMessage::try_new(channel, kind, data1, data2)
        .map_err(|_| (false, MidiMessageDiagnosticClass::InvalidData))
}

impl fmt::Debug for PhysicalMidiIngress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PhysicalMidiIngress")
            .field("revision", &self.revision)
            .finish_non_exhaustive()
    }
}

/// Non-cloneable opaque ownership of one active backend connection.
pub struct ActiveMidiInput {
    identity: MidiInputDeviceId,
    request_id: MidiConnectionRequestId,
    revision: MidiConnectionRevision,
    backend: Box<dyn Any + Send>,
}

impl ActiveMidiInput {
    pub fn from_backend<T: Any + Send>(request: &ConnectMidiInput, backend: T) -> Self {
        Self {
            identity: request.identity.clone(),
            request_id: request.request_id,
            revision: request.revision,
            backend: Box::new(backend),
        }
    }

    pub const fn identity(&self) -> &MidiInputDeviceId {
        &self.identity
    }

    pub const fn request_id(&self) -> MidiConnectionRequestId {
        self.request_id
    }

    pub const fn revision(&self) -> MidiConnectionRevision {
        self.revision
    }

    pub fn into_backend<T: Any + Send>(self) -> Result<Box<T>, Self> {
        let Self {
            identity,
            request_id,
            revision,
            backend,
        } = self;
        backend.downcast::<T>().map_err(|backend| Self {
            identity,
            request_id,
            revision,
            backend,
        })
    }
}

impl fmt::Debug for ActiveMidiInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ActiveMidiInput")
            .field("identity", &self.identity)
            .field("request_id", &self.request_id)
            .field("revision", &self.revision)
            .finish_non_exhaustive()
    }
}

/// Minimal selected-input value nested in the separate preference document.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MidiPreferredInput {
    identity_schema: String,
    identity: String,
    last_known_display_name: String,
}

impl MidiPreferredInput {
    pub const SERIALIZED_PROPERTY_DESCRIPTOR: &'static [&'static str] =
        &["identitySchema", "identity", "lastKnownDisplayName"];

    pub fn new(
        identity: MidiInputDeviceId,
        last_known_display_name: impl Into<String>,
    ) -> Result<Self, MidiDeviceContractError> {
        let last_known_display_name = last_known_display_name.into();
        validate_text(
            &last_known_display_name,
            "MIDI preference display name",
            MAX_MIDI_DISPLAY_NAME_BYTES,
        )?;
        Ok(Self {
            identity_schema: identity.identity_schema,
            identity: identity.identity,
            last_known_display_name,
        })
    }

    pub fn device_id(&self) -> MidiInputDeviceId {
        MidiInputDeviceId {
            identity_schema: self.identity_schema.clone(),
            identity: self.identity.clone(),
        }
    }

    pub fn last_known_display_name(&self) -> &str {
        &self.last_known_display_name
    }
}

impl<'de> Deserialize<'de> for MidiPreferredInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Wire {
            identity_schema: String,
            identity: String,
            last_known_display_name: String,
        }

        let wire = Wire::deserialize(deserializer)?;
        let identity = MidiInputDeviceId::new(wire.identity_schema, wire.identity)
            .map_err(D::Error::custom)?;
        Self::new(identity, wire.last_known_display_name).map_err(D::Error::custom)
    }
}

/// Version-one preference document kept separate from `SavedSession`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MidiInputPreference {
    version: u32,
    selected_input: MidiPreferredInput,
}

impl MidiInputPreference {
    pub const SERIALIZED_PROPERTY_DESCRIPTOR: &'static [&'static str] =
        &["version", "selectedInput"];

    pub const fn new(selected_input: MidiPreferredInput) -> Self {
        Self {
            version: MIDI_INPUT_PREFERENCE_VERSION,
            selected_input,
        }
    }

    pub const fn version(&self) -> u32 {
        self.version
    }

    pub const fn selected_input(&self) -> &MidiPreferredInput {
        &self.selected_input
    }
}

impl<'de> Deserialize<'de> for MidiInputPreference {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Wire {
            version: u32,
            selected_input: MidiPreferredInput,
        }

        let wire = Wire::deserialize(deserializer)?;
        if wire.version != MIDI_INPUT_PREFERENCE_VERSION {
            return Err(D::Error::custom(
                MidiDeviceContractError::UnsupportedPreferenceVersion(wire.version),
            ));
        }
        Ok(Self::new(wire.selected_input))
    }
}

/// Object-safe capability for discovery, exact connection, and retirement.
pub trait MidiInputDevicePort: Send {
    fn enumerate(&mut self) -> Result<Vec<MidiInputDescriptor>, MidiDeviceFailure>;

    fn connect(
        &mut self,
        request: ConnectMidiInput,
        ingress: PhysicalMidiIngress,
    ) -> Result<ActiveMidiInput, MidiDeviceFailure>;

    fn disconnect(&mut self, active: ActiveMidiInput) -> Result<(), MidiDeviceFailure>;
}

/// Object-safe capability for the separate per-user MIDI preference document.
pub trait MidiInputPreferencePort: Send {
    fn load(&mut self) -> Result<Option<MidiInputPreference>, MidiDeviceFailure>;

    fn store(&mut self, value: &MidiInputPreference) -> Result<(), MidiDeviceFailure>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::midi_message::MidiMessageKind;
    use crate::kernel::MidiChannel;
    use core::mem::{needs_drop, size_of};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    fn id(value: &str) -> MidiInputDeviceId {
        MidiInputDeviceId::new("midir-v1", value).unwrap()
    }

    fn descriptor(value: &str) -> MidiInputDescriptor {
        MidiInputDescriptor::new(id(value), "Controller", None).unwrap()
    }

    #[test]
    fn bounded_contracts_validate_at_construction_and_deserialization() {
        assert!(MidiInputDeviceId::new("midir-v1", "opaque").is_ok());
        assert!(MidiInputDeviceId::new("MIDI", "opaque").is_err());
        assert!(MidiInputDeviceId::new("midir-v1", "").is_err());
        assert!(MidiInputDeviceId::new("midir-v1", "x".repeat(513)).is_err());
        assert!(MidiInputDescriptor::new(id("a"), "", None).is_err());
        assert!(MidiInputDescriptor::new(id("a"), "x".repeat(257), None).is_err());
        assert!(serde_json::from_str::<MidiInputDeviceId>(
            r#"{"identitySchema":"bad schema","identity":"a"}"#
        )
        .is_err());
    }

    #[test]
    fn monotonic_identifiers_are_nonzero_and_fail_before_wrap() {
        assert!(MidiInputScanId::new(0).is_err());
        assert!(MidiConnectionRequestId::new(0).is_err());
        assert!(MidiConnectionRevision::new(0).is_err());
        assert_eq!(MidiInputScanId::FIRST.checked_next().unwrap().value(), 2);
        assert!(MidiInputScanId::new(u64::MAX)
            .unwrap()
            .checked_next()
            .is_err());
        assert!(MidiConnectionRequestId::new(u64::MAX)
            .unwrap()
            .checked_next()
            .is_err());
        assert!(MidiConnectionRevision::new(u64::MAX)
            .unwrap()
            .checked_next()
            .is_err());
    }

    #[test]
    fn serialization_descriptors_match_exact_json_fields() {
        let device = descriptor("opaque-a");
        let value = serde_json::to_value(&device).unwrap();
        assert_eq!(
            value
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            ["displayName", "id", "portFacts"]
        );
        assert_eq!(
            MidiInputDeviceId::SERIALIZED_PROPERTY_DESCRIPTOR,
            ["identitySchema", "identity"]
        );
        assert_eq!(
            MidiInputDescriptor::SERIALIZED_PROPERTY_DESCRIPTOR,
            ["id", "displayName", "portFacts"]
        );
        assert_eq!(MidiDeviceFailure::SERIALIZED_KIND_DESCRIPTOR.len(), 16);
        assert_eq!(
            MidiInputConnectionStatus::SERIALIZED_KIND_DESCRIPTOR,
            [
                "available",
                "connecting",
                "connected",
                "unavailable",
                "disconnected",
                "failed"
            ]
        );
    }

    #[test]
    fn physical_event_and_callback_diagnostics_are_copy_and_destructor_free() {
        fn assert_copy<T: Copy>() {}
        assert_copy::<PhysicalMidiEvent>();
        assert_copy::<MidiCallbackDiagnostics>();
        assert_copy::<MidiActivitySnapshot>();
        assert!(!needs_drop::<PhysicalMidiEvent>());
        assert!(!needs_drop::<MidiCallbackDiagnostics>());
        assert!(!needs_drop::<MidiActivitySnapshot>());
        assert_eq!(size_of::<MidiMessage>(), 4);
        assert_eq!(size_of::<PhysicalMidiEvent>(), 40);
        assert!(size_of::<MidiCallbackDiagnostics>() <= 40);
        assert!(size_of::<MidiActivitySnapshot>() <= 120);
    }

    #[derive(Debug)]
    struct FakeBackendHandle(Arc<AtomicUsize>);

    impl Drop for FakeBackendHandle {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    struct FakeDevicePort {
        live: Arc<AtomicUsize>,
    }

    impl MidiInputDevicePort for FakeDevicePort {
        fn enumerate(&mut self) -> Result<Vec<MidiInputDescriptor>, MidiDeviceFailure> {
            Ok(vec![descriptor("opaque-a")])
        }

        fn connect(
            &mut self,
            request: ConnectMidiInput,
            ingress: PhysicalMidiIngress,
        ) -> Result<ActiveMidiInput, MidiDeviceFailure> {
            assert_eq!(ingress.revision(), request.revision());
            self.live.fetch_add(1, Ordering::SeqCst);
            Ok(ActiveMidiInput::from_backend(
                &request,
                FakeBackendHandle(self.live.clone()),
            ))
        }

        fn disconnect(&mut self, active: ActiveMidiInput) -> Result<(), MidiDeviceFailure> {
            let handle = active.into_backend::<FakeBackendHandle>().map_err(|_| {
                MidiDeviceFailure::StaleCorrelation {
                    operation: MidiStaleOperation::Retirement,
                }
            })?;
            drop(handle);
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakePreferencePort(Option<MidiInputPreference>);

    impl MidiInputPreferencePort for FakePreferencePort {
        fn load(&mut self) -> Result<Option<MidiInputPreference>, MidiDeviceFailure> {
            Ok(self.0.clone())
        }

        fn store(&mut self, value: &MidiInputPreference) -> Result<(), MidiDeviceFailure> {
            self.0 = Some(value.clone());
            Ok(())
        }
    }

    #[test]
    fn capability_ports_are_object_safe_and_consume_owned_handles() {
        let live = Arc::new(AtomicUsize::new(0));
        let mut device: Box<dyn MidiInputDevicePort> =
            Box::new(FakeDevicePort { live: live.clone() });
        let request = ConnectMidiInput::new(
            id("opaque-a"),
            MidiConnectionRequestId::FIRST,
            MidiConnectionRevision::FIRST,
        );
        let ingress = PhysicalMidiIngress::from_backend(request.revision(), ());
        assert_eq!(device.enumerate().unwrap().len(), 1);
        let active = device.connect(request, ingress).unwrap();
        assert_eq!(live.load(Ordering::SeqCst), 1);
        device.disconnect(active).unwrap();
        assert_eq!(live.load(Ordering::SeqCst), 0);

        let preferred = MidiPreferredInput::new(id("opaque-a"), "Controller").unwrap();
        let preference = MidiInputPreference::new(preferred);
        let mut store: Box<dyn MidiInputPreferencePort> = Box::new(FakePreferencePort::default());
        store.store(&preference).unwrap();
        assert_eq!(store.load().unwrap(), Some(preference));
    }

    #[test]
    fn bounded_ingress_normalizes_every_supported_channel_message_exactly() {
        let revision = MidiConnectionRevision::FIRST;
        let (mut ingress, mut control) = PhysicalMidiIngress::bounded(revision);
        control.enable();
        let cases = [
            (
                &[0x80, 0x00, 0x7f][..],
                MidiMessage::try_new(
                    MidiChannel::new(0).unwrap(),
                    MidiMessageKind::NoteOff,
                    0,
                    127,
                )
                .unwrap(),
            ),
            (
                &[0x91, 0x7f, 0x01][..],
                MidiMessage::try_new(
                    MidiChannel::new(1).unwrap(),
                    MidiMessageKind::NoteOn,
                    127,
                    1,
                )
                .unwrap(),
            ),
            (
                &[0x92, 0x3c, 0x00][..],
                MidiMessage::try_new(
                    MidiChannel::new(2).unwrap(),
                    MidiMessageKind::NoteOff,
                    60,
                    0,
                )
                .unwrap(),
            ),
            (
                &[0xa9, 60, 23][..],
                MidiMessage::try_new(
                    MidiChannel::new(9).unwrap(),
                    MidiMessageKind::PolyPressure,
                    60,
                    23,
                )
                .unwrap(),
            ),
            (
                &[0xb3, 0x01, 0x7f][..],
                MidiMessage::try_new(
                    MidiChannel::new(3).unwrap(),
                    MidiMessageKind::ControlChange,
                    1,
                    127,
                )
                .unwrap(),
            ),
            (
                &[0xc4, 0x7f][..],
                MidiMessage::try_new(
                    MidiChannel::new(4).unwrap(),
                    MidiMessageKind::ProgramChange,
                    127,
                    0,
                )
                .unwrap(),
            ),
            (
                &[0xd5, 0x00][..],
                MidiMessage::try_new(
                    MidiChannel::new(5).unwrap(),
                    MidiMessageKind::ChannelPressure,
                    0,
                    0,
                )
                .unwrap(),
            ),
            (
                &[0xe6, 0x00, 0x7f][..],
                MidiMessage::try_new(
                    MidiChannel::new(6).unwrap(),
                    MidiMessageKind::PitchBend,
                    0,
                    127,
                )
                .unwrap(),
            ),
            (
                &[0xb7, 120, 0][..],
                MidiMessage::all_notes_off(MidiChannel::new(7).unwrap()),
            ),
            (
                &[0xb8, 123, 127][..],
                MidiMessage::all_notes_off(MidiChannel::new(8).unwrap()),
            ),
        ];
        for (timestamp, (raw, expected)) in cases.into_iter().enumerate() {
            assert_eq!(
                ingress.receive_raw(timestamp as u64, raw),
                PhysicalMidiIngressOutcome::Accepted
            );
            let event = control.try_pop().unwrap();
            assert_eq!(event.revision(), revision);
            assert_eq!(event.timestamp_micros(), timestamp as u64);
            assert_eq!(event.message(), expected);
            assert_eq!(event.raw_bytes(), raw);
            assert_eq!(event.control_receipt_micros(), None);
        }
    }

    #[test]
    fn transport_and_performance_share_order_revision_and_capacity() {
        let (mut ingress, mut control) =
            PhysicalMidiIngress::bounded(MidiConnectionRevision::FIRST);
        control.enable();
        let packets: &[&[u8]] = &[
            &[0xfa],
            &[0xf8],
            &[0x90, 60, 99],
            &[0xf2, 127, 127],
            &[0xfb],
            &[0xfc],
        ];
        for (index, raw) in packets.iter().enumerate() {
            assert_eq!(
                ingress.receive_raw(index as u64, raw),
                PhysicalMidiIngressOutcome::Accepted
            );
        }
        for (index, raw) in packets.iter().enumerate() {
            let packet = control.try_pop_packet().unwrap();
            assert_eq!(packet.revision(), MidiConnectionRevision::FIRST);
            match packet {
                PhysicalMidiPacket::Performance(event) => {
                    assert_eq!(event.raw_bytes(), *raw);
                    assert_eq!(event.timestamp_micros(), index as u64);
                }
                PhysicalMidiPacket::Transport(event) => {
                    assert_eq!(event.raw_bytes(), *raw);
                    assert_eq!(event.timestamp_micros(), index as u64);
                }
            }
        }
        assert!(!needs_drop::<PhysicalMidiPacket>());
        assert!(matches!(
            ingress.receive_raw(8, &[0xf2, 128, 0]),
            PhysicalMidiIngressOutcome::Malformed(_)
        ));
        assert!(matches!(
            ingress.receive_raw(9, &[0xf8, 0]),
            PhysicalMidiIngressOutcome::Malformed(_)
        ));
        for _ in 0..PHYSICAL_MIDI_QUEUE_CAPACITY {
            assert_eq!(
                ingress.receive_raw(10, &[0xf8]),
                PhysicalMidiIngressOutcome::Accepted
            );
        }
        assert_eq!(
            ingress.receive_raw(11, &[0x80, 60, 0]),
            PhysicalMidiIngressOutcome::CapacityFailure
        );
        assert!(!control.is_enabled());
        assert_eq!(control.overflow_epoch(), 1);
    }

    #[test]
    fn ingress_preclassifies_malformed_unsupported_and_large_system_input() {
        let (mut ingress, control) = PhysicalMidiIngress::bounded(MidiConnectionRevision::FIRST);
        assert_eq!(
            ingress.receive_raw(0, &[0x90, 60, 100]),
            PhysicalMidiIngressOutcome::Inactive
        );
        control.enable();
        for (raw, class) in [
            (&[][..], MidiMessageDiagnosticClass::Empty),
            (&[0x40][..], MidiMessageDiagnosticClass::InvalidData),
            (&[0x90, 60][..], MidiMessageDiagnosticClass::InvalidLength),
            (
                &[0x90, 60, 0x80][..],
                MidiMessageDiagnosticClass::InvalidData,
            ),
        ] {
            assert_eq!(
                ingress.receive_raw(0, raw),
                PhysicalMidiIngressOutcome::Malformed(class)
            );
        }
        for (raw, class) in [
            (&[0xf1, 1][..], MidiMessageDiagnosticClass::SystemCommon),
            (&[0xfe][..], MidiMessageDiagnosticClass::SystemRealtime),
        ] {
            assert_eq!(
                ingress.receive_raw(0, raw),
                PhysicalMidiIngressOutcome::Unsupported(class)
            );
        }
        let mut large_sysex = vec![0; 1_000_000];
        large_sysex[0] = 0xf0;
        assert_eq!(
            ingress.receive_raw(0, &large_sysex),
            PhysicalMidiIngressOutcome::Unsupported(MidiMessageDiagnosticClass::SystemExclusive)
        );
        assert_eq!(control.queued(), 0);
        let diagnostics = control.diagnostics();
        assert_eq!(diagnostics.inactive, 1);
        assert_eq!(diagnostics.malformed, 4);
        assert_eq!(diagnostics.unsupported, 3);
    }

    #[test]
    fn ingress_capacity_failure_disables_without_overwrite_and_rings_are_distinct() {
        let (mut first, mut first_control) =
            PhysicalMidiIngress::bounded(MidiConnectionRevision::FIRST);
        let second_revision = MidiConnectionRevision::new(2).unwrap();
        let (mut second, mut second_control) = PhysicalMidiIngress::bounded(second_revision);
        first_control.enable();
        second_control.enable();
        assert_eq!(
            second.receive_raw(77, &[0x90, 64, 1]),
            PhysicalMidiIngressOutcome::Accepted
        );
        assert_eq!(first_control.queued(), 0);
        assert_eq!(second_control.queued(), 1);
        assert_eq!(
            second_control.try_pop().unwrap().revision(),
            second_revision
        );

        for timestamp in 0..PHYSICAL_MIDI_QUEUE_CAPACITY as u64 {
            assert_eq!(
                first.receive_raw(timestamp, &[0x90, 60, 1]),
                PhysicalMidiIngressOutcome::Accepted
            );
        }
        assert_eq!(first_control.queued(), PHYSICAL_MIDI_QUEUE_CAPACITY);
        assert_eq!(
            first.receive_raw(9999, &[0x80, 60, 0]),
            PhysicalMidiIngressOutcome::CapacityFailure
        );
        assert!(!first_control.is_enabled());
        assert_eq!(first_control.overflow_epoch(), 1);
        assert_eq!(first_control.diagnostics().capacity_failures, 1);
        for timestamp in 0..PHYSICAL_MIDI_QUEUE_CAPACITY as u64 {
            assert_eq!(
                first_control.try_pop().unwrap().timestamp_micros(),
                timestamp,
                "full push cannot overwrite queued order"
            );
        }
        assert_eq!(first_control.try_pop(), None);
    }

    #[test]
    fn activity_snapshot_contains_only_fixed_callback_safe_values() {
        let message = MidiMessage::try_new(
            MidiChannel::new(0).unwrap(),
            MidiMessageKind::NoteOn,
            60,
            100,
        )
        .unwrap();
        let event = PhysicalMidiEvent::new(MidiConnectionRevision::FIRST, 12, message);
        assert!(event.raw_bytes().is_empty());
        assert_eq!(event.control_receipt_micros(), None);
        assert!(!std::mem::needs_drop::<PhysicalMidiEvent>());
        let snapshot = MidiActivitySnapshot::new(
            MidiConnectionRevision::FIRST,
            1,
            Some(event),
            25,
            MidiCallbackDiagnostics::default(),
            0,
        );
        assert_eq!(snapshot.last_event(), Some(event));
        assert_eq!(snapshot.accepted_count(), 1);
    }
}
