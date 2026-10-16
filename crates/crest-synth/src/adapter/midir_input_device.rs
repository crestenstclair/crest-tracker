use crate::control::{
    ActiveMidiInput, ConnectMidiInput, MidiConnectionFailureClass, MidiDeviceFailure,
    MidiInputDescriptor, MidiInputDeviceId, MidiInputDevicePort, MidiStaleOperation,
    PhysicalMidiIngress,
};

pub const MIDIR_IDENTITY_SCHEMA: &str = "midir-v1";
const MIDIR_CLIENT_NAME: &str = "Crest Synth MIDI Input";
const MIDIR_CONNECTION_NAME: &str = "crest-synth-input";

trait MidirEnumerationBackend {
    type Port;

    fn ports(&self) -> Vec<Self::Port>;
    fn port_id(&self, port: &Self::Port) -> Result<String, ()>;
    fn port_name(&self, port: &Self::Port) -> Result<String, ()>;
}

impl MidirEnumerationBackend for midir::MidiInput {
    type Port = midir::MidiInputPort;

    fn ports(&self) -> Vec<Self::Port> {
        self.ports()
    }

    fn port_id(&self, port: &Self::Port) -> Result<String, ()> {
        Ok(port.id())
    }

    fn port_name(&self, port: &Self::Port) -> Result<String, ()> {
        self.port_name(port).map_err(|_| ())
    }
}

fn enumerate_backend(
    backend: &impl MidirEnumerationBackend,
) -> Result<Vec<MidiInputDescriptor>, MidiDeviceFailure> {
    let mut descriptors = Vec::new();
    for port in backend.ports() {
        let identity = backend
            .port_id(&port)
            .ok()
            .and_then(|value| MidiInputDeviceId::new(MIDIR_IDENTITY_SCHEMA, value).ok())
            .ok_or(MidiDeviceFailure::PortInformationFailed { identity: None })?;
        let display_name =
            backend
                .port_name(&port)
                .map_err(|_| MidiDeviceFailure::PortInformationFailed {
                    identity: Some(identity.clone()),
                })?;
        let descriptor =
            MidiInputDescriptor::new(identity.clone(), display_name, None).map_err(|_| {
                MidiDeviceFailure::PortInformationFailed {
                    identity: Some(identity),
                }
            })?;
        if descriptors
            .iter()
            .any(|existing: &MidiInputDescriptor| existing.id() == descriptor.id())
        {
            return Err(MidiDeviceFailure::PortInformationFailed {
                identity: Some(descriptor.id().clone()),
            });
        }
        descriptors.push(descriptor);
    }
    descriptors.sort_by(|left, right| {
        left.display_name()
            .to_lowercase()
            .cmp(&right.display_name().to_lowercase())
            .then_with(|| left.id().cmp(right.id()))
    });
    Ok(descriptors)
}

struct MidirActiveConnection(midir::MidiInputConnection<PhysicalMidiIngress>);

/// Production midir adapter. It reconstructs a client per operation because
/// `midir::MidiInput::connect` consumes its client into the active handle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MidirInputDeviceAdapter {
    client_name: String,
}

/// Builds the production device capability without turning an unavailable
/// platform backend into an application-startup failure. The unavailable
/// capability reports the exact typed initialization failure through every
/// worker-side operation, so Settings can remain open and truthful.
pub fn system_midi_input_device() -> Box<dyn MidiInputDevicePort> {
    match MidirInputDeviceAdapter::new() {
        Ok(adapter) => Box::new(adapter),
        Err(failure) => Box::new(UnavailableMidirInputDevice { failure }),
    }
}

struct UnavailableMidirInputDevice {
    failure: MidiDeviceFailure,
}

impl MidiInputDevicePort for UnavailableMidirInputDevice {
    fn enumerate(&mut self) -> Result<Vec<MidiInputDescriptor>, MidiDeviceFailure> {
        Err(self.failure.clone())
    }

    fn connect(
        &mut self,
        _request: ConnectMidiInput,
        _ingress: PhysicalMidiIngress,
    ) -> Result<ActiveMidiInput, MidiDeviceFailure> {
        Err(self.failure.clone())
    }

