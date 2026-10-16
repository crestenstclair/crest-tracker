use crate::kernel::PatchId;
use crate::mixer::bus_return::BusReturnBank;
use crate::mixer::global_parameters::GlobalParameters;
use crate::mixer::mixer_state::MixerState;
use crate::real_time::GraphRevision;
use crate::synth::Patch;

pub(crate) type PatchSampleVisualizations =
    std::collections::BTreeMap<PatchId, Vec<crate::synth::PreparedSampleVisualization>>;

/// One reducer payload correlated with an already prepared complete graph.
///
/// Its fields and constructor remain crate-private so persistence, shell, and
/// adapters cannot assemble a competing session model. The sole producer is
/// `PreparedSavedSession`; the sole consumer is `AppState::apply`.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionReplacementPayload {
    patches: Vec<Patch>,
    asset_descriptors: Vec<crate::synth::CapabilityDescriptor>,
    mixer: MixerState,
    global: GlobalParameters,
    returns: BusReturnBank,
    target_graph_revision: GraphRevision,
    sample_visualizations: PatchSampleVisualizations,
}

impl SessionReplacementPayload {
    pub(crate) fn from_prepared_state(
        state: &crate::control::AppState,
        target_graph_revision: GraphRevision,
        sample_visualizations: PatchSampleVisualizations,
    ) -> Self {
        Self {
            patches: state.patches().to_vec(),
            asset_descriptors: state.capabilities().asset_descriptors().to_vec(),
            mixer: state.mixer().clone(),
            global: *state.global(),
            returns: state.bus_returns().clone(),
            target_graph_revision,
            sample_visualizations,
        }
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        Vec<Patch>,
        MixerState,
        GlobalParameters,
        BusReturnBank,
        GraphRevision,
        PatchSampleVisualizations,
    ) {
        (
            self.patches,
            self.mixer,
            self.global,
            self.returns,
            self.target_graph_revision,
            self.sample_visualizations,
        )
    }

    pub(crate) fn asset_descriptors(&self) -> &[crate::synth::CapabilityDescriptor] {
        &self.asset_descriptors
    }

    pub const fn target_graph_revision(&self) -> GraphRevision {
        self.target_graph_revision
    }

    pub fn patch_ids(&self) -> impl ExactSizeIterator<Item = PatchId> + '_ {
        self.patches.iter().map(Patch::id)
    }
}
