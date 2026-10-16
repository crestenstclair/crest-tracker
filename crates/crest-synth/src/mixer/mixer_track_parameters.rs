use crate::mixer::bus_id::{BusId, MAX_BUS_RETURNS};
use core::fmt;
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Stable identity of one configurable mixer-track field.
///
/// The genuine fader controls are `Level`, `Pan`, `Mute`, and `Solo` (`MAIN`).
/// Sends are not named fields: they are one indexed array on
/// [`MixerTrackParameters`], addressed by `(MixerTrackId, BusId)`. This enum
/// therefore holds exactly the four `MAIN` variants; a destination-named
/// send variant here would close the open bus topology.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MixerTrackParameter {
    Level,
    Pan,
    Mute,
    Solo,
}

impl MixerTrackParameter {
    pub const MAIN: [Self; 4] = [Self::Level, Self::Pan, Self::Mute, Self::Solo];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Level => "levelDb",
            Self::Pan => "pan",
            Self::Mute => "mute",
            Self::Solo => "solo",
        }
    }

    pub const fn descriptor(self) -> &'static MixerTrackParameterDescriptor {
        match self {
            Self::Level => &MIXER_TRACK_SURFACE_DESCRIPTOR[0],
            Self::Pan => &MIXER_TRACK_SURFACE_DESCRIPTOR[1],
            Self::Mute => &MIXER_TRACK_SURFACE_DESCRIPTOR[2],
            Self::Solo => &MIXER_TRACK_SURFACE_DESCRIPTOR[3],
        }
    }
}

impl fmt::Display for MixerTrackParameter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MixerTrackParameterKind {
    Continuous,
    Toggle,
}

/// Bounds and edit steps shared by every indexed bus send.
///
/// All sends share this one descriptor; its values are copied exactly
/// from the retired per-name send descriptors so the generalization changes no
/// bound: 0.0..=1.0, default 0.0, fine 0.01, coarse 0.1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BusSendDescriptor {
    minimum: f32,
    maximum: f32,
    default: f32,
    fine_step: f32,
    coarse_step: f32,
}

impl BusSendDescriptor {
    pub const fn minimum(self) -> f32 {
        self.minimum
    }

    pub const fn maximum(self) -> f32 {
        self.maximum
    }

    pub const fn default(self) -> f32 {
        self.default
    }

    pub const fn fine_step(self) -> f32 {
        self.fine_step
    }

    pub const fn coarse_step(self) -> f32 {
        self.coarse_step
    }

    pub fn contains(self, value: f32) -> bool {
        value.is_finite() && (self.minimum..=self.maximum).contains(&value)
    }
}

/// The one shared descriptor for all indexed sends.
pub const BUS_SEND_DESCRIPTOR: BusSendDescriptor = BusSendDescriptor {
    minimum: 0.0,
    maximum: 1.0,
    default: 0.0,
    fine_step: 0.01,
    coarse_step: 0.1,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MixerTrackParameterDescriptor {
    parameter: MixerTrackParameter,
    label: &'static str,
    kind: MixerTrackParameterKind,
    minimum: f32,
    maximum: f32,
    default: f32,
    fine_step: f32,
    coarse_step: f32,
    unit: Option<&'static str>,
}

impl MixerTrackParameterDescriptor {
    const fn continuous(
        parameter: MixerTrackParameter,
        label: &'static str,
        range: (f32, f32),
        default: f32,
        steps: (f32, f32),
        unit: Option<&'static str>,
    ) -> Self {
        Self {
            parameter,
            label,
            kind: MixerTrackParameterKind::Continuous,
            minimum: range.0,
            maximum: range.1,
            default,
            fine_step: steps.0,
            coarse_step: steps.1,
            unit,
        }
    }

    const fn toggle(parameter: MixerTrackParameter, label: &'static str) -> Self {
        Self {
            parameter,
            label,
            kind: MixerTrackParameterKind::Toggle,
            minimum: 0.0,
            maximum: 1.0,
            default: 0.0,
            fine_step: 1.0,
            coarse_step: 1.0,
            unit: None,
        }
    }

    pub const fn parameter(self) -> MixerTrackParameter {
        self.parameter
    }

    pub const fn name(self) -> &'static str {
        self.parameter.name()
    }

    pub const fn label(self) -> &'static str {
        self.label
    }

    pub const fn kind(self) -> MixerTrackParameterKind {
        self.kind
    }

    pub const fn minimum(self) -> f32 {
        self.minimum
    }

    pub const fn maximum(self) -> f32 {
        self.maximum
    }

    pub const fn default(self) -> f32 {
        self.default
    }

    pub const fn fine_step(self) -> f32 {
        self.fine_step
    }

    pub const fn coarse_step(self) -> f32 {
        self.coarse_step
    }

    pub const fn unit(self) -> Option<&'static str> {
        self.unit
    }

    pub fn contains(self, value: f32) -> bool {
        value.is_finite() && (self.minimum..=self.maximum).contains(&value)
    }
}

