use core::fmt;
use serde::{Deserialize, Serialize};

/// Identifies one editable value in the shared global mix surface.
///
/// `MasterGainDb` is the only global value: it is a property of the master
/// stage rather than of any effect, and it is the single documented exception
/// to the no-name-enumeration invariant. Effect-owned values are never global
/// rows: they belong to the per-return MIXER rows — `ReturnOccupancy`,
/// `ReturnLevel`, and the occupying registry entry's descriptor scalars —
/// addressed by `BusId` on `MixerControlId`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum GlobalParameter {
    MasterGainDb,
}

impl GlobalParameter {
    /// Returns the stable serialized and projected field name.
    pub const fn name(self) -> &'static str {
        match self {
            Self::MasterGainDb => "masterGainDb",
        }
    }

    /// Returns this field's production-owned bounds and edit steps.
    pub const fn descriptor(self) -> &'static GlobalParameterDescriptor {
        match self {
            Self::MasterGainDb => &GLOBAL_PARAMETER_SURFACE_DESCRIPTOR[0],
        }
    }
}

impl fmt::Display for GlobalParameter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// The independent pre-dispatch oracle for one global parameter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlobalParameterDescriptor {
    parameter: GlobalParameter,
    label: &'static str,
    unit: &'static str,
    minimum: f32,
    maximum: f32,
    fine_step: f32,
    coarse_step: f32,
}

impl GlobalParameterDescriptor {
    const fn new(
        parameter: GlobalParameter,
        label: &'static str,
        unit: &'static str,
        minimum: f32,
        maximum: f32,
        fine_step: f32,
        coarse_step: f32,
    ) -> Self {
        Self {
            parameter,
            label,
            unit,
            minimum,
            maximum,
            fine_step,
            coarse_step,
        }
    }

    pub const fn parameter(&self) -> GlobalParameter {
        self.parameter
    }

    /// Returns the stable serialized field name.
    ///
    /// This is a **serialization key**, not a screen label. It names the value
    /// in the state tree, the parameter snapshot, and the diagnostic text; it
    /// is read by tools, not by players. Projecting it as a control label is a
    /// defect rather than a naming choice — see [`Self::label`], which is the
    /// only thing a surface may put on screen.
    pub const fn name(&self) -> &'static str {
        self.parameter.name()
    }

    /// Returns the authored on-screen label.
    ///
    /// Every other descriptor in this system carries one beside its bounds —
    /// the envelope rows, the Patch output rows, the mixer track rows, the
    /// voice limit. This one did not, which is precisely how `masterGainDb`
    /// reached the screen: with no authored label to project, both surfaces
    /// that host the row reached for [`Self::name`].
    pub const fn label(&self) -> &'static str {
        self.label
    }

    pub const fn unit(&self) -> &'static str {
        self.unit
    }

    pub const fn minimum(&self) -> f32 {
        self.minimum
    }

    pub const fn maximum(&self) -> f32 {
        self.maximum
    }

    pub const fn fine_step(&self) -> f32 {
        self.fine_step
    }

    pub const fn coarse_step(&self) -> f32 {
        self.coarse_step
    }

    pub fn contains(&self, value: f32) -> bool {
        value.is_finite() && (self.minimum..=self.maximum).contains(&value)
    }
}

const GLOBAL_PARAMETER_SURFACE_DESCRIPTOR: [GlobalParameterDescriptor; 1] =
    [GlobalParameterDescriptor::new(
        GlobalParameter::MasterGainDb,
        "Master Volume",
        "dB",
        -60.0,
        6.0,
        1.0,
        6.0,
    )];

/// A violation of the one bounded global mixer parameter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GlobalParametersError {
    /// masterGainDb is not finite or is outside -60.0..=6.0.
    InvalidMasterGainDb,
}

impl fmt::Display for GlobalParametersError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidMasterGainDb => "master gain must be finite and in -60.0..=6.0 dB",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for GlobalParametersError {}

/// All editable parameters shared by the complete mix.
///
/// Only master gain is global. The retired reverb and delay fields live on
/// the canonical `BusReturnBank` — descriptor scalars on the occupying
/// registry effect, plus the return-owned level.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlobalParameters {
    master_gain_db: f32,
}

impl GlobalParameters {
    /// Creates the complete, validated global mixer parameter value.
    pub fn new(master_gain_db: f32) -> Result<Self, GlobalParametersError> {
        if !GlobalParameter::MasterGainDb
            .descriptor()
            .contains(master_gain_db)
        {
            return Err(GlobalParametersError::InvalidMasterGainDb);
        }
        Ok(Self { master_gain_db })
    }

    /// Returns each editable MIXER global row exactly once in canonical
    /// projection order: master gain alone.
    pub const fn surface_descriptor() -> &'static [GlobalParameterDescriptor] {
        &GLOBAL_PARAMETER_SURFACE_DESCRIPTOR
    }

    /// Replaces master gain after validating it against the shared descriptor.
    pub fn with_master_gain_db(self, value: f32) -> Result<Self, GlobalParametersError> {
        Self::new(value)
    }

    /// Returns the final level applied after the return mix.
    pub const fn master_gain_db(&self) -> f32 {
        self.master_gain_db
    }
}

#[cfg(test)]
mod tests {
    use super::{
        GlobalParameter, GlobalParameterDescriptor, GlobalParameters, GlobalParametersError,
    };

    #[test]
    fn surface_descriptor_is_master_gain_alone() {
        let descriptor = GlobalParameters::surface_descriptor();

        assert_eq!(
            descriptor,
            &[GlobalParameterDescriptor::new(
                GlobalParameter::MasterGainDb,
                "Master Volume",
                "dB",
                -60.0,
                6.0,
                1.0,
                6.0,
            )]
        );
        assert_eq!(descriptor.len(), 1);
        assert_eq!(descriptor[0].name(), "masterGainDb");
        // The two vocabularies are for two readers and must never be the same
        // string: `name` addresses the value in the state tree and the
        // parameter snapshot, `label` is the only one a surface may show.
        assert_eq!(descriptor[0].label(), "Master Volume");
        assert_eq!(descriptor[0].unit(), "dB");
        assert_ne!(descriptor[0].label(), descriptor[0].name());
        assert!(descriptor[0].fine_step() > 0.0);
        assert!(descriptor[0].coarse_step() >= descriptor[0].fine_step());
    }

    #[test]
    fn only_master_gain_carries_global_storage() {
        let parameters = GlobalParameters::new(-3.0).unwrap();
        assert_eq!(parameters.master_gain_db(), -3.0);
        assert_eq!(
            parameters
                .with_master_gain_db(6.0)
                .unwrap()
                .master_gain_db(),
            6.0
        );
        assert_eq!(
            GlobalParameters::new(-60.1),
            Err(GlobalParametersError::InvalidMasterGainDb)
        );
        assert_eq!(
            GlobalParameters::new(f32::NAN),
            Err(GlobalParametersError::InvalidMasterGainDb)
        );
        assert!(GlobalParameters::new(-60.0).is_ok());
        assert!(GlobalParameters::new(6.0).is_ok());
    }

    #[test]
    fn errors_explain_the_rejected_bound() {
        assert_eq!(
            GlobalParametersError::InvalidMasterGainDb.to_string(),
            "master gain must be finite and in -60.0..=6.0 dB"
        );
    }
}