    fn disconnect(&mut self, _active: ActiveMidiInput) -> Result<(), MidiDeviceFailure> {
        Err(self.failure.clone())
    }
}

impl MidirInputDeviceAdapter {
    pub fn new() -> Result<Self, MidiDeviceFailure> {
        midir::MidiInput::new(MIDIR_CLIENT_NAME)
            .map_err(|_| MidiDeviceFailure::InitializationUnavailable)?;
        Ok(Self {
            client_name: MIDIR_CLIENT_NAME.to_owned(),
        })
    }

    fn input(&self) -> Result<midir::MidiInput, MidiDeviceFailure> {
        midir::MidiInput::new(&self.client_name)
            .map_err(|_| MidiDeviceFailure::InitializationUnavailable)
    }
}

fn midir_callback(timestamp_micros: u64, raw: &[u8], ingress: &mut PhysicalMidiIngress) {
    let _ = ingress.receive_raw(timestamp_micros, raw);
}

impl MidiInputDevicePort for MidirInputDeviceAdapter {
    fn enumerate(&mut self) -> Result<Vec<MidiInputDescriptor>, MidiDeviceFailure> {
        let mut input = self.input()?;
        input.ignore(midir::Ignore::None);
        enumerate_backend(&input)
    }

    fn connect(
        &mut self,
        request: ConnectMidiInput,
        ingress: PhysicalMidiIngress,
    ) -> Result<ActiveMidiInput, MidiDeviceFailure> {
        if request.identity().identity_schema() != MIDIR_IDENTITY_SCHEMA
            || ingress.revision() != request.revision()
        {
            return Err(MidiDeviceFailure::IdentityUnavailable {
                identity: request.identity().clone(),
            });
        }
        let mut input = self
            .input()
            .map_err(|_| MidiDeviceFailure::ConnectionFailed {
                identity: request.identity().clone(),
                class: MidiConnectionFailureClass::BackendUnavailable,
            })?;
        input.ignore(midir::Ignore::None);
        let mut matches = input
            .ports()
            .into_iter()
            .filter(|port| port.id() == request.identity().identity());
        let port = matches
            .next()
            .filter(|_| matches.next().is_none())
            .ok_or_else(|| MidiDeviceFailure::IdentityUnavailable {
                identity: request.identity().clone(),
            })?;
        let connection = input
            .connect(&port, MIDIR_CONNECTION_NAME, midir_callback, ingress)
            .map_err(|error| MidiDeviceFailure::ConnectionFailed {
                identity: request.identity().clone(),
                class: match error.kind() {
                    midir::ConnectErrorKind::InvalidPort => MidiConnectionFailureClass::Rejected,
                    midir::ConnectErrorKind::Other(_) => MidiConnectionFailureClass::Unknown,
                },
            })?;
        Ok(ActiveMidiInput::from_backend(
            &request,
            MidirActiveConnection(connection),
        ))
    }