const MIXER_TRACK_SURFACE_DESCRIPTOR: [MixerTrackParameterDescriptor; 4] = [
    MixerTrackParameterDescriptor::continuous(
        MixerTrackParameter::Level,
        "Level",
        (-60.0, 6.0),
        0.0,
        (1.0, 6.0),
        Some("dB"),
    ),
    MixerTrackParameterDescriptor::continuous(
        MixerTrackParameter::Pan,
        "Pan",
        (-1.0, 1.0),
        0.0,
        (0.01, 0.1),
        None,
    ),
    MixerTrackParameterDescriptor::toggle(MixerTrackParameter::Mute, "Mute"),
    MixerTrackParameterDescriptor::toggle(MixerTrackParameter::Solo, "Solo"),
];

/// Canonical scalar and toggle state owned by one persistent mixer track.
///
/// Sends are one indexed vector over the configured bus returns: a send is a level
/// pointed at a `BusId`, never a named field, so adding a registry entry to a
/// return changes no field of this value. All sends share
/// [`BUS_SEND_DESCRIPTOR`].
#[derive(Clone, Debug, PartialEq)]
pub struct MixerTrackParameters {
    level_db: f32,
    pan: f32,
    mute: bool,
    solo: bool,
    sends: Vec<f32>,
}

impl MixerTrackParameters {
    /// Creates one validated track value from the complete indexed send bank.
    pub fn from_values(
        level_db: f32,
        pan: f32,
        mute: bool,
        solo: bool,
        sends: impl Into<Vec<f32>>,
    ) -> Result<Self, MixerTrackParametersError> {
        validate(MixerTrackParameter::Level, level_db)?;
        validate(MixerTrackParameter::Pan, pan)?;
        let sends = sends.into();
        validate_send_count(sends.len())?;
        for (index, send) in sends.iter().enumerate() {
            let bus = BusId::new(index as u16).expect("validated send count fits BusId");
            validate_send(bus, *send)?;
        }
        Ok(Self {
            level_db,
            pan,
            mute,
            solo,
            sends,
        })
    }

