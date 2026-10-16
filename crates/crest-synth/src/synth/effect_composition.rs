use crate::synth::{
    EffectCapabilityError, EffectCapabilityId, EffectCapabilityProvider, EffectCapabilityRegistry,
    EffectPreparer,
};
use core::fmt;

/// Validates and freezes the exact effect provider/preparer registration.
pub fn compose_effect_registry(
    providers: &[Box<dyn EffectCapabilityProvider>],
    preparers: &[Box<dyn EffectPreparer>],
) -> Result<EffectCapabilityRegistry, EffectCompositionError> {
    let descriptors = providers
        .iter()
        .map(|provider| provider.descriptor())
        .collect::<Vec<_>>();
    for (index, descriptor) in descriptors.iter().enumerate() {
        if descriptors[..index]
            .iter()
            .any(|prior| prior.id() == descriptor.id())
        {
            return Err(EffectCompositionError::DuplicateProvider {
                capability_id: descriptor.id().clone(),
            });
        }
    }
    for (index, preparer) in preparers.iter().enumerate() {
        if preparers[..index]
            .iter()
            .any(|prior| prior.capability_id() == preparer.capability_id())
        {
            return Err(EffectCompositionError::DuplicatePreparer {
                capability_id: preparer.capability_id().clone(),
            });
        }
    }
    let missing = descriptors
        .iter()
        .find(|descriptor| {
            !preparers
                .iter()
                .any(|preparer| preparer.capability_id() == descriptor.id())
        })
        .map(|descriptor| descriptor.id().clone());
    let unknown = preparers
        .iter()
        .find(|preparer| {
            !descriptors
                .iter()
                .any(|descriptor| descriptor.id() == preparer.capability_id())
        })
        .map(|preparer| preparer.capability_id().clone());
    match (missing, unknown) {
        (Some(provider_id), Some(preparer_id)) if providers.len() == preparers.len() => {
            return Err(EffectCompositionError::MismatchedRegistration {
                provider_id,
                preparer_id,
            });
        }
        (_, Some(capability_id)) => {
            return Err(EffectCompositionError::UnknownPreparer { capability_id });
        }
        (Some(capability_id), _) => {
            return Err(EffectCompositionError::MissingPreparer { capability_id });
        }
        _ => {}
    }
    EffectCapabilityRegistry::new(descriptors).map_err(EffectCompositionError::Capability)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EffectCompositionError {
    DuplicateProvider {
        capability_id: EffectCapabilityId,
    },
    DuplicatePreparer {
        capability_id: EffectCapabilityId,
    },
    MissingPreparer {
        capability_id: EffectCapabilityId,
    },
    UnknownPreparer {
        capability_id: EffectCapabilityId,
    },
    MismatchedRegistration {
        provider_id: EffectCapabilityId,
        preparer_id: EffectCapabilityId,
    },
    Capability(EffectCapabilityError),
}

impl fmt::Display for EffectCompositionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateProvider { capability_id } => {
                write!(
                    formatter,
                    "duplicate effect capability provider {capability_id}"
                )
            }
            Self::DuplicatePreparer { capability_id } => {
                write!(formatter, "duplicate effect preparer {capability_id}")
            }
            Self::MissingPreparer { capability_id } => {
                write!(
                    formatter,
                    "effect capability {capability_id} has no preparer"
                )
            }
            Self::UnknownPreparer { capability_id } => {
                write!(formatter, "effect preparer {capability_id} has no provider")
            }
            Self::MismatchedRegistration {
                provider_id,
                preparer_id,
            } => write!(
                formatter,
                "effect provider {provider_id} is paired with preparer {preparer_id}"
            ),
            Self::Capability(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for EffectCompositionError {}