    fn disconnect(&mut self, active: ActiveMidiInput) -> Result<(), MidiDeviceFailure> {
        let identity = active.identity().clone();
        let revision = active.revision();
        let connection = active
            .into_backend::<MidirActiveConnection>()
            .map_err(|_| MidiDeviceFailure::StaleCorrelation {
                operation: MidiStaleOperation::Retirement,
            })?;
        let (_input, _ingress) = connection.0.close();
        let _ = (identity, revision);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::{
        MidiConnectionRequestId, MidiConnectionRevision, PhysicalMidiIngressOutcome,
    };

    #[derive(Clone)]
    struct FakePort {
        id: Result<&'static str, ()>,
        name: Result<&'static str, ()>,
    }

    struct FakeBackend(Vec<FakePort>);

    impl MidirEnumerationBackend for FakeBackend {
        type Port = FakePort;

        fn ports(&self) -> Vec<Self::Port> {
            self.0.clone()
        }

        fn port_id(&self, port: &Self::Port) -> Result<String, ()> {
            port.id.map(str::to_owned)
        }

        fn port_name(&self, port: &Self::Port) -> Result<String, ()> {
            port.name.map(str::to_owned)
        }
    }

    fn fake(id: &'static str, name: &'static str) -> FakePort {
        FakePort {
            id: Ok(id),
            name: Ok(name),
        }
    }

    #[test]
    fn fake_enumeration_namespaces_normalizes_and_rejects_duplicate_or_failed_facts() {
        let descriptors = enumerate_backend(&FakeBackend(vec![
            fake("b", "Same Name"),
            fake("a", "same name"),
            fake("c", "Alpha"),
        ]))
        .unwrap();
        assert_eq!(
            descriptors
                .iter()
                .map(|descriptor| descriptor.id().identity())
                .collect::<Vec<_>>(),
            ["c", "a", "b"]
        );
        assert!(descriptors
            .iter()
            .all(|descriptor| descriptor.id().identity_schema() == MIDIR_IDENTITY_SCHEMA));
        assert!(matches!(
            enumerate_backend(&FakeBackend(vec![fake("a", "One"), fake("a", "Two")])),
            Err(MidiDeviceFailure::PortInformationFailed { identity: Some(_) })
        ));
        assert!(matches!(
            enumerate_backend(&FakeBackend(vec![FakePort {
                id: Ok("a"),
                name: Err(())
            }])),
            Err(MidiDeviceFailure::PortInformationFailed { identity: Some(_) })
        ));
        assert!(matches!(
            enumerate_backend(&FakeBackend(vec![FakePort {
                id: Err(()),
                name: Ok("A")
            }])),
            Err(MidiDeviceFailure::PortInformationFailed { identity: None })
        ));
    }

    #[test]
    fn disabled_prebuilt_ingress_rejects_callbacks_before_activation() {
        let revision = MidiConnectionRevision::FIRST;
        let request = ConnectMidiInput::new(
            MidiInputDeviceId::new(MIDIR_IDENTITY_SCHEMA, "a").unwrap(),
            MidiConnectionRequestId::FIRST,
            revision,
        );
        let (mut ingress, mut control) = PhysicalMidiIngress::bounded(revision);
        midir_callback(1, &[0x90, 60, 100], &mut ingress);
        assert_eq!(control.try_pop(), None);
        assert_eq!(control.diagnostics().inactive, 1);
        assert!(!control.is_enabled());
        assert_eq!(
            ingress.receive_raw(2, &[0x90, 60, 100]),
            PhysicalMidiIngressOutcome::Inactive
        );
        assert_eq!(request.identity().identity(), "a");
    }

    #[test]
    fn real_host_seam_reports_zero_or_more_ports_or_one_typed_initialization_failure() {
        match MidirInputDeviceAdapter::new() {
            Ok(mut adapter) => match adapter.enumerate() {
                Ok(descriptors) => {
                    println!(
                        "CREST_MIDI_HOST_ENUMERATION ports={} schema={MIDIR_IDENTITY_SCHEMA}",
                        descriptors.len()
                    );
                    for (index, descriptor) in descriptors.iter().enumerate() {
                        assert!(!descriptor.display_name().is_empty());
                        assert!(!descriptors[..index]
                            .iter()
                            .any(|prior| prior.id() == descriptor.id()));
                    }
                }
                Err(failure) => assert!(matches!(
                    failure,
                    MidiDeviceFailure::InitializationUnavailable
                        | MidiDeviceFailure::PortInformationFailed { .. }
                )),
            },
            Err(failure) => assert_eq!(failure, MidiDeviceFailure::InitializationUnavailable),
        }
    }

    #[test]
    fn system_capability_keeps_platform_initialization_failure_behind_the_port() {
        let mut capability = system_midi_input_device();
        match capability.enumerate() {
            Ok(descriptors) => assert!(descriptors
                .windows(2)
                .all(|pair| pair[0].id() != pair[1].id())),
            Err(failure) => assert!(matches!(
                failure,
                MidiDeviceFailure::InitializationUnavailable
                    | MidiDeviceFailure::PortInformationFailed { .. }
            )),
        }
    }
}
