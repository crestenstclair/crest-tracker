use crate::synth::instrument_capability::{ParameterKind, VoicePolicy};
use core::fmt;
use serde::Serialize;

/// Initial budget for legacy EngineManaged providers. New production providers
/// declare configurable defaults; this is not a product maximum.
pub const ENGINE_MANAGED_POLYPHONY_CEILING: u16 = 64;

/// The stable identifier of the one canonical voice-limit surface field.
///
/// This is the name the reducer, projection, and serialization all address; it
/// is declared once here so no consumer spells it independently.
pub const VOICE_LIMIT_FIELD: &str = "voiceLimit";

/// The independent pre-dispatch oracle for the one voice-limit field.
///
/// This is the same descriptor contract [`crate::synth::VoiceEnvelope`] and the
/// mixer's global parameters already hold: bounds and edit steps live in one
/// production-owned declaration that reducer, projection, and demos all read,
/// so no caller invents its own range or step size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoiceLimitDescriptor {
    name: &'static str,
    label: &'static str,
    unit: Option<&'static str>,
    kind: ParameterKind,
    minimum: u16,
    maximum: u16,
    fine_step: u16,
    coarse_step: u16,
}

impl VoiceLimitDescriptor {
    #[allow(clippy::too_many_arguments)]
    const fn new(
        name: &'static str,
        label: &'static str,
        unit: Option<&'static str>,
        kind: ParameterKind,
        minimum: u16,
        maximum: u16,
        fine_step: u16,
        coarse_step: u16,
    ) -> Self {
        Self {
            name,
            label,
            unit,
            kind,
            minimum,
            maximum,
            fine_step,
            coarse_step,
        }
    }

    /// Returns the stable serialized and projected field name.
    pub const fn name(&self) -> &'static str {
        self.name
    }

    /// Returns the presentation label shown on the PATCH Utility row.
    pub const fn label(&self) -> &'static str {
        self.label
    }

    /// Returns the presentation unit. A voice count is unitless.
    pub const fn unit(&self) -> Option<&'static str> {
        self.unit
    }

    /// Returns the control classification this field produces.
    pub const fn kind(&self) -> ParameterKind {
        self.kind
    }

    pub const fn minimum(&self) -> u16 {
        self.minimum
    }

    pub const fn maximum(&self) -> u16 {
        self.maximum
    }

    /// Returns the integral single-step adjustment.
    pub const fn fine_step(&self) -> u16 {
        self.fine_step
    }

    /// Returns the integral coarse adjustment.
    pub const fn coarse_step(&self) -> u16 {
        self.coarse_step
    }

    /// Returns whether `value` is inside the inclusive declared bounds.
    pub const fn contains(&self, value: u16) -> bool {
        self.minimum <= value && value <= self.maximum
    }
}

/// The one voice-limit field, enumerated exactly once.
///
/// The single-entry table is deliberate: consumers iterate one declaration
/// rather than special-casing a lone field, which is the same shape the ADSR
/// surface descriptor already presents.
const VOICE_LIMIT_SURFACE_DESCRIPTOR: [VoiceLimitDescriptor; 1] = [VoiceLimitDescriptor::new(
    VOICE_LIMIT_FIELD,
    "Voice Limit",
    None,
    ParameterKind::Stepped,
    VoiceLimit::MINIMUM,
    VoiceLimit::MAXIMUM,
    1,
    8,
)];

/// The reason a canonical voice limit could not be constructed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VoiceLimitError {
    /// The value fell outside the inclusive declared bounds. The construction
    /// is refused; it is never clamped, wrapped, defaulted, or substituted.
    OutOfRange { value: u16 },
}

impl fmt::Display for VoiceLimitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::OutOfRange { value } => write!(
                formatter,
                "{VOICE_LIMIT_FIELD} must be in {}..={}, got {value}",
                VoiceLimit::MINIMUM,
                VoiceLimit::MAXIMUM
            ),
        }
    }
}

impl std::error::Error for VoiceLimitError {}

/// The canonical Patch-owned ceiling on how many of that Patch's notes may
/// sound at once.
///
/// The limit is Patch-local and follows the Patch. It is not a device polyphony
/// setting, not a global voice budget, and not a second name for an engine's own
/// capacity ceiling. It carries no engine object, voice handle, allocator,
/// buffer, or device capacity — only a bounded integer the audio callback
/// compares against the active-note count it already maintains.
///
/// Enforcement refuses rather than steals: a note-on arriving while the Patch
/// already sounds its limit is not started, and no sounding note is truncated to
/// make room.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct VoiceLimit(u16);