    pub const fn surface_descriptor() -> &'static [MixerTrackParameterDescriptor] {
        &MIXER_TRACK_SURFACE_DESCRIPTOR
    }

    pub const fn level_db(&self) -> f32 {
        self.level_db
    }

    pub const fn pan(&self) -> f32 {
        self.pan
    }

    pub const fn mute(&self) -> bool {
        self.mute
    }

    pub const fn solo(&self) -> bool {
        self.solo
    }

    /// Returns the send level directed at one bus return.
    pub fn send(&self, bus: BusId) -> f32 {
        self.sends.get(bus.index()).copied().unwrap_or(0.0)
    }

    /// Returns every send level in ascending `BusId` order.
    pub fn sends(&self) -> &[f32] {
        &self.sends
    }

    /// Replaces one send level after validating it against the shared descriptor.
    ///
    /// This control-side edit grows storage when the destination is newly configured.
    pub fn with_send(mut self, bus: BusId, value: f32) -> Result<Self, MixerTrackParametersError> {
        validate_send(bus, value)?;
        if bus.index() >= self.sends.len() {
            self.sends
                .resize(bus.index() + 1, BUS_SEND_DESCRIPTOR.default());
        }
        self.sends[bus.index()] = value;
        Ok(self)
    }

    /// Shapes the control-side send bank for a prepared graph's return count.
    pub fn with_send_count(mut self, count: usize) -> Result<Self, MixerTrackParametersError> {
        validate_send_count(count)?;
        self.sends.resize(count, BUS_SEND_DESCRIPTOR.default());
        Ok(self)
    }

    pub const fn scalar_value(&self, parameter: MixerTrackParameter) -> Option<f32> {
        match parameter {
            MixerTrackParameter::Level => Some(self.level_db),
            MixerTrackParameter::Pan => Some(self.pan),
            MixerTrackParameter::Mute | MixerTrackParameter::Solo => None,
        }
    }

    pub const fn toggle_value(&self, parameter: MixerTrackParameter) -> Option<bool> {
        match parameter {
            MixerTrackParameter::Mute => Some(self.mute),
            MixerTrackParameter::Solo => Some(self.solo),
            MixerTrackParameter::Level | MixerTrackParameter::Pan => None,
        }
    }

    pub fn with_scalar_value(
        mut self,
        parameter: MixerTrackParameter,
        value: f32,
    ) -> Result<Self, MixerTrackParametersError> {
        if parameter.descriptor().kind() != MixerTrackParameterKind::Continuous {
            return Err(MixerTrackParametersError::WrongValueKind { parameter });
        }
        validate(parameter, value)?;
        match parameter {
            MixerTrackParameter::Level => self.level_db = value,
            MixerTrackParameter::Pan => self.pan = value,
            MixerTrackParameter::Mute | MixerTrackParameter::Solo => unreachable!(),
        }
        Ok(self)
    }

    pub fn toggled(
        mut self,
        parameter: MixerTrackParameter,
    ) -> Result<Self, MixerTrackParametersError> {
        match parameter {
            MixerTrackParameter::Mute => self.mute = !self.mute,
            MixerTrackParameter::Solo => self.solo = !self.solo,
            MixerTrackParameter::Level | MixerTrackParameter::Pan => {
                return Err(MixerTrackParametersError::WrongValueKind { parameter });
            }
        }
        Ok(self)
    }
}

impl Default for MixerTrackParameters {
    fn default() -> Self {
        Self {
            level_db: 0.0,
            pan: 0.0,
            mute: false,
            solo: false,
            sends: vec![BUS_SEND_DESCRIPTOR.default(); MAX_BUS_RETURNS],
        }
    }
}

