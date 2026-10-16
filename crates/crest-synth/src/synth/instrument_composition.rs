use crate::synth::capability_id::CapabilityId;
use crate::synth::instrument_capability::{CapabilityError, CapabilityRegistry};
use crate::synth::instrument_capability_provider::InstrumentCapabilityProvider;
use crate::synth::instrument_preparer::InstrumentPreparer;
use core::fmt;

/// Validates the exact provider/preparer registration installed by a
/// composition root and freezes its canonical descriptor registry.
pub fn compose_instrument_registry(
    providers: &[Box<dyn InstrumentCapabilityProvider>],
    preparers: &[Box<dyn InstrumentPreparer>],
) -> Result<CapabilityRegistry, InstrumentCompositionError> {
    let descriptors = providers
        .iter()
        .map(|provider| provider.descriptor())
        .collect::<Vec<_>>();

    for (index, descriptor) in descriptors.iter().enumerate() {
        if descriptors[..index]
            .iter()
            .any(|prior| prior.id() == descriptor.id())
        {
            return Err(InstrumentCompositionError::DuplicateProvider {
                capability_id: descriptor.id().clone(),
            });
        }
    }

    for (index, preparer) in preparers.iter().enumerate() {
        if preparers[..index]
            .iter()
            .any(|prior| prior.capability_id() == preparer.capability_id())
        {
            return Err(InstrumentCompositionError::DuplicatePreparer {
                capability_id: preparer.capability_id().clone(),
            });
        }
    }

    let missing = descriptors
        .iter()
        .filter(|descriptor| {
            !preparers
                .iter()
                .any(|preparer| preparer.capability_id() == descriptor.id())
        })
        .map(|descriptor| descriptor.id().clone())
        .collect::<Vec<_>>();
    let unknown = preparers
        .iter()
        .filter(|preparer| {
            !descriptors
                .iter()
                .any(|descriptor| descriptor.id() == preparer.capability_id())
        })
        .map(|preparer| preparer.capability_id().clone())
        .collect::<Vec<_>>();

    if providers.len() == preparers.len() && missing.len() == 1 && unknown.len() == 1 {
        return Err(InstrumentCompositionError::MismatchedRegistration {
            provider_id: missing[0].clone(),
            preparer_id: unknown[0].clone(),
        });
    }
    if let Some(capability_id) = unknown.into_iter().next() {
        return Err(InstrumentCompositionError::UnknownPreparer { capability_id });
    }
    if let Some(capability_id) = missing.into_iter().next() {
        return Err(InstrumentCompositionError::MissingPreparer { capability_id });
    }

    CapabilityRegistry::new(descriptors).map_err(InstrumentCompositionError::Capability)
}

/// Typed no-fallback failures for composition-root registration mistakes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InstrumentCompositionError {
    DuplicateProvider {
        capability_id: CapabilityId,
    },
    DuplicatePreparer {
        capability_id: CapabilityId,
    },
    MissingPreparer {
        capability_id: CapabilityId,
    },
    UnknownPreparer {
        capability_id: CapabilityId,
    },
    MismatchedRegistration {
        provider_id: CapabilityId,
        preparer_id: CapabilityId,
    },
    Capability(CapabilityError),
}

impl fmt::Display for InstrumentCompositionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateProvider { capability_id } => {
                write!(formatter, "duplicate capability provider {capability_id}")
            }
            Self::DuplicatePreparer { capability_id } => {
                write!(formatter, "duplicate instrument preparer {capability_id}")
            }
            Self::MissingPreparer { capability_id } => {
                write!(formatter, "capability {capability_id} has no preparer")
            }
            Self::UnknownPreparer { capability_id } => {
                write!(formatter, "preparer {capability_id} has no provider")
            }
            Self::MismatchedRegistration {
                provider_id,
                preparer_id,
            } => write!(
                formatter,
                "provider {provider_id} is paired with preparer {preparer_id}"
            ),
            Self::Capability(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for InstrumentCompositionError {}

#[cfg(test)]
mod tests {
    use super::{compose_instrument_registry, InstrumentCompositionError};
    use crate::adapter::braids_capability::{BraidsCapability, BRAIDS_CAPABILITY_ID};
    use crate::adapter::hidef_soundfont_capability::HIDEF_CAPABILITY_ID;
    use crate::kernel::patch_id::PatchId;
    use crate::synth::{
        CapabilityId, InstrumentCapabilityProvider, InstrumentPreparationError, InstrumentPreparer,
        Patch, PreparedInstrument,
    };

    struct NeverPreparer(CapabilityId);

    impl NeverPreparer {
        fn new(id: &str) -> Self {
            Self(CapabilityId::new(id).unwrap())
        }
    }

    impl InstrumentPreparer for NeverPreparer {
        fn capability_id(&self) -> &CapabilityId {
            &self.0
        }

        fn prepare(
            &self,
            patch: &Patch,
            _sample_rate: f32,
            _max_frames: usize,
        ) -> Result<Box<dyn PreparedInstrument>, InstrumentPreparationError> {
            Err(InstrumentPreparationError::PreparationFailed {
                patch_id: PatchId::new(patch.id().value()).unwrap(),
            })
        }
    }

    fn hidef() -> Box<dyn InstrumentCapabilityProvider> {
        Box::new(crate::adapter::production_instruments::production_soundfont_capability().unwrap())
    }

    fn braids() -> Box<dyn InstrumentCapabilityProvider> {
        Box::new(BraidsCapability::new().unwrap())
    }

    fn preparer(id: &str) -> Box<dyn InstrumentPreparer> {
        Box::new(NeverPreparer::new(id))
    }

    #[test]
    fn matching_provider_and_preparer_registrations_freeze_in_provider_order() {
        let registry = compose_instrument_registry(
            &[hidef(), braids()],
            &[
                preparer(HIDEF_CAPABILITY_ID),
                preparer(BRAIDS_CAPABILITY_ID),
            ],
        )
        .unwrap();
        assert_eq!(
            registry
                .descriptors()
                .iter()
                .map(|descriptor| descriptor.id().as_str())
                .collect::<Vec<_>>(),
            [HIDEF_CAPABILITY_ID, BRAIDS_CAPABILITY_ID]
        );
    }

    #[test]
    fn duplicate_missing_unknown_and_mismatched_registrations_are_typed() {
        assert!(matches!(
            compose_instrument_registry(&[hidef(), hidef()], &[preparer(HIDEF_CAPABILITY_ID)]),
            Err(InstrumentCompositionError::DuplicateProvider { .. })
        ));
        assert!(matches!(
            compose_instrument_registry(
                &[hidef()],
                &[preparer(HIDEF_CAPABILITY_ID), preparer(HIDEF_CAPABILITY_ID)]
            ),
            Err(InstrumentCompositionError::DuplicatePreparer { .. })
        ));
        assert!(matches!(
            compose_instrument_registry(&[hidef(), braids()], &[preparer(HIDEF_CAPABILITY_ID)]),
            Err(InstrumentCompositionError::MissingPreparer { .. })
        ));
        assert!(matches!(
            compose_instrument_registry(
                &[hidef()],
                &[
                    preparer(HIDEF_CAPABILITY_ID),
                    preparer("instrument.test.unknown")
                ]
            ),
            Err(InstrumentCompositionError::UnknownPreparer { .. })
        ));
        assert!(matches!(
            compose_instrument_registry(&[hidef()], &[preparer(BRAIDS_CAPABILITY_ID)]),
            Err(InstrumentCompositionError::MismatchedRegistration { .. })
        ));
    }
}