impl VoiceLimit {
    /// The smallest representable limit. A Patch always sounds at least one
    /// note; a zero limit is a mute, which is the mixer's concern.
    pub const MINIMUM: u16 = 1;

    /// Storage representation bound, independent of any engine or hardware budget.
    pub const MAXIMUM: u16 = u16::MAX;

    /// Returns the one voice-limit field exactly once.
    ///
    /// The single-element array stays even though every production caller
    /// wants [`Self::descriptor`] instead. It is what makes this value satisfy
    /// the same descriptor contract `VoiceEnvelope` and `GlobalParameters`
    /// hold — "the descriptor enumerates the field exactly once" — and that
    /// contract is what a projection or a schema sweep reads a value's fields
    /// through. `VoiceLimit` has exactly one field today; a value that
    /// declared its surface differently from its neighbours because of that
    /// accident would be a shape no sweep could rely on.
    pub const fn surface_descriptor() -> &'static [VoiceLimitDescriptor] {
        &VOICE_LIMIT_SURFACE_DESCRIPTOR
    }

    /// Returns the single descriptor for callers addressing the field directly.
    pub const fn descriptor() -> &'static VoiceLimitDescriptor {
        &VOICE_LIMIT_SURFACE_DESCRIPTOR[0]
    }

    /// Constructs a positive voice budget.
    ///
    /// Out-of-range input is an error, never a clamped, wrapped, defaulted, or
    /// substituted value — the same refuse-rather-than-wrap idiom the reducer
    /// applies at every other boundary. Seeding, which narrows a declared engine
    /// ceiling into these bounds, is [`Self::seeded_from`].
    pub const fn new(value: u16) -> Result<Self, VoiceLimitError> {
        if value < Self::MINIMUM {
            return Err(VoiceLimitError::OutOfRange { value });
        }
        Ok(Self(value))
    }

    /// Seeds a new Patch from its capability's initial prepared voice budget.
    /// Configurable defaults do not establish a product maximum. Increasing an
    /// installed Patch's budget prepares replacement storage before activation.
    pub const fn seeded_from(policy: VoicePolicy) -> Self {
        Self::seeded_from_ceiling(policy.initial_voices())
    }

    /// Seeds from a raw declared ceiling. See [`Self::seeded_from`].
    pub const fn seeded_from_ceiling(ceiling: u16) -> Self {
        if ceiling < Self::MINIMUM {
            return Self(Self::MINIMUM);
        }
        Self(ceiling)
    }

    /// Returns the bounded value.
    pub const fn value(self) -> u16 {
        self.0
    }

    /// Returns the limit in the width the callback's active-note counter uses,
    /// so enforcement is one integer comparison with no conversion branch.
    pub const fn active_note_ceiling(self) -> u32 {
        self.0 as u32
    }
}