/// The serialized send surface is the one indexed `sends` array, never a key
/// per destination. It must stay in step with the
/// `ParameterSnapshot`/`StateTree` `SERIALIZED_LEAF_DESCRIPTOR` tables: the
/// shape and its declarations move in one change. These keys have no
/// persisted documents and no external consumers.
impl Serialize for MixerTrackParameters {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("MixerTrackParameters", 5)?;
        state.serialize_field("levelDb", &self.level_db)?;
        state.serialize_field("pan", &self.pan)?;
        state.serialize_field("mute", &self.mute)?;
        state.serialize_field("solo", &self.solo)?;
        state.serialize_field("sends", &self.sends)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for MixerTrackParameters {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Value {
            level_db: f32,
            pan: f32,
            mute: bool,
            solo: bool,
            sends: Vec<f32>,
        }

        // Deliberately routes through `from_values` so validation cannot be
        // bypassed.
        let value = Value::deserialize(deserializer)?;
        Self::from_values(
            value.level_db,
            value.pan,
            value.mute,
            value.solo,
            value.sends,
        )
        .map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MixerTrackParametersError {
    NonFinite {
        parameter: MixerTrackParameter,
    },
    OutOfRange {
        parameter: MixerTrackParameter,
        value: f32,
    },
    WrongValueKind {
        parameter: MixerTrackParameter,
    },
    NonFiniteSend {
        bus: BusId,
    },
    OutOfRangeSend {
        bus: BusId,
        value: f32,
    },
    SendCountOutOfRange {
        count: usize,
    },
}

impl fmt::Display for MixerTrackParametersError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::NonFinite { parameter } => write!(formatter, "{parameter} must be finite"),
            Self::OutOfRange { parameter, value } => {
                let descriptor = parameter.descriptor();
                write!(
                    formatter,
                    "{parameter} must be in {}..={}, got {value}",
                    descriptor.minimum(),
                    descriptor.maximum()
                )
            }
            Self::WrongValueKind { parameter } => {
                write!(formatter, "{parameter} does not accept that value kind")
            }
            Self::NonFiniteSend { bus } => {
                write!(formatter, "send to {bus} must be finite")
            }
            Self::OutOfRangeSend { bus, value } => write!(
                formatter,
                "send to {bus} must be in {}..={}, got {value}",
                BUS_SEND_DESCRIPTOR.minimum(),
                BUS_SEND_DESCRIPTOR.maximum()
            ),
            Self::SendCountOutOfRange { count } => write!(
                formatter,
                "send count must fit the bus identity range, got {count}"
            ),
        }
    }
}

impl std::error::Error for MixerTrackParametersError {}

fn validate(parameter: MixerTrackParameter, value: f32) -> Result<(), MixerTrackParametersError> {
    if !value.is_finite() {
        return Err(MixerTrackParametersError::NonFinite { parameter });
    }
    if !parameter.descriptor().contains(value) {
        return Err(MixerTrackParametersError::OutOfRange { parameter, value });
    }
    Ok(())
}

fn validate_send(bus: BusId, value: f32) -> Result<(), MixerTrackParametersError> {
    if !value.is_finite() {
        return Err(MixerTrackParametersError::NonFiniteSend { bus });
    }
    if !BUS_SEND_DESCRIPTOR.contains(value) {
        return Err(MixerTrackParametersError::OutOfRangeSend { bus, value });
    }
    Ok(())
}

