use crate::control::app_state::AppState;
use crate::control::graphical_shell_projection::{
    GraphicalShellProjection, GraphicalShellProjectionError, ShellContextLine, ShellFooter,
    ShellIdentityHeader,
};
use crate::control::interaction_state::{Selection, SelectionSection};
use crate::control::patch_page_projection::{PatchPageProjection, PatchPageProjectionError};
use crate::control::serialized_state::SerializedState;
use crate::control::state_snapshot::StateSnapshot;
use crate::control::state_tree::{StateTree, StateTreeError};
use crate::control::text_projection::TextProjection;
use crate::control::{
    MixerControlId, SemanticControlId, SemanticGraphicalViewModel, SemanticGraphicalViewModelError,
};
use crate::real_time::parameter_snapshot::{ParameterSnapshot, ParameterSnapshotError};
use crate::real_time::GraphRevision;
use core::fmt;

const MIXER_HEADER: &str =
    "MIXER | 1 MIXER | 2 PATCH | W/S parameters | A/D tracks | K+direction edit";
const PATCH_HEADER: &str = "PATCH | 1 MIXER | 2 PATCH | W/S controls | K+direction edit";
const SEPARATOR: &str = "------------------------------------------------------------";

/// Complete coherent projection set used by AppLoop and observation consumers.
pub type ShellTreeProjection = (
    StateSnapshot,
    Option<PatchPageProjection>,
    TextProjection,
    GraphicalShellProjection,
    ParameterSnapshot,
    StateTree,
);

/// Prior immutable values reused by the bounded MIDI generation-only path.
pub(crate) struct MidiProjectionSeed<'a> {
    snapshot: &'a StateSnapshot,
    page: Option<&'a PatchPageProjection>,
    text: &'a TextProjection,
    shell: &'a GraphicalShellProjection,
    parameters: ParameterSnapshot,
    tree: &'a StateTree,
}

impl<'a> MidiProjectionSeed<'a> {
    pub(crate) const fn new(
        snapshot: &'a StateSnapshot,
        page: Option<&'a PatchPageProjection>,
        text: &'a TextProjection,
        shell: &'a GraphicalShellProjection,
        parameters: ParameterSnapshot,
        tree: &'a StateTree,
    ) -> Self {
        Self {
            snapshot,
            page,
            text,
            shell,
            parameters,
            tree,
        }
    }
}

/// A projection failure detected on the control side before publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StateProjectionError {
    StateSerialization,
    StateDeserialization,
    StateGenerationTemplateMismatch,
    SelectionDoesNotMatchSnapshot,
    InvalidSelection,
    InvalidInstrumentConfig,
    InvalidEffectConfig,
    PatchPage(PatchPageProjectionError),
    GraphicalShell(GraphicalShellProjectionError),
    SemanticGraphicalViewModel(SemanticGraphicalViewModelError),
    ParameterSnapshot(ParameterSnapshotError),
    StateTree(StateTreeError),
}