impl fmt::Display for VoiceLimit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::{VoiceLimit, VoiceLimitError, ENGINE_MANAGED_POLYPHONY_CEILING, VOICE_LIMIT_FIELD};
    use crate::adapter::braids_capability::{BRAIDS_CAPABILITY_ID, BRAIDS_FIXED_VOICES};
    use crate::adapter::hidef_soundfont_capability::{
        HIDEF_CAPABILITY_ID, HIDEF_POLYPHONY_CEILING,
    };
    use crate::adapter::production_instruments::production_capability_registry;
    use crate::control::SemanticControlKind;
    use crate::synth::instrument_capability::{ParameterKind, VoicePolicy};

    #[test]
    fn construction_refuses_out_of_range_values_instead_of_clamping_them() {
        assert_eq!(
            VoiceLimit::new(0),
            Err(VoiceLimitError::OutOfRange { value: 0 })
        );
        assert_eq!(VoiceLimit::new(65).unwrap().value(), 65);
        assert_eq!(VoiceLimit::new(u16::MAX).unwrap().value(), u16::MAX);

        assert_eq!(VoiceLimit::new(1).unwrap().value(), 1);
        assert_eq!(VoiceLimit::new(64).unwrap().value(), 64);
        assert_eq!(VoiceLimit::new(37).unwrap().value(), 37);
    }

    /// The refusal must be legible, because a clamped value and a refused one
    /// are indistinguishable to a caller that only reads the success path.
    #[test]
    fn the_refusal_names_the_bound_it_violated() {
        assert_eq!(
            VoiceLimit::new(0).unwrap_err().to_string(),
            "voiceLimit must be in 1..=65535, got 0"
        );
    }

    #[test]
    fn the_descriptor_enumerates_the_one_field_exactly_once() {
        let descriptor = VoiceLimit::surface_descriptor();

        assert_eq!(descriptor.len(), 1);
        assert_eq!(descriptor[0], *VoiceLimit::descriptor());

        let field = VoiceLimit::descriptor();
        assert_eq!(field.name(), VOICE_LIMIT_FIELD);
        assert_eq!(field.label(), "Voice Limit");
        assert_eq!(field.unit(), None, "a voice count is unitless");
        assert_eq!(field.minimum(), 1);
        assert_eq!(field.maximum(), u16::MAX);
        assert_eq!(field.fine_step(), 1);
        assert_eq!(field.coarse_step(), 8);
        assert!(!field.contains(0));
        assert!(field.contains(1));
        assert!(field.contains(64));
        assert!(field.contains(65));
    }

    /// The claim this subtask exists to make: the `Stepped` classification, which
    /// the component vocabulary has always declared and nothing ever drove, now
    /// has a production producer. The assertion goes through the production
    /// `ParameterKind -> SemanticControlKind` mapping rather than restating the
    /// variant, so a producer that classified itself `Continuous` fails here.
    #[test]
    fn the_descriptor_classifies_the_field_as_stepped() {
        assert_eq!(VoiceLimit::descriptor().kind(), ParameterKind::Stepped);
        assert_eq!(
            SemanticControlKind::from(VoiceLimit::descriptor().kind()),
            SemanticControlKind::Stepped
        );
    }

    /// Every production capability declares a configurable, valid initial budget.
    #[test]
    fn production_capabilities_declare_configurable_voice_budgets() {
        assert_eq!(ENGINE_MANAGED_POLYPHONY_CEILING, HIDEF_POLYPHONY_CEILING);
        assert_eq!(VoiceLimit::MAXIMUM, u16::MAX);

        let registry = production_capability_registry().unwrap();
        assert!(registry
            .descriptors()
            .iter()
            .any(|d| d.id().as_str() == HIDEF_CAPABILITY_ID));
        assert!(registry
            .descriptors()
            .iter()
            .any(|d| d.id().as_str() == BRAIDS_CAPABILITY_ID));
        for descriptor in registry.descriptors() {
            let VoicePolicy::Configurable { default_voices } = descriptor.voice_policy() else {
                panic!("{} must expose a configurable budget", descriptor.id());
            };
            assert_eq!(
                VoiceLimit::seeded_from(descriptor.voice_policy()),
                VoiceLimit::new(default_voices).unwrap()
            );
        }
    }

    #[test]
    fn seeding_narrows_a_declared_ceiling_into_the_bounds() {
        assert_eq!(
            VoiceLimit::seeded_from(VoicePolicy::EngineManaged).value(),
            HIDEF_POLYPHONY_CEILING
        );
        assert_eq!(
            VoiceLimit::seeded_from(VoicePolicy::FixedPerPatch {
                voices: BRAIDS_FIXED_VOICES
            })
            .value(),
            BRAIDS_FIXED_VOICES
        );
        // A larger configured default is preserved. Legacy zero declarations
        // still seed the smallest positive budget.
        assert_eq!(
            VoiceLimit::seeded_from(VoicePolicy::Configurable {
                default_voices: 4096
            })
            .value(),
            4096
        );
        assert_eq!(
            VoiceLimit::seeded_from_ceiling(0).value(),
            VoiceLimit::MINIMUM
        );
    }

    #[test]
    fn the_limit_carries_only_a_bounded_integer() {
        fn assert_copy<T: Copy>() {}

        assert_copy::<VoiceLimit>();
        assert!(!core::mem::needs_drop::<VoiceLimit>());
        assert_eq!(
            core::mem::size_of::<VoiceLimit>(),
            core::mem::size_of::<u16>()
        );
        assert_eq!(
            serde_json::to_string(&VoiceLimit::new(12).unwrap()).unwrap(),
            "12"
        );
        assert_eq!(VoiceLimit::new(12).unwrap().active_note_ceiling(), 12_u32);
    }
}