fn validate_send_count(count: usize) -> Result<(), MixerTrackParametersError> {
    if count > usize::from(u16::MAX) + 1 {
        return Err(MixerTrackParametersError::SendCountOutOfRange { count });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        MixerTrackParameter, MixerTrackParameterKind, MixerTrackParameters,
        MixerTrackParametersError, BUS_SEND_DESCRIPTOR,
    };
    use crate::mixer::bus_id::{BusId, MAX_BUS_RETURNS};

    #[test]
    fn descriptor_has_exactly_the_four_main_fields() {
        let parameters = MixerTrackParameters::surface_descriptor()
            .iter()
            .map(|descriptor| descriptor.parameter())
            .collect::<Vec<_>>();
        assert_eq!(parameters, MixerTrackParameter::MAIN);
        assert_eq!(
            MixerTrackParameter::Mute.descriptor().kind(),
            MixerTrackParameterKind::Toggle
        );
    }

    #[test]
    fn all_sends_share_the_one_bus_send_descriptor_exactly() {
        assert_eq!(BUS_SEND_DESCRIPTOR.minimum(), 0.0);
        assert_eq!(BUS_SEND_DESCRIPTOR.maximum(), 1.0);
        assert_eq!(BUS_SEND_DESCRIPTOR.default(), 0.0);
        assert_eq!(BUS_SEND_DESCRIPTOR.fine_step(), 0.01);
        assert_eq!(BUS_SEND_DESCRIPTOR.coarse_step(), 0.1);
    }

    #[test]
    fn every_continuous_inclusive_boundary_is_valid() {
        let minimums =
            MixerTrackParameters::from_values(-60.0, -1.0, false, false, [0.0; MAX_BUS_RETURNS])
                .unwrap();
        let maximums =
            MixerTrackParameters::from_values(6.0, 1.0, true, true, [1.0; MAX_BUS_RETURNS])
                .unwrap();
        assert_eq!(minimums.level_db(), -60.0);
        assert_eq!(maximums.level_db(), 6.0);
        assert!(maximums.mute());
        assert!(maximums.solo());
    }

    #[test]
    fn each_invalid_numeric_class_is_rejected() {
        assert!(matches!(
            MixerTrackParameters::from_values(f32::NAN, 0.0, false, false, [0.0; MAX_BUS_RETURNS]),
            Err(MixerTrackParametersError::NonFinite {
                parameter: MixerTrackParameter::Level
            })
        ));
        for (parameter, value) in [
            (MixerTrackParameter::Level, 6.1),
            (MixerTrackParameter::Pan, -1.1),
        ] {
            assert!(matches!(
                MixerTrackParameters::default().with_scalar_value(parameter, value),
                Err(MixerTrackParametersError::OutOfRange { .. })
            ));
        }
        for (bus, value) in [
            (BusId::new(0).unwrap(), 1.1),
            (BusId::new(1).unwrap(), -0.1),
        ] {
            assert!(matches!(
                MixerTrackParameters::default().with_send(bus, value),
                Err(MixerTrackParametersError::OutOfRangeSend { .. })
            ));
        }
    }

    #[test]
    fn every_default_send_validates_its_range() {
        let parameters = MixerTrackParameters::default();
        for bus in BusId::ALL {
            let raised = parameters.clone().with_send(bus, 1.0).unwrap();
            assert_eq!(raised.send(bus), 1.0);
            for other in BusId::ALL {
                if other != bus {
                    assert_eq!(raised.send(other), 0.0);
                }
            }
            assert_eq!(
                parameters.clone().with_send(bus, 1.1),
                Err(MixerTrackParametersError::OutOfRangeSend { bus, value: 1.1 })
            );
            assert_eq!(
                parameters.clone().with_send(bus, -0.1),
                Err(MixerTrackParametersError::OutOfRangeSend { bus, value: -0.1 })
            );
            assert_eq!(
                parameters.clone().with_send(bus, f32::NAN),
                Err(MixerTrackParametersError::NonFiniteSend { bus })
            );
        }
    }

    #[test]
    fn from_values_validates_the_complete_send_bank() {
        let mut sends = [0.5; MAX_BUS_RETURNS];
        assert!(MixerTrackParameters::from_values(0.0, 0.0, false, false, sends).is_ok());
        sends[7] = 1.5;
        assert_eq!(
            MixerTrackParameters::from_values(0.0, 0.0, false, false, sends),
            Err(MixerTrackParametersError::OutOfRangeSend {
                bus: BusId::new(7).unwrap(),
                value: 1.5
            })
        );
    }

    #[test]
    fn scalar_adjustment_and_toggle_kinds_are_disjoint() {
        let parameters = MixerTrackParameters::default()
            .with_scalar_value(MixerTrackParameter::Pan, 0.5)
            .unwrap()
            .toggled(MixerTrackParameter::Mute)
            .unwrap();
        assert_eq!(parameters.pan(), 0.5);
        assert!(parameters.mute());
        assert!(parameters
            .clone()
            .with_scalar_value(MixerTrackParameter::Solo, 1.0)
            .is_err());
        assert!(parameters.toggled(MixerTrackParameter::Level).is_err());
    }

    #[test]
    fn serialized_shape_retains_declared_leaves_and_configured_count() {
        // The pinned shape is the one indexed sends array, matching the
        // SERIALIZED_LEAF_DESCRIPTOR tables.
        let json = serde_json::to_string(&MixerTrackParameters::default()).unwrap();
        assert_eq!(
            json,
            r#"{"levelDb":0.0,"pan":0.0,"mute":false,"solo":false,"sends":[0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0]}"#
        );
        let round_tripped = serde_json::from_str::<MixerTrackParameters>(&json).unwrap();
        assert_eq!(round_tripped, MixerTrackParameters::default());
    }

    #[test]
    fn legacy_eight_send_documents_remain_valid_and_missing_sends_are_silent() {
        let parameters = serde_json::from_str::<MixerTrackParameters>(
            r#"{"levelDb":0.0,"pan":0.0,"mute":false,"solo":false,"sends":[0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.5]}"#,
        )
        .unwrap();
        assert_eq!(parameters.sends().len(), 8);
        assert_eq!(parameters.send(BusId::new(7).unwrap()), 0.5);
        assert_eq!(parameters.send(BusId::new(15).unwrap()), 0.0);
    }

    #[test]
    fn control_side_edits_support_more_than_sixteen_sends() {
        let bus = BusId::new(31).unwrap();
        let parameters = MixerTrackParameters::default();
        assert_eq!(parameters.sends().len(), 16);
        assert_eq!(parameters.send(bus), 0.0);
        let extended = parameters.with_send(bus, 0.75).unwrap();
        assert_eq!(extended.sends().len(), 32);
        assert_eq!(extended.send(bus), 0.75);
        assert!(extended.sends()[..31].iter().all(|level| *level == 0.0));
        let expanded = extended.with_send_count(48).unwrap();
        assert_eq!(expanded.send(bus), 0.75);
        assert!(expanded.sends()[32..].iter().all(|level| *level == 0.0));
        let narrowed = expanded.with_send_count(16).unwrap();
        assert_eq!(narrowed.sends().len(), 16);
        assert_eq!(narrowed.send(bus), 0.0);
    }

    #[test]
    fn send_bank_limit_is_only_the_bus_identity_representable_range() {
        let largest_count = usize::from(u16::MAX) + 1;
        let largest_bus = BusId::new(u16::MAX).unwrap();
        let parameters =
            MixerTrackParameters::from_values(0.0, 0.0, false, false, vec![0.0; largest_count])
                .unwrap()
                .with_send(largest_bus, 1.0)
                .unwrap();
        assert_eq!(parameters.send(largest_bus), 1.0);
        assert_eq!(
            parameters.with_send_count(largest_count + 1),
            Err(MixerTrackParametersError::SendCountOutOfRange {
                count: largest_count + 1
            })
        );
        assert_eq!(
            MixerTrackParameters::from_values(0.0, 0.0, false, false, vec![0.0; largest_count + 1]),
            Err(MixerTrackParametersError::SendCountOutOfRange {
                count: largest_count + 1
            })
        );
    }

    #[test]
    fn serde_cannot_bypass_numeric_validation() {
        assert!(serde_json::from_str::<MixerTrackParameters>(
            r#"{"levelDb":0.0,"pan":2.0,"mute":false,"solo":false,"sends":[0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0]}"#
        )
        .is_err());
        assert!(serde_json::from_str::<MixerTrackParameters>(
            r#"{"levelDb":0.0,"pan":0.0,"mute":false,"solo":false,"sends":[1.5,0.0,0.0,0.0,0.0,0.0,0.0,0.0]}"#
        )
        .is_err());
        assert!(serde_json::from_str::<MixerTrackParameters>(
            r#"{"levelDb":0.0,"pan":0.0,"mute":false,"solo":false,"sends":[0.0,0.0,0.0,0.0,0.0,0.0,0.0,-0.5]}"#
        )
        .is_err());
    }
}