impl fmt::Display for StateProjectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StateSerialization => {
                formatter.write_str("accepted control state could not be serialized")
            }
            Self::StateDeserialization => {
                formatter.write_str("control state snapshot could not be decoded")
            }
            Self::StateGenerationTemplateMismatch => formatter
                .write_str("canonical state snapshot cannot advance a generation-only projection"),
            Self::SelectionDoesNotMatchSnapshot => {
                formatter.write_str("typed selection does not match the state snapshot")
            }
            Self::InvalidSelection => {
                formatter.write_str("selection is outside the projected control state")
            }
            Self::InvalidInstrumentConfig => {
                formatter.write_str("instrument config does not resolve through the registry")
            }
            Self::InvalidEffectConfig => {
                formatter.write_str("effect config does not resolve through the registry")
            }
            Self::PatchPage(error) => error.fmt(formatter),
            Self::GraphicalShell(error) => error.fmt(formatter),
            Self::SemanticGraphicalViewModel(error) => error.fmt(formatter),
            Self::ParameterSnapshot(error) => error.fmt(formatter),
            Self::StateTree(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for StateProjectionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ParameterSnapshot(error) => Some(error),
            Self::StateTree(error) => Some(error),
            Self::PatchPage(error) => Some(error),
            Self::GraphicalShell(error) => Some(error),
            Self::SemanticGraphicalViewModel(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ParameterSnapshotError> for StateProjectionError {
    fn from(error: ParameterSnapshotError) -> Self {
        Self::ParameterSnapshot(error)
    }
}

impl From<StateTreeError> for StateProjectionError {
    fn from(error: StateTreeError) -> Self {
        Self::StateTree(error)
    }
}

impl From<PatchPageProjectionError> for StateProjectionError {
    fn from(error: PatchPageProjectionError) -> Self {
        Self::PatchPage(error)
    }
}

impl From<GraphicalShellProjectionError> for StateProjectionError {
    fn from(error: GraphicalShellProjectionError) -> Self {
        Self::GraphicalShell(error)
    }
}

impl From<SemanticGraphicalViewModelError> for StateProjectionError {
    fn from(error: SemanticGraphicalViewModelError) -> Self {
        Self::SemanticGraphicalViewModel(error)
    }
}

/// Derives all immutable effects from one already-accepted AppState.
///
/// This service never mutates AppState. Serialization and text rendering run on
/// the control side, while the returned ParameterSnapshot is fully owned and
/// fixed-size for publication to the audio boundary.
#[derive(Clone, Copy, Debug)]
pub struct StateProjector {
    graph_revision: GraphRevision,
}

impl Default for StateProjector {
    fn default() -> Self {
        Self::new()
    }
}

impl StateProjector {
    pub const fn new() -> Self {
        Self {
            graph_revision: GraphRevision::INITIAL,
        }
    }

    /// Creates a projector whose scalar output targets one prepared graph.
    pub const fn for_graph(graph_revision: GraphRevision) -> Self {
        Self { graph_revision }
    }

    pub const fn graph_revision(self) -> GraphRevision {
        self.graph_revision
    }

    /// Derives the snapshot, active text, and fixed real-time values.
    pub fn project(
        &self,
        state: &AppState,
    ) -> Result<(StateSnapshot, TextProjection, ParameterSnapshot), StateProjectionError> {
        let (snapshot, _page, text, parameters) = self.project_with_page(state)?;
        Ok((snapshot, text, parameters))
    }

    /// Derives the complete projection set including the optional PATCH page.
    pub fn project_with_page(
        &self,
        state: &AppState,
    ) -> Result<
        (
            StateSnapshot,
            Option<PatchPageProjection>,
            TextProjection,
            ParameterSnapshot,
        ),
        StateProjectionError,
    > {
        let (snapshot, page, text, _shell, parameters) = self.project_with_shell(state)?;
        Ok((snapshot, page, text, parameters))
    }

    /// Derives the active page, retained diagnostic, graphical shell, and
    /// fixed audio parameters from one canonical accepted-state view.
    pub fn project_with_shell(
        &self,
        state: &AppState,
    ) -> Result<
        (
            StateSnapshot,
            Option<PatchPageProjection>,
            TextProjection,
            GraphicalShellProjection,
            ParameterSnapshot,
        ),
        StateProjectionError,
    > {
        let serialized = SerializedState::from(state);
        let snapshot = self.snapshot_from_serialized(&serialized)?;
        let page = self.patch_page_projection(state, snapshot.hash())?;
        let text = self.text_from_serialized(
            &serialized,
            state.selection(),
            page.as_ref(),
            snapshot.hash(),
        )?;
        let semantic = SemanticGraphicalViewModel::project(state, snapshot.hash())?;
        let shell = self.graphical_shell_from_serialized(
            &serialized,
            page.as_ref(),
            &text,
            snapshot.hash(),
            &semantic,
        )?;
        let parameters = self.parameter_snapshot(state)?;

        Ok((snapshot, page, text, shell, parameters))
    }

    /// Derives every coherent projection, including the canonical observation tree.
    pub fn project_with_tree(
        &self,
        state: &AppState,
    ) -> Result<
        (
            StateSnapshot,
            Option<PatchPageProjection>,
            TextProjection,
            ParameterSnapshot,
            StateTree,
        ),
        StateProjectionError,
    > {
        let (snapshot, page, text, _shell, parameters, tree) =
            self.project_with_shell_tree(state)?;
        Ok((snapshot, page, text, parameters, tree))
    }

    /// Derives the complete canonical control projection set, including the
    /// production-window shell and observation tree, from one snapshot.
    pub fn project_with_shell_tree(
        &self,
        state: &AppState,
    ) -> Result<ShellTreeProjection, StateProjectionError> {
        let serialized = SerializedState::from(state);
        let snapshot = self.snapshot_from_serialized(&serialized)?;
        let page = self.patch_page_projection(state, snapshot.hash())?;
        let text = self.text_from_serialized(
            &serialized,
            state.selection(),
            page.as_ref(),
            snapshot.hash(),
        )?;
        let semantic = SemanticGraphicalViewModel::project(state, snapshot.hash())?;
        let shell = self.graphical_shell_from_serialized(
            &serialized,
            page.as_ref(),
            &text,
            snapshot.hash(),
            &semantic,
        )?;
        let parameters = self.parameter_snapshot(state)?;
        let tree = StateTree::from_serialized_state(
            &serialized,
            &snapshot,
            page.as_ref(),
            &shell,
            &text,
            &parameters,
        )?;

        Ok((snapshot, page, text, shell, parameters, tree))
    }

    /// Builds the canonical tree from one already-derived projection set.
    pub fn state_tree(
        &self,
        snapshot: &StateSnapshot,
        text: &TextProjection,
        parameters: &ParameterSnapshot,
    ) -> Result<StateTree, StateProjectionError> {
        self.state_tree_with_page(snapshot, None, text, parameters)
    }

    /// Builds the canonical tree with an explicit optional PATCH page.
    pub fn state_tree_with_page(
        &self,
        snapshot: &StateSnapshot,
        page: Option<&PatchPageProjection>,
        text: &TextProjection,
        parameters: &ParameterSnapshot,
    ) -> Result<StateTree, StateProjectionError> {
        let state: SerializedState<'_> = serde_json::from_str(snapshot.json())
            .map_err(|_| StateProjectionError::StateDeserialization)?;
        let semantic = SemanticGraphicalViewModel::fixture_for_interaction(
            state.generation,
            snapshot.hash(),
            state.interaction.active_focus.clone(),
            state.interaction.mode,
            state.interaction.return_path.clone(),
            state.interaction.remembered_patch_main.clone(),
            Some(state.interaction.remembered_mixer_main.clone()),
            state.engine_selection.projection_graph_revision(),
        );
        let shell =
            self.graphical_shell_from_serialized(&state, page, text, snapshot.hash(), &semantic)?;
        StateTree::with_patch_page_and_shell(snapshot, page, &shell, text, parameters)
            .map_err(StateProjectionError::from)
    }

    /// Serializes every AppState field through the canonical borrowed state view.
    ///
    /// Decode/encode identity is verified outside this hot production path by
    /// tests that deserialize the emitted snapshot into the same canonical type.
    pub fn state_snapshot(&self, state: &AppState) -> Result<StateSnapshot, StateProjectionError> {
        let encoded_state = SerializedState::from(state);
        self.snapshot_from_serialized(&encoded_state)
    }

    /// Renders text exclusively from one snapshot and its typed selection.
    pub fn text_projection(
        &self,
        snapshot: &StateSnapshot,
        selection: Selection,
    ) -> Result<TextProjection, StateProjectionError> {
        self.text_projection_with_page(snapshot, selection, None)
    }

    /// Renders text with an explicit optional host-neutral PATCH page.
    pub fn text_projection_with_page(
        &self,
        snapshot: &StateSnapshot,
        selection: Selection,
        page: Option<&PatchPageProjection>,
    ) -> Result<TextProjection, StateProjectionError> {
        let state: SerializedState<'_> = serde_json::from_str(snapshot.json())
            .map_err(|_| StateProjectionError::StateDeserialization)?;
        self.text_from_serialized(&state, selection, page, snapshot.hash())
    }

    /// Resolves the optional active PATCH page from canonical state and registry data.
    pub fn patch_page_projection(
        &self,
        state: &AppState,
        state_hash: &str,
    ) -> Result<Option<PatchPageProjection>, StateProjectionError> {
        if state.interaction().active_surface().is_system() {
            return Ok(None);
        }
        match state.context() {
            crate::control::TopLevelContext::Mixer => Ok(None),
            crate::control::TopLevelContext::Patch => {
                PatchPageProjection::project(state, state_hash)
                    .map(Some)
                    .map_err(StateProjectionError::from)
            }
        }
    }

    /// Derives the production graphical shell for an already-derived page and
    /// diagnostic projection from the same accepted AppState.
    pub fn graphical_shell_projection(
        &self,
        state: &AppState,
        page: Option<&PatchPageProjection>,
        diagnostic: &TextProjection,
        state_hash: &str,
    ) -> Result<GraphicalShellProjection, StateProjectionError> {
        let serialized = SerializedState::from(state);
        let semantic = SemanticGraphicalViewModel::project(state, state_hash)?;
        self.graphical_shell_from_serialized(&serialized, page, diagnostic, state_hash, &semantic)
    }

    /// Copies every audio parameter into bounded, fully owned storage.
    pub fn parameter_snapshot(
        &self,
        state: &AppState,
    ) -> Result<ParameterSnapshot, StateProjectionError> {
        ParameterSnapshot::project_patches_with_effects_and_returns(
            state.generation(),
            state.engine_selection().projection_graph_revision(),
            *state.global(),
            state.mixer().clone(),
            state.patches(),
            state.capabilities(),
            state.effects(),
            state.bus_returns(),
        )
        .map_err(|error| match error {
            ParameterSnapshotError::InvalidInstrumentConfig { .. } => {
                StateProjectionError::InvalidInstrumentConfig
            }
            ParameterSnapshotError::InvalidEffectConfig { .. } => {
                StateProjectionError::InvalidEffectConfig
            }
            other => StateProjectionError::ParameterSnapshot(other),
        })
    }

    /// Advances all coherent projections after a validated MIDI event changed
    /// only AppState generation and emitted one discrete audio command.
    pub(crate) fn project_midi_generation(
        &self,
        state: &AppState,
        previous: MidiProjectionSeed<'_>,
    ) -> Result<ShellTreeProjection, StateProjectionError> {
        let MidiProjectionSeed {
            snapshot: previous_snapshot,
            page: previous_page,
            text: previous_text,
            shell: previous_shell,
            parameters: previous_parameters,
            tree: previous_tree,
        } = previous;
        let generation = state.generation();
        if previous_parameters.generation().checked_add(1) != Some(generation)
            || previous_tree.generation().checked_add(1) != Some(generation)
            || previous_shell.generation().checked_add(1) != Some(generation)
            || previous_parameters.graph_revision()
                != state.engine_selection().projection_graph_revision()
            || previous_tree.graph_revision()
                != state.engine_selection().projection_graph_revision()
        {
            return Err(StateProjectionError::StateGenerationTemplateMismatch);
        }

        let snapshot = previous_snapshot
            .with_generation(generation)
            .ok_or(StateProjectionError::StateGenerationTemplateMismatch)?;
        let page = previous_page.map(|page| page.with_state_hash(snapshot.hash().to_owned()));
        let text = previous_text.with_state_hash(snapshot.hash().to_owned());
        let shell =
            previous_shell.with_generation(generation, snapshot.hash().to_owned(), text.clone())?;
        let parameters = previous_parameters.with_generation(generation);
        let tree = previous_tree.with_midi_generation(
            &snapshot,
            page.as_ref(),
            &shell,
            &text,
            &parameters,
        )?;
        Ok((snapshot, page, text, shell, parameters, tree))
    }

    fn snapshot_from_serialized(
        &self,
        state: &SerializedState<'_>,
    ) -> Result<StateSnapshot, StateProjectionError> {
        let json =
            serde_json::to_string(state).map_err(|_| StateProjectionError::StateSerialization)?;
        Ok(StateSnapshot::new(json))
    }

    fn text_from_serialized(
        &self,
        state: &SerializedState<'_>,
        selection: Selection,
        page: Option<&PatchPageProjection>,
        state_hash: &str,
    ) -> Result<TextProjection, StateProjectionError> {
        if state.interaction.active_focus.surface() == crate::control::SurfaceId::FileBrowser {
            return Ok(TextProjection::for_context(
                state.interaction.active_focus.context(),
                format!(
                    "FILE BROWSER\n> {}",
                    serde_json::to_string(state.interaction.active_focus.control_id())
                        .map_err(|_| StateProjectionError::StateSerialization)?
                ),
                1,
                state_hash.to_owned(),
            ));
        }
        if state.interaction.active_focus.surface() == crate::control::SurfaceId::Sends {
            let active = &state.interaction.active_focus;
            let SemanticControlId::Send(control) = active.control_id() else {
                return Err(StateProjectionError::InvalidSelection);
            };
            let send = state
                .returns
                .entries()
                .get(control.bus().index())
                .ok_or(StateProjectionError::InvalidSelection)?;
            return Ok(TextProjection::for_context(
                crate::control::TopLevelContext::Mixer,
                format!(
                    "SENDS\n> SEND {} · {} · {:?}\n{}",
                    control.bus().index() + 1,
                    send.name,
                    control,
                    serde_json::to_string(send)
                        .map_err(|_| StateProjectionError::StateSerialization)?
                ),
                1,
                state_hash.to_owned(),
            ));
        }
        if state.interaction.active_focus.surface().is_system() {
            return Ok(TextProjection::for_context(
                state.interaction.active_focus.context(),
                format!(
                    "SETTINGS · {}\n{}",
                    state.interaction.active_focus.surface().label(),
                    if state.interaction.active_focus.surface()
                        == crate::control::SurfaceId::ControllerSettings
                    {
                        state.controller.preference_status().label()
                    } else if state.interaction.active_focus.surface()
                        == crate::control::SurfaceId::SaveLoadSettings
                    {
                        "SESSION FILES"
                    } else {
                        "WATCHING FOR PHYSICAL INPUTS"
                    }
                ),
                0,
                state_hash.to_owned(),
            ));
        }
        if selection_from_serialized(state)? != selection {
            return Err(StateProjectionError::SelectionDoesNotMatchSnapshot);
        }
        match state.interaction.active_focus.context() {
            crate::control::TopLevelContext::Mixer => {
                if page.is_some() {
                    return Err(StateProjectionError::InvalidSelection);
                }
                render_mixer_text(state, selection, state_hash)
            }
            crate::control::TopLevelContext::Patch => {
                let page = page.ok_or(StateProjectionError::InvalidSelection)?;
                if page.state_hash() != state_hash {
                    return Err(StateProjectionError::InvalidSelection);
                }
                render_patch_text(page, state_hash)
            }
        }
    }

    fn graphical_shell_from_serialized(
        &self,
        state: &SerializedState<'_>,
        page: Option<&PatchPageProjection>,
        diagnostic: &TextProjection,
        state_hash: &str,
        semantic: &SemanticGraphicalViewModel,
    ) -> Result<GraphicalShellProjection, StateProjectionError> {
        if diagnostic.context() != state.interaction.active_focus.context()
            || diagnostic.state_hash() != state_hash
        {
            return Err(StateProjectionError::InvalidSelection);
        }

        let context = semantic.context();
        if semantic.generation() != state.generation
            || semantic.state_hash() != state_hash
            || semantic.context() != state.interaction.active_focus.context()
        {
            return Err(StateProjectionError::InvalidSelection);
        }
        // NFR-005, across the projections rather than within one of them: the
        // canonical snapshot and the PATCH page assembled beside it must name
        // the same Patch. A disagreement here is a projection set built from
        // two accepted states, which is exactly the intermediate a patch switch
        // must never produce.
        //
        // Only the page is compared. `semantic.patch_identity()` resolves to
        // `state.interaction.active_focus.patch_id()` — literally the
        // expression on the line below — so checking it compared a value to
        // itself and could never fire. The model's identity is guarded where it
        // can actually be wrong: `validate_data` refuses to build a model whose
        // focus path, surface summaries, and control paths disagree, and the
        // generation and state-hash equality directly above pins that model to
        // this snapshot. The page is the one projection assembled on a separate
        // path (it derives from `patch_focus()`), so it is the one that needs a
        // cross-check.
        let snapshot_patch = state.interaction.active_focus.patch_id();
        if page.is_some_and(|page| page.patch().id() != snapshot_patch) {
            return Err(StateProjectionError::InvalidSelection);
        }
        let settings_active = semantic.active_surface().is_system();
        let status_label = if settings_active {
            "WATCHING"
        } else {
            semantic.status().label()
        };
        let context_label = if settings_active {
            "SETTINGS"
        } else {
            context.label()
        };
        let context_line = ShellContextLine::new("CREST SYNTH", context_label, status_label);
        let action_hints = semantic
            .valid_actions()
            .iter()
            .map(|valid| match valid.hint() {
                Some(hint) => format!("{hint} {}", valid.label()).to_ascii_uppercase(),
                None => valid.label().to_ascii_uppercase(),
            })
            .collect::<Vec<_>>();

        let (identity_header, main_label, side_label, footer) = if settings_active {
            (
                ShellIdentityHeader::new(
                    format!("SETTINGS · {}", semantic.active_surface().label()),
                    if semantic.active_surface() == crate::control::SurfaceId::SaveLoadSettings {
                        "SESSION FILES"
                    } else {
                        "PHYSICAL INPUT"
                    },
                ),
                if semantic.active_surface() == crate::control::SurfaceId::ControllerSettings {
                    "BUTTON ASSIGNMENTS"
                } else if semantic.active_surface() == crate::control::SurfaceId::SaveLoadSettings {
                    "SESSION FILES"
                } else {
                    "AVAILABLE INPUTS"
                }
                .to_owned(),
                if semantic.active_surface() == crate::control::SurfaceId::ControllerSettings {
                    "CONTROLLER INSPECTOR"
                } else if semantic.active_surface() == crate::control::SurfaceId::SaveLoadSettings {
                    "CURRENT SESSION"
                } else {
                    "INPUT INSPECTOR"
                }
                .to_owned(),
                ShellFooter::new(footer_path_label(semantic), action_hints),
            )
        } else {
            match context {
                crate::control::TopLevelContext::Patch => {
                    let page = page.ok_or(StateProjectionError::InvalidSelection)?;
                    if page.context() != context || page.state_hash() != state_hash {
                        return Err(StateProjectionError::InvalidSelection);
                    }
                    let patch = page.patch();
                    let (primary, secondary) = match (patch.id(), patch.midi_channel()) {
                        (Some(patch_id), Some(channel)) => (
                            format!("PATCH {:02} · {}", patch_id.value(), patch.name()),
                            format!(
                                "MIDI CH {:02} · {}",
                                u16::from(channel.value()) + 1,
                                page.engine().active_label()
                            ),
                        ),
                        (None, None) if patch.is_empty() => (
                            "NEW PATCH · EMPTY".to_owned(),
                            format!(
                                "{} · CAPACITY {}/{}",
                                page.engine().active_label(),
                                patch.active_count(),
                                patch.capacity()
                            ),
                        ),
                        _ => return Err(StateProjectionError::InvalidSelection),
                    };
                    match semantic.focus_path().control_id() {
                        SemanticControlId::Patch(_) | SemanticControlId::Modal(_) => {}
                        SemanticControlId::SurfaceRoot
                            if semantic.active_surface()
                                == crate::control::SurfaceId::PatchUtility => {}
                        SemanticControlId::Send(_)
                        | SemanticControlId::Mixer(_)
                        | SemanticControlId::MidiInputDevice(_)
                        | SemanticControlId::ControllerSetting(_)
                        | SemanticControlId::SessionFileAction(_)
                        | SemanticControlId::MidiInputListRoot
                        | SemanticControlId::SurfaceRoot => {
                            return Err(StateProjectionError::InvalidSelection)
                        }
                    }
                    let path = footer_path_label(semantic);
                    (
                        ShellIdentityHeader::new(primary, secondary),
                        format!("PATCH WORKSPACE · {}", patch.name()),
                        "UTILITY".to_owned(),
                        ShellFooter::new(path, action_hints),
                    )
                }
                crate::control::TopLevelContext::Mixer => {
                    if page.is_some() {
                        return Err(StateProjectionError::InvalidSelection);
                    }
                    let routed_for = |track_id: &crate::mixer::mixer_track_id::MixerTrackId| {
                        state
                            .patches
                            .iter()
                            .filter(|patch| patch.output.track_id() == *track_id)
                            .map(|patch| format!("{:02}", patch.id))
                            .collect::<Vec<_>>()
                            .join(", ")
                    };
                    let identity_path =
                        if semantic.active_surface() == crate::control::SurfaceId::FileBrowser {
                            semantic
                                .return_path()
                                .ok_or(StateProjectionError::InvalidSelection)?
                                .origin()
                        } else {
                            semantic.focus_path()
                        };
                    let (primary, secondary) = match identity_path.control_id() {
                        SemanticControlId::Send(control) => (
                            format!("SEND {:02}", control.bus().index() + 1),
                            state
                                .returns
                                .entries()
                                .get(control.bus().index())
                                .map(|send| send.name.clone())
                                .unwrap_or_default(),
                        ),
                        SemanticControlId::Mixer(
                            MixerControlId::Track { track_id, .. }
                            | MixerControlId::Send { track_id, .. },
                        ) => (
                            format!("MIXER · {track_id}"),
                            format!("ROUTED PATCHES · {}", routed_for(track_id)),
                        ),
                        SemanticControlId::Mixer(
                            MixerControlId::ReturnOccupancy { .. }
                            | MixerControlId::ReturnLevel { .. }
                            | MixerControlId::ReturnEffect { .. },
                        ) => (
                            "MIXER · RETURNS".to_owned(),
                            format!("{} PATCHES · BUS RETURNS", state.patches.len()),
                        ),
                        SemanticControlId::Mixer(MixerControlId::Global { .. }) => (
                            "MIXER · GLOBAL".to_owned(),
                            format!("{} PATCHES · MASTER OUTPUT", state.patches.len()),
                        ),
                        SemanticControlId::SurfaceRoot => (
                            "MIXER · INSPECTOR".to_owned(),
                            format!("{} PATCHES · READ ONLY", state.patches.len()),
                        ),
                        SemanticControlId::Patch(_) => {
                            return Err(StateProjectionError::InvalidSelection)
                        }
                        SemanticControlId::Modal(_) => {
                            return Err(StateProjectionError::InvalidSelection)
                        }
                        SemanticControlId::MidiInputDevice(_)
                        | SemanticControlId::ControllerSetting(_)
                        | SemanticControlId::SessionFileAction(_)
                        | SemanticControlId::MidiInputListRoot => {
                            return Err(StateProjectionError::InvalidSelection)
                        }
                    };
                    let path = footer_path_label(semantic);
                    (
                        ShellIdentityHeader::new(primary, secondary),
                        "MIXER WORKSPACE".to_owned(),
                        "INSPECTOR".to_owned(),
                        ShellFooter::new(path, action_hints),
                    )
                }
            }
        };

        GraphicalShellProjection::new(
            state.generation,
            state_hash,
            semantic.clone(),
            context_line,
            identity_header,
            main_label,
            side_label,
            diagnostic.clone(),
            footer,
        )
        .map_err(StateProjectionError::from)
    }
}

/// The shell footer's breadcrumb: the top-level context, then the **authored
/// label** of the row the cursor is on.
///
/// This used to be composed per control-identity variant out of the identity's
/// own serialization key — `"MIXER / GLOBAL / masterGainDb"`,
/// `"PATCH / patch.voiceLimit"` — which is exactly the defect T016 exists to
/// close, one layer up from the row labels it closed it on. It was harmless
/// only by accident: the page ignores `pathLabel` and composes its own
/// breadcrumb from `control.label`. It stops being harmless the moment anything
/// reads the field.
///
/// The per-variant match is gone rather than repaired arm by arm. There is one
/// composition, one source — [`SemanticGraphicalViewModel::focused_control`] —
/// and no place left for a key to be spelled, so a future control identity
/// cannot reintroduce the defect by adding an arm. A focus resting on a surface
/// root owns no row, so it names the surface, whose label is authored too.
fn footer_path_label(semantic: &SemanticGraphicalViewModel) -> String {
    let leaf = semantic.focused_control().map_or_else(
        || semantic.active_surface().label().to_owned(),
        |control| control.label().to_owned(),
    );
    let root = if semantic.active_surface().is_system() {
        "SETTINGS"
    } else if semantic.surface(crate::control::SurfaceId::Sends).is_some() {
        "SENDS"
    } else {
        semantic.context().label()
    };
    format!("{root} / {leaf}")
}

fn selection_from_serialized(
    state: &SerializedState<'_>,
) -> Result<Selection, StateProjectionError> {
    match state.interaction.remembered_mixer_main.control_id() {
        SemanticControlId::Mixer(MixerControlId::Track {
            track_id,
            parameter,
        }) => {
            let parameter_index = crate::mixer::mixer_track_parameters::MixerTrackParameter::MAIN
                .iter()
                .position(|candidate| candidate == parameter)
                .ok_or(StateProjectionError::InvalidSelection)?;
            Ok(Selection {
                section: SelectionSection::Patch,
                patch_index: track_id.index(),
                parameter_index,
            })
        }
        SemanticControlId::Mixer(_)
        | SemanticControlId::Patch(_)
        | SemanticControlId::Send(_)
        | SemanticControlId::Modal(_)
        | SemanticControlId::MidiInputDevice(_)
        | SemanticControlId::ControllerSetting(_)
        | SemanticControlId::SessionFileAction(_)
        | SemanticControlId::MidiInputListRoot
        | SemanticControlId::SurfaceRoot => Err(StateProjectionError::InvalidSelection),
    }
}

fn render_mixer_text(
    state: &SerializedState<'_>,
    _selection: Selection,
    state_hash: &str,
) -> Result<TextProjection, StateProjectionError> {
    let mut lines = vec![MIXER_HEADER.to_owned()];
    let mut selected_line = None;
    let active = &state.interaction.active_focus;
    for track_id in crate::mixer::mixer_track_id::MixerTrackId::ALL {
        if track_id.index() > 0 {
            lines.push(SEPARATOR.to_owned());
        }
        let routed = state
            .patches
            .iter()
            .filter(|patch| patch.output.track_id() == track_id)
            .map(|patch| format!("{:02}:{}", patch.id, patch.name))
            .collect::<Vec<_>>();
        lines.push(format!(
            "TRACK {track_id} routedPatches=[{}]",
            routed.join(",")
        ));
        let values = state.mixer.track(track_id).clone();
        for parameter in crate::mixer::mixer_track_parameters::MixerTrackParameter::MAIN {
            let descriptor = parameter.descriptor();
            let path = crate::control::FocusPath::mixer_track(track_id, parameter);
            let selected = active == &path;
            let value = values
                .scalar_value(parameter)
                .map(|value| value.to_string())
                .or_else(|| {
                    values
                        .toggle_value(parameter)
                        .map(|value| value.to_string())
                })
                .ok_or(StateProjectionError::InvalidSelection)?;
            push_parameter_text_line(
                &mut lines,
                &mut selected_line,
                selected,
                descriptor.name(),
                &value,
            );
        }
        // All eight indexed sends in ascending BusId order.
        for bus in (0..state.returns.entries().len())
            .filter_map(|index| crate::mixer::bus_id::BusId::new(index as u16).ok())
        {
            let path = crate::control::FocusPath::mixer_send(track_id, bus);
            push_parameter_line(
                &mut lines,
                &mut selected_line,
                active == &path,
                &format!("send[{}]", bus.index()),
                values.send(bus),
            );
        }
    }

    lines.push(SEPARATOR.to_owned());
    lines.push("RETURNS".to_owned());
    for bus in (0..state.returns.entries().len())
        .filter_map(|index| crate::mixer::bus_id::BusId::new(index as u16).ok())
    {
        let entry = state
            .returns
            .entries()
            .get(bus.index())
            .ok_or(StateProjectionError::InvalidSelection)?;
        lines.push(format!("RETURN {bus}"));
        let occupancy = entry.effect.as_ref().map_or_else(
            || "empty".to_owned(),
            |config| config.capability_id().to_string(),
        );
        push_parameter_text_line(
            &mut lines,
            &mut selected_line,
            active == &crate::control::FocusPath::mixer_return_occupancy(bus),
            "occupancy",
            &occupancy,
        );
        push_parameter_line(
            &mut lines,
            &mut selected_line,
            active == &crate::control::FocusPath::mixer_return_level(bus),
            "returnLevel",
            entry.return_level,
        );
        let Some(config) = entry.effect.as_ref() else {
            continue;
        };
        let descriptor = state
            .effects
            .descriptor(config.capability_id())
            .ok_or(StateProjectionError::InvalidEffectConfig)?;
        let predicate_satisfied = |predicate: Option<&crate::synth::ParameterPredicate>| {
            predicate.is_none_or(|predicate| {
                config.value(predicate.parameter_id()) == Some(predicate.equals())
            })
        };
        for spec in descriptor.parameters().filter(|spec| {
            spec.patch_interaction() == crate::synth::PatchInteraction::ScalarEdit
                && predicate_satisfied(spec.visible_when())
                && predicate_satisfied(spec.enabled_when())
        }) {
            let value = config
                .value(spec.id())
                .and_then(|value| spec.scalar_value(value).ok())
                .ok_or(StateProjectionError::InvalidEffectConfig)?;
            let path = crate::control::FocusPath::mixer_return_effect(
                bus,
                spec.id().clone(),
                config.capability_id().clone(),
            );
            push_parameter_line(
                &mut lines,
                &mut selected_line,
                active == &path,
                spec.id().as_str(),
                value,
            );
        }
    }

    lines.push(SEPARATOR.to_owned());
    lines.push("GLOBAL".to_owned());

    for descriptor in crate::mixer::global_parameters::GlobalParameters::surface_descriptor() {
        let parameter = descriptor.parameter();
        let selected = active == &crate::control::FocusPath::mixer_global(parameter);
        push_parameter_line(
            &mut lines,
            &mut selected_line,
            selected,
            parameter.name(),
            state.global.master_gain_db,
        );
    }

    let selected_line = selected_line.ok_or(StateProjectionError::InvalidSelection)?;
    Ok(TextProjection::for_context(
        crate::control::TopLevelContext::Mixer,
        lines.join("\n"),
        selected_line,
        state_hash.to_owned(),
    ))
}

fn render_patch_text(
    page: &PatchPageProjection,
    state_hash: &str,
) -> Result<TextProjection, StateProjectionError> {
    let mut lines = vec![PATCH_HEADER.to_owned()];
    let mut selected_line = None;
    lines.push(format!(
        " PATCH {}",
        serde_json::to_string(page.patch())
            .map_err(|_| StateProjectionError::StateSerialization)?
    ));
    for row in page.output() {
        let selected = row.control_id() == page.focused_control_id();
        if selected {
            selected_line = Some(lines.len());
        }
        let marker = if selected { '>' } else { ' ' };
        lines.push(format!(
            "{marker} OUTPUT {}",
            serde_json::to_string(row).map_err(|_| StateProjectionError::StateSerialization)?
        ));
    }
    let engine_selected = page.engine().control_id() == page.focused_control_id();
    if engine_selected {
        selected_line = Some(lines.len());
    }
    let engine_marker = if engine_selected { '>' } else { ' ' };
    lines.push(format!(
        "{engine_marker} ENGINE {}",
        serde_json::to_string(page.engine())
            .map_err(|_| StateProjectionError::StateSerialization)?
    ));
    for row in page.envelope() {
        let selected = row.control_id() == page.focused_control_id();
        if selected {
            selected_line = Some(lines.len());
        }
        let marker = if selected { '>' } else { ' ' };
        lines.push(format!(
            "{marker} ENVELOPE {}",
            serde_json::to_string(row).map_err(|_| StateProjectionError::StateSerialization)?
        ));
    }
    for section in page.sections() {
        lines.push(format!(
            " SECTION id={} label={}",
            section.id(),
            section.label()
        ));
        for row in section.parameters() {
            let selected = row.control_id() == Some(page.focused_control_id());
            if selected {
                selected_line = Some(lines.len());
            }
            let marker = if selected { '>' } else { ' ' };
            lines.push(format!(
                "{marker} PARAMETER {}",
                serde_json::to_string(row).map_err(|_| StateProjectionError::StateSerialization)?
            ));
        }
    }
    for slot in page.effects() {
        #[derive(serde::Serialize)]
        #[serde(rename_all = "camelCase")]
        struct SlotHeader<'a> {
            slot_index: crate::synth::effect_slot_id::EffectSlotIndex,
            occupancy_control_id: crate::control::PatchControlId,
            occupancy: &'a crate::control::PatchPageSlotOccupancy,
            choices: &'a [crate::control::PatchPageOccupancyChoice],
            requested_choice_id: Option<&'a str>,
            status: Option<crate::control::EngineSelectionStatusKind>,
            failure: Option<crate::control::EngineSelectionFailure>,
            editable: bool,
        }
        let selected = slot.occupancy_control_id() == page.focused_control_id();
        if selected {
            selected_line = Some(lines.len());
        }
        let marker = if selected { '>' } else { ' ' };
        let header = SlotHeader {
            slot_index: slot.slot_index(),
            occupancy_control_id: slot.occupancy_control_id(),
            occupancy: slot.occupancy(),
            choices: slot.choices(),
            requested_choice_id: slot.requested_choice_id(),
            status: slot.status(),
            failure: slot.failure(),
            editable: slot.editable(),
        };
        lines.push(format!(
            "{marker} EFFECT_SLOT {}",
            serde_json::to_string(&header).map_err(|_| StateProjectionError::StateSerialization)?
        ));
        for section in slot.sections() {
            lines.push(format!(
                " EFFECT_SECTION id={} label={}",
                section.id(),
                section.label()
            ));
            for row in section.parameters().iter().filter(|row| row.visible()) {
                let selected = row.control_id() == Some(page.focused_control_id());
                if selected {
                    selected_line = Some(lines.len());
                }
                let marker = if selected { '>' } else { ' ' };
                lines.push(format!(
                    "{marker} EFFECT_PARAMETER {}",
                    serde_json::to_string(row)
                        .map_err(|_| StateProjectionError::StateSerialization)?
                ));
            }
        }
    }

    // The open detail entry, rendered from the same page value the graphical
    // projection reads. A detail focus lands on a row that exists in exactly
    // one place — Braids' detail rows are absent from its main order entirely —
    // so without these lines a detail focus has nothing to select and the whole
    // projection fails on an accepted state.
    if let Some(detail) = page.detail() {
        lines.push(format!(
            " DETAIL {}",
            serde_json::to_string(&DetailHeader {
                subject: detail.subject(),
                label: detail.label(),
                status: detail.status(),
            })
            .map_err(|_| StateProjectionError::StateSerialization)?
        ));
        for section in detail.sections() {
            lines.push(format!(
                " DETAIL_SECTION id={} label={}",
                section.id(),
                section.label()
            ));
            for row in section.parameters().iter().filter(|row| row.visible()) {
                let selected = row.control_id() == Some(page.focused_control_id());
                if selected {
                    selected_line = Some(lines.len());
                }
                let marker = if selected { '>' } else { ' ' };
                lines.push(format!(
                    "{marker} DETAIL_PARAMETER {}",
                    serde_json::to_string(row)
                        .map_err(|_| StateProjectionError::StateSerialization)?
                ));
            }
        }
    }

    let selected_line = selected_line.ok_or(StateProjectionError::InvalidSelection)?;

    Ok(TextProjection::for_context(
        crate::control::TopLevelContext::Patch,
        lines.join("\n"),
        selected_line,
        state_hash.to_owned(),
    ))
}

/// The detail entry's own header line: which capability filled the surface and
/// what its lifecycle is doing, without a second copy of its schema.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct DetailHeader<'a> {
    subject: &'a crate::control::PatchDetailSubject,
    label: &'a str,
    status: crate::control::EngineSelectionStatusKind,
}

fn push_parameter_line(
    lines: &mut Vec<String>,
    selected_line: &mut Option<usize>,
    selected: bool,
    name: &str,
    value: f32,
) {
    push_parameter_text_line(lines, selected_line, selected, name, &value.to_string());
}

fn push_parameter_text_line(
    lines: &mut Vec<String>,
    selected_line: &mut Option<usize>,
    selected: bool,
    name: &str,
    value: &str,
) {
    let marker = if selected { '>' } else { ' ' };
    if selected {
        *selected_line = Some(lines.len());
    }
    lines.push(format!("{marker} {name}={value}"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::braids_capability::{BraidsCapability, BRAIDS_CAPABILITY_ID};
    use crate::adapter::hidef_soundfont_capability::{
        HIDEF_CAPABILITY_ID, SOUNDFONT_PRESET_PARAMETER_ID,
    };
    use crate::adapter::production_instruments::production_capability_registry;
    use crate::control::app_event::{AppEvent, Direction};
    use crate::control::app_state::EventRejection;
    use crate::control::serialized_state::SerializedState;
    use crate::control::{EngineSelectionFailure, EngineSelectionStatusKind, MixerControlId};
    use crate::kernel::midi_channel::MidiChannel;
    use crate::kernel::midi_message::{MidiMessage, MidiMessageKind};
    use crate::kernel::patch_id::PatchId;
    use crate::mixer::global_parameters::GlobalParameters;
    use crate::mixer::mixer_track_id::MixerTrackId;
    use crate::mixer::mixer_track_parameters::MixerTrackParameter;
    use crate::mixer::patch_output::PatchOutput;
    use crate::real_time::MAX_ACTIVE_PATCHES;
    use crate::synth::patch::Patch;
    use crate::synth::sound_font_instrument::SoundFontInstrument;
    use crate::synth::{InstrumentConfig, ParameterValue, PatchInteraction};
    use crate::testing::automatic_midi_test::create_soundfont_config;

    fn global_parameters() -> GlobalParameters {
        GlobalParameters::new(-3.0).unwrap()
    }

    fn patch(id: u32, gain_db: f32) -> Patch {
        let provider =
            crate::adapter::production_instruments::production_soundfont_capability().unwrap();
        Patch::new(
            PatchId::new(id).unwrap(),
            format!("Patch {id}"),
            create_soundfont_config(
                &provider,
                SoundFontInstrument::new(128, ((id - 1) % 2) as u8, (id & 1) == 0).unwrap(),
            )
            .unwrap(),
            MidiChannel::new(((id - 1) % 16) as u8).unwrap(),
            PatchOutput::new(MixerTrackId::new(((id - 1) % 16) as u8).unwrap(), gain_db).unwrap(),
        )
    }

    fn installed_state_for_graph(graph_revision: GraphRevision) -> AppState {
        let provider =
            crate::adapter::production_instruments::production_soundfont_capability().unwrap();
        let mut state = AppState::for_graph(
            provider.registry().unwrap(),
            global_parameters(),
            graph_revision,
        );
        state
            .apply(AppEvent::InstallPatches(vec![
                patch(1, -6.0),
                patch(2, -12.0),
            ]))
            .unwrap();
        state
    }

    fn installed_state() -> AppState {
        installed_state_for_graph(GraphRevision::INITIAL)
    }

    fn mixed_patch_state() -> AppState {
        mixed_patch_state_with_config(patch(1, -6.0).instrument_config().clone())
    }

    fn mixed_patch_state_with_config(config: InstrumentConfig) -> AppState {
        let mut state = AppState::new(
            production_capability_registry().unwrap(),
            global_parameters(),
        );
        let patch = Patch::new(
            PatchId::new(1).unwrap(),
            "Patch 1".to_owned(),
            config,
            MidiChannel::new(0).unwrap(),
            PatchOutput::new(MixerTrackId::default(), -6.0).unwrap(),
        )
        .with_envelope(crate::synth::VoiceEnvelope::new(12.0, 34.0, 0.56, 78.0).unwrap());
        state.apply(AppEvent::InstallPatches(vec![patch])).unwrap();
        state
            .apply(AppEvent::SelectContext(
                crate::control::TopLevelContext::Patch,
            ))
            .unwrap();
        state
    }

    #[test]
    fn serialization_is_deterministic_and_round_trips_every_field() {
        let state = installed_state();
        let projector = StateProjector::new();
        let first = projector.state_snapshot(&state).unwrap();
        let second = projector.state_snapshot(&state).unwrap();
        let decoded: SerializedState = serde_json::from_str(first.json()).unwrap();

        assert_eq!(first, second);
        assert_eq!(decoded, SerializedState::from(&state));
        assert_eq!(decoded.generation, 1);
        assert_eq!(decoded.patches.len(), 2);
        assert_eq!(decoded.patches[0].name, "Patch 1");
        assert_eq!(decoded.capabilities.descriptors().len(), 1);
        assert_eq!(
            decoded.patches[0].instrument.capability_id().as_str(),
            HIDEF_CAPABILITY_ID
        );
        assert_eq!(
            decoded.patches[0]
                .instrument
                .value(&crate::synth::ParameterId::new(SOUNDFONT_PRESET_PARAMETER_ID).unwrap()),
            Some(&ParameterValue::Choice(
                SoundFontInstrument::new(128, 0, false)
                    .unwrap()
                    .preset_id()
                    .choice_id()
            ))
        );
        assert_eq!(decoded.patches[0].output.trim_gain_db(), -6.0);
        assert!(decoded.global.master_gain_db.is_finite());
        assert!(decoded.returns.is_complete());
        assert_eq!(
            decoded.interaction.remembered_mixer_main.control_id(),
            &SemanticControlId::Mixer(MixerControlId::Track {
                track_id: MixerTrackId::default(),
                parameter: MixerTrackParameter::Level,
            })
        );
    }

    #[test]
    fn text_is_derived_from_the_snapshot_and_typed_selection() {
        let mut state = installed_state();
        let projector = StateProjector::new();
        let snapshot = projector.state_snapshot(&state).unwrap();

        state.apply(AppEvent::Adjust(Direction::Right)).unwrap();
        let text = projector
            .text_projection(&snapshot, Selection::patch(0))
            .unwrap();

        assert!(text.body().starts_with(MIXER_HEADER));
        assert!(text.body().contains("TRACK T00 routedPatches=[01:Patch 1]"));
        assert!(text.body().contains("TRACK T0F routedPatches=[]"));
        assert!(text.body().contains("> levelDb=0"));
        assert!(!text.body().contains("levelDb=1"));
        assert!(text.body().contains(SEPARATOR));
        assert!(text.body().contains("GLOBAL"));
        assert_eq!(
            text.body()
                .lines()
                .position(|line| line.starts_with("> levelDb=")),
            Some(text.selected_line())
        );
        assert_eq!(text.state_hash(), snapshot.hash());
    }

    #[test]
    fn rejects_a_typed_selection_from_a_different_snapshot() {
        let state = installed_state();
        let projector = StateProjector::new();
        let snapshot = projector.state_snapshot(&state).unwrap();

        assert_eq!(
            projector.text_projection(&snapshot, Selection::global()),
            Err(StateProjectionError::SelectionDoesNotMatchSnapshot)
        );
    }

    #[test]
    fn state_projector_exact_projection_values() {
        let state = installed_state();
        let snapshot = StateProjector::new().parameter_snapshot(&state).unwrap();

        assert_eq!(snapshot.generation(), state.generation());
        assert_eq!(snapshot.global(), state.global());
        assert_eq!(snapshot.mixer_tracks(), state.mixer().tracks());
        assert_eq!(snapshot.patch_count(), state.patches().len());
        for (projected, patch) in snapshot.patches().iter().zip(state.patches()) {
            assert_eq!(projected.patch_id(), Some(patch.id()));
            assert_eq!(projected.output(), patch.output());
        }
    }

    #[test]
    fn rejects_oversized_patch_installation_before_projection() {
        let provider =
            crate::adapter::production_instruments::production_soundfont_capability().unwrap();
        let mut state = AppState::new(provider.registry().unwrap(), global_parameters());
        let patches = (1..=(MAX_ACTIVE_PATCHES as u32 + 1))
            .map(|id| patch(id, -6.0))
            .collect();

        assert_eq!(
            state.apply(AppEvent::InstallPatches(patches)),
            Err(EventRejection::TooManyPatches)
        );
        assert_eq!(state.generation(), 0);
        assert!(state.patches().is_empty());

        let snapshot = StateProjector::new().parameter_snapshot(&state).unwrap();
        assert_eq!(snapshot.generation(), state.generation());
        assert_eq!(snapshot.patch_count(), 0);
    }

    #[test]
    fn complete_projection_uses_one_accepted_generation() {
        let revision = GraphRevision::new(17).unwrap();
        let state = installed_state_for_graph(revision);
        let (snapshot, page, text, shell, parameters, tree) = StateProjector::for_graph(revision)
            .project_with_shell_tree(&state)
            .unwrap();

        assert!(page.is_none());
        assert_eq!(text.state_hash(), snapshot.hash());
        assert_eq!(shell.generation(), state.generation());
        assert_eq!(shell.state_hash(), snapshot.hash());
        assert_eq!(shell.workspace().diagnostic(), &text);
        assert_eq!(parameters.generation(), state.generation());
        assert_eq!(parameters.graph_revision(), revision);
        assert_eq!(tree.generation(), parameters.generation());
        assert_eq!(tree.graph_revision(), revision);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(tree.json()).unwrap()["parameters"]
                ["graphRevision"],
            revision.value()
        );
        assert_eq!(tree.state_hash(), snapshot.hash());
        assert_eq!(tree.selected_line(), text.selected_line());
        assert_eq!(tree.patch_count(), parameters.patch_count());
        assert_eq!(
            serde_json::from_str::<SerializedState>(snapshot.json())
                .unwrap()
                .generation,
            parameters.generation()
        );
    }

    #[test]
    fn midi_generation_projection_is_exactly_equal_to_eager_projection() {
        let revision = GraphRevision::new(23).unwrap();
        let mut state = installed_state_for_graph(revision);
        let projector = StateProjector::for_graph(revision);
        let (snapshot, page, text, shell, parameters, tree) =
            projector.project_with_shell_tree(&state).unwrap();
        let patch_id = state.patches()[0].id();
        let message = MidiMessage::try_new(
            state.patches()[0].channel(),
            MidiMessageKind::NoteOn,
            60,
            100,
        )
        .unwrap();
        state.apply(AppEvent::Midi { patch_id, message }).unwrap();

        let fast = projector
            .project_midi_generation(
                &state,
                MidiProjectionSeed::new(&snapshot, page.as_ref(), &text, &shell, parameters, &tree),
            )
            .unwrap();
        let eager = projector.project_with_shell_tree(&state).unwrap();

        assert_eq!(fast.0, eager.0);
        assert_eq!(fast.1, eager.1);
        assert_eq!(fast.2, eager.2);
        assert_eq!(fast.3, eager.3);
        assert_eq!(fast.4, eager.4);
        assert_eq!(fast.5, eager.5);
        assert_eq!(fast.4.graph_revision(), revision);
        assert_eq!(fast.5.graph_revision(), revision);
    }

    #[test]
    fn patch_projection_and_generation_only_midi_are_context_coherent_and_shared() {
        let revision = GraphRevision::new(29).unwrap();
        let mut state = installed_state_for_graph(revision);
        state
            .apply(AppEvent::SelectContext(
                crate::control::TopLevelContext::Patch,
            ))
            .unwrap();
        let projector = StateProjector::for_graph(revision);
        let (snapshot, page, text, shell, parameters, tree) =
            projector.project_with_shell_tree(&state).unwrap();
        let page = page.expect("PATCH context projects one focused page");

        assert_eq!(page.patch().id(), Some(state.patches()[0].id()));
        assert_eq!(page.state_hash(), snapshot.hash());
        assert_eq!(text.context(), crate::control::TopLevelContext::Patch);
        assert!(text.body().starts_with(PATCH_HEADER));
        assert_eq!(text.state_hash(), snapshot.hash());
        let tree_value: serde_json::Value = serde_json::from_str(tree.json()).unwrap();
        assert_eq!(tree_value["interaction"]["activeFocus"]["context"], "patch");
        assert_eq!(tree_value["projection"]["context"], "patch");
        assert_eq!(
            tree_value["patchPage"]["patch"]["id"],
            page.patch().id().unwrap().value()
        );

        let body_address = text.body().as_ptr();
        let message = MidiMessage::try_new(
            state.patches()[0].channel(),
            MidiMessageKind::NoteOn,
            60,
            100,
        )
        .unwrap();
        state
            .apply(AppEvent::Midi {
                patch_id: state.patches()[0].id(),
                message,
            })
            .unwrap();
        let fast = projector
            .project_midi_generation(
                &state,
                MidiProjectionSeed::new(&snapshot, Some(&page), &text, &shell, parameters, &tree),
            )
            .unwrap();
        let eager = projector.project_with_shell_tree(&state).unwrap();

        assert_eq!(fast, eager);
        let fast_page = fast.1.as_ref().unwrap();
        assert!(page.shares_content_with(fast_page));
        assert_eq!(fast.2.body().as_ptr(), body_address);
        assert_eq!(fast_page.state_hash(), fast.0.hash());
        assert_eq!(fast.2.state_hash(), fast.0.hash());
        assert_eq!(fast.3.generation(), state.generation());
        assert_eq!(fast.4.generation(), state.generation());
        assert_eq!(fast.5.generation(), state.generation());
    }

    #[test]
    fn patch_text_marker_and_selected_line_follow_every_canonical_control() {
        let mut state = mixed_patch_state();
        let controls = crate::control::SemanticResolver::new(&state)
            .patch_main_paths(state.interaction().patch_focus().unwrap())
            .unwrap()
            .into_iter()
            .filter_map(|path| match path.control_id() {
                crate::control::SemanticControlId::Patch(control) => Some(control.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        for (index, expected_control) in controls.iter().cloned().enumerate() {
            let (_, page, text, _, _) = StateProjector::new().project_with_tree(&state).unwrap();
            let page = page.unwrap();
            assert_eq!(page.focused_control_id(), expected_control);
            assert_eq!(
                text.body()
                    .lines()
                    .filter(|line| line.starts_with('>'))
                    .count(),
                1
            );
            assert!(text
                .body()
                .lines()
                .nth(text.selected_line())
                .unwrap()
                .contains(expected_control.as_str().as_ref()));

            if index + 1 < controls.len() {
                state.apply(AppEvent::Navigate(Direction::Down)).unwrap();
            }
        }

        let mut preparing = mixed_patch_state();
        preparing.apply(AppEvent::Adjust(Direction::Right)).unwrap();
        let (_, page, text, _, _) = StateProjector::new().project_with_tree(&preparing).unwrap();
        let page = page.unwrap();
        assert_eq!(
            page.focused_control_id(),
            crate::control::PatchControlId::Engine
        );
        assert!(!page.engine().editable());
        assert!(text
            .body()
            .lines()
            .nth(text.selected_line())
            .unwrap()
            .starts_with("> ENGINE"));
    }

    #[test]
    fn patch_projection_is_generic_for_both_engines_every_focus_and_context_round_trip() {
        let soundfont = patch(1, -6.0).instrument_config().clone();
        let braids = BraidsCapability::new().unwrap().default_config().unwrap();

        for config in [soundfont, braids] {
            let mut state = mixed_patch_state_with_config(config);
            let descriptor = state
                .capabilities()
                .descriptor(state.patches()[0].instrument_config().capability_id())
                .unwrap()
                .clone();
            let controls = crate::control::SemanticResolver::new(&state)
                .patch_main_paths(state.interaction().patch_focus().unwrap())
                .unwrap()
                .into_iter()
                .filter_map(|path| match path.control_id() {
                    crate::control::SemanticControlId::Patch(control) => Some(control.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>();

            for (index, control) in controls.iter().cloned().enumerate() {
                let (_, page, text, _, tree) =
                    StateProjector::new().project_with_tree(&state).unwrap();
                let page = page.unwrap();
                assert_eq!(page.focused_control_id(), control);
                assert_eq!(
                    text.body()
                        .lines()
                        .filter(|line| line.starts_with('>'))
                        .count(),
                    1
                );
                assert_eq!(page.sections().len(), descriptor.sections().len());
                for (section, section_spec) in page.sections().iter().zip(descriptor.sections()) {
                    assert_eq!(section.id(), section_spec.id());
                    for (row, spec) in section.parameters().iter().zip(section_spec.parameters()) {
                        assert_eq!(row.id(), spec.id());
                        assert!(text.body().contains(spec.id().as_str()));
                        assert_eq!(
                            row.editable(),
                            spec.patch_interaction() == PatchInteraction::StructuralChoice
                        );
                    }
                }
                let tree_json: serde_json::Value = serde_json::from_str(tree.json()).unwrap();
                assert_eq!(
                    tree_json["patchPage"]["focusedControlId"],
                    control.as_str().as_ref()
                );

                if index + 1 < controls.len() {
                    state.apply(AppEvent::Navigate(Direction::Down)).unwrap();
                }
            }

            let retained = state.interaction().patch_control_focus();
            let retained_line = StateProjector::new()
                .project_with_tree(&state)
                .unwrap()
                .2
                .selected_line();
            state
                .apply(AppEvent::SelectContext(
                    crate::control::TopLevelContext::Mixer,
                ))
                .unwrap();
            assert!(StateProjector::new()
                .project_with_tree(&state)
                .unwrap()
                .1
                .is_none());
            state
                .apply(AppEvent::SelectContext(
                    crate::control::TopLevelContext::Patch,
                ))
                .unwrap();
            let (_, page, text, _, _) = StateProjector::new().project_with_tree(&state).unwrap();
            assert_eq!(page.unwrap().focused_control_id(), retained.unwrap());
            assert_eq!(text.selected_line(), retained_line);
        }
    }

    #[test]
    fn every_engine_selection_generation_is_cross_projection_coherent() {
        let projector = StateProjector::new();
        let mut state = mixed_patch_state();
        let source_config = state.patches()[0].instrument_config().clone();
        let source_revision = GraphRevision::INITIAL;

        let assert_projection =
            |state: &AppState,
             expected_kind: EngineSelectionStatusKind,
             expected_projection_revision: GraphRevision,
             expected_editable: bool,
             expected_failure: Option<EngineSelectionFailure>| {
                let (snapshot, page, text, parameters, tree) =
                    projector.project_with_tree(state).unwrap();
                let page = page.expect("PATCH context always projects its focused page");
                let snapshot_value: serde_json::Value =
                    serde_json::from_str(snapshot.json()).unwrap();
                let tree_value: serde_json::Value = serde_json::from_str(tree.json()).unwrap();
                let focused_control = state.interaction().patch_control_focus().unwrap();

                assert_eq!(page.engine().status(), expected_kind);
                assert_eq!(page.focused_control_id(), focused_control);
                assert_eq!(page.engine().editable(), expected_editable);
                assert_eq!(page.engine().failure(), expected_failure);
                assert_eq!(parameters.generation(), state.generation());
                assert_eq!(parameters.graph_revision(), expected_projection_revision);
                assert_eq!(tree.generation(), state.generation());
                assert_eq!(tree.graph_revision(), expected_projection_revision);
                assert_eq!(page.state_hash(), snapshot.hash());
                assert_eq!(text.state_hash(), snapshot.hash());
                assert_eq!(tree.state_hash(), snapshot.hash());
                assert!(text
                    .body()
                    .lines()
                    .nth(text.selected_line())
                    .unwrap()
                    .starts_with('>'));
                assert_eq!(
                    snapshot_value["engineSelection"],
                    tree_value["engineSelection"]
                );
                assert_eq!(
                    serde_json::to_value(page.engine()).unwrap(),
                    tree_value["patchPage"]["engine"]
                );
                assert_eq!(
                    tree_value["parameters"]["graphRevision"],
                    expected_projection_revision.value()
                );
                assert!(text
                    .body()
                    .contains(&format!("\"status\":\"{}\"", expected_kind.name())));
                assert_eq!(
                    text.body()
                        .lines()
                        .find(|line| line.contains("ENGINE"))
                        .unwrap()
                        .starts_with('>'),
                    page.focused_control_id() == crate::control::PatchControlId::Engine
                );
                if let Some(failure) = expected_failure {
                    assert!(text
                        .body()
                        .contains(&format!("\"failure\":\"{}\"", failure.name())));
                }
            };

        assert_projection(
            &state,
            EngineSelectionStatusKind::Ready,
            source_revision,
            true,
            None,
        );

        let ready_generation = state.generation();
        state.apply(AppEvent::Adjust(Direction::Right)).unwrap();
        assert_eq!(state.generation(), ready_generation + 1);
        assert_eq!(state.patches()[0].instrument_config(), &source_config);
        assert_projection(
            &state,
            EngineSelectionStatusKind::Loading,
            source_revision,
            false,
            None,
        );

        state.apply(AppEvent::Navigate(Direction::Down)).unwrap();
        assert_projection(
            &state,
            EngineSelectionStatusKind::Loading,
            source_revision,
            false,
            None,
        );

        let failed_correlation = state.engine_selection().correlation().unwrap().clone();
        let target_revision = source_revision.checked_next().unwrap();
        state
            .apply(AppEvent::EnginePreparationFailed {
                request_id: failed_correlation.request_id(),
                patch_id: failed_correlation.patch_id().unwrap(),
                intent: failed_correlation.intent().clone(),
                source_capability_id: failed_correlation.source_capability_id().unwrap().clone(),
                target_capability_id: failed_correlation.target_capability_id().unwrap().clone(),
                source_graph_revision: failed_correlation.source_graph_revision(),
                target_graph_revision: target_revision,
                failure: EngineSelectionFailure::WorkerUnavailable,
            })
            .unwrap();
        assert_eq!(state.patches()[0].instrument_config(), &source_config);
        assert_projection(
            &state,
            EngineSelectionStatusKind::Unavailable,
            source_revision,
            true,
            Some(EngineSelectionFailure::WorkerUnavailable),
        );

        state.apply(AppEvent::Navigate(Direction::Up)).unwrap();
        state.apply(AppEvent::Adjust(Direction::Right)).unwrap();
        state.apply(AppEvent::Navigate(Direction::Down)).unwrap();
        state.apply(AppEvent::Navigate(Direction::Down)).unwrap();
        let request_id = state.engine_selection().correlation().unwrap().request_id();
        state
            .apply(AppEvent::EngineSelectionLifecycleAdvanced {
                request_id,
                lifecycle: EngineSelectionStatusKind::Validating,
            })
            .unwrap();
        state
            .apply(AppEvent::EngineSelectionLifecycleAdvanced {
                request_id,
                lifecycle: EngineSelectionStatusKind::Preparing,
            })
            .unwrap();
        assert_projection(
            &state,
            EngineSelectionStatusKind::Preparing,
            source_revision,
            false,
            None,
        );
        let correlation = state.engine_selection().correlation().unwrap().clone();
        let candidate_config = BraidsCapability::new().unwrap().default_config().unwrap();
        state
            .apply(AppEvent::EnginePrepared {
                request_id: correlation.request_id(),
                patch_id: correlation.patch_id().unwrap(),
                intent: correlation.intent().clone(),
                source_capability_id: correlation.source_capability_id().unwrap().clone(),
                target_capability_id: correlation.target_capability_id().unwrap().clone(),
                source_graph_revision: correlation.source_graph_revision(),
                target_graph_revision: target_revision,
                candidate_config,
                prepared_visualization: None,
            })
            .unwrap();
        assert_eq!(state.patches()[0].instrument_config(), &source_config);
        assert_projection(
            &state,
            EngineSelectionStatusKind::Activating,
            target_revision,
            false,
            None,
        );

        state.apply(AppEvent::Navigate(Direction::Down)).unwrap();
        assert_projection(
            &state,
            EngineSelectionStatusKind::Activating,
            target_revision,
            false,
            None,
        );

        state
            .apply(AppEvent::EngineActivationAcknowledged {
                request_id: correlation.request_id(),
                intent: correlation.intent().clone(),
                target_graph_revision: target_revision,
                retired_graph_revision: source_revision,
                collected: true,
            })
            .unwrap();
        assert_eq!(
            state.patches()[0]
                .instrument_config()
                .capability_id()
                .as_str(),
            BRAIDS_CAPABILITY_ID
        );
        assert_projection(
            &state,
            EngineSelectionStatusKind::Ready,
            target_revision,
            true,
            None,
        );
    }

    #[test]
    fn projector_rejects_a_tree_projection_for_another_graph_revision() {
        let first_revision = GraphRevision::new(31).unwrap();
        let second_revision = GraphRevision::new(32).unwrap();
        let first_state = installed_state_for_graph(first_revision);
        let second_state = installed_state_for_graph(second_revision);
        let first = StateProjector::for_graph(first_revision);
        let second = StateProjector::for_graph(second_revision);
        let (_, _, parameters) = first.project(&first_state).unwrap();
        let (snapshot, text, _) = second.project(&second_state).unwrap();

        assert_eq!(
            second.state_tree(&snapshot, &text, &parameters),
            Err(StateProjectionError::StateTree(
                StateTreeError::GraphRevisionMismatch
            ))
        );
    }

    #[test]
    fn malformed_snapshot_cannot_be_rendered() {
        let snapshot = StateSnapshot::new("not-json");

        assert_eq!(
            StateProjector::new().text_projection(&snapshot, Selection::global()),
            Err(StateProjectionError::StateDeserialization)
        );
    }
}
