use crate::control::{
    FocusPath, FocusPathError, InteractionMode, MidiInputDeviceId, MixerControlId,
    PatchChoiceSubject, PatchControlId, PatchDetailSubject, PatchPositionId, ReturnPath,
    SemanticControlId, SurfaceId, TopLevelContext,
};
use crate::kernel::PatchId;
use crate::mixer::mixer_track_id::MixerTrackId;
use crate::mixer::mixer_track_parameters::MixerTrackParameter;
use crate::mixer::patch_output::PatchOutputParameter;
use crate::synth::ParameterId;

/// Compatibility classification derived from the canonical MixerControlId.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectionSection {
    Patch,
    Global,
}

/// Transitional diagnostic coordinates derived from a stable MixerControlId.
///
/// This value is never stored by InteractionState and is not an interaction
/// authority. It remains available only while older text/test projections are
/// migrated to semantic paths.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Selection {
    pub(super) section: SelectionSection,
    pub(super) patch_index: usize,
    pub(super) parameter_index: usize,
}

impl Selection {
    pub const fn patch(patch_index: usize) -> Self {
        Self {
            section: SelectionSection::Patch,
            patch_index,
            parameter_index: 0,
        }
    }

    pub const fn global() -> Self {
        Self {
            section: SelectionSection::Global,
            patch_index: 0,
            parameter_index: 0,
        }
    }

    pub const fn section(&self) -> SelectionSection {
        self.section
    }

    pub const fn patch_index(&self) -> usize {
        self.patch_index
    }

    pub const fn parameter_index(&self) -> usize {
        self.parameter_index
    }
}

/// The one reducer-owned PATCH subordinate session.
///
/// Choice and browser surfaces replace, rather than stack on, Detail or
/// Utility. Their suspended return identity is enough to reconstruct the exact
/// replaced surface on one Return without storing option, folder, or row
/// indices. The whole union is transient interaction state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PatchSubordinateSession {
    Detail {
        subject: PatchDetailSubject,
    },
    /// Utility reached horizontally from an open Detail session. Detail is
    /// suspended, not closed: its exact focus and original Overview return
    /// path are retained while Utility owns the one active focus.
    UtilityFromDetail {
        subject: PatchDetailSubject,
        detail_focus: FocusPath,
        detail_return: ReturnPath,
    },
    Choice {
        subject: PatchChoiceSubject,
        suspended_detail: Option<PatchDetailSubject>,
        suspended_return: Option<ReturnPath>,
    },
    FileBrowser {
        patch_position: Option<PatchPositionId>,
        asset_parameter_id: ParameterId,
        suspended_detail: Option<PatchDetailSubject>,
        suspended_return: Option<ReturnPath>,
    },
}

impl PatchSubordinateSession {
    pub const fn detail_subject(&self) -> Option<&PatchDetailSubject> {
        match self {
            Self::Detail { subject } | Self::UtilityFromDetail { subject, .. } => Some(subject),
            Self::Choice {
                suspended_detail, ..
            }
            | Self::FileBrowser {
                suspended_detail, ..
            } => suspended_detail.as_ref(),
        }
    }

    fn rekey_trailing_empty(&mut self, patch_id: PatchId) {
        match self {
            Self::Detail { .. } => {}
            Self::UtilityFromDetail {
                detail_focus,
                detail_return,
                ..
            } => {
                detail_focus.rekey_trailing_empty(patch_id);
                detail_return.rekey_trailing_empty(patch_id);
            }
            Self::Choice {
                subject,
                suspended_return,
                ..
            } => {
                subject.rekey_trailing_empty(patch_id);
                if let Some(return_path) = suspended_return {
                    return_path.rekey_trailing_empty(patch_id);
                }
            }
            Self::FileBrowser {
                patch_position,
                suspended_return,
                ..
            } => {
                if *patch_position == Some(PatchPositionId::TrailingEmpty) {
                    *patch_position = Some(PatchPositionId::Created(patch_id));
                }
                if let Some(return_path) = suspended_return {
                    return_path.rekey_trailing_empty(patch_id);
                }
            }
        }
    }
}

/// Complete interaction snapshot suspended while Settings owns
/// the one active semantic focus.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SettingsSession {
    suspended_focus: FocusPath,
    suspended_mode: InteractionMode,
    suspended_return_path: Option<ReturnPath>,
    suspended_subordinate_session: Option<PatchSubordinateSession>,
}

impl SettingsSession {
    pub const fn suspended_focus(&self) -> &FocusPath {
        &self.suspended_focus
    }

    pub const fn suspended_mode(&self) -> InteractionMode {
        self.suspended_mode
    }

    pub const fn suspended_return_path(&self) -> Option<&ReturnPath> {
        self.suspended_return_path.as_ref()
    }

    pub const fn suspended_subordinate_session(&self) -> Option<&PatchSubordinateSession> {
        self.suspended_subordinate_session.as_ref()
    }
}

/// Reducer-owned singular semantic focus, remembered roots, mode, return, and
/// at most one PATCH subordinate session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InteractionState {
    pub(super) remembered_send: FocusPath,
    pub(super) send_choice_origin: Option<FocusPath>,
    pub(super) send_name_editing: bool,
    pub(super) active_focus: FocusPath,
    pub(super) remembered_patch_main: Option<FocusPath>,
    pub(super) remembered_mixer_main: FocusPath,
    pub(super) mode: InteractionMode,
    pub(super) return_path: Option<ReturnPath>,
    /// Deliberately private: surface, subject, and suspended origin may only
    /// move together through this type's transitions.
    subordinate_session: Option<PatchSubordinateSession>,
    /// Present only while a temporary Settings system surface owns
    /// `active_focus`; it is the exact performance interaction to restore.
    settings_session: Option<SettingsSession>,
}

impl InteractionState {
    /// Creates startup interaction state on the always-available global mixer.
    pub fn new() -> Self {
        let mixer = FocusPath::mixer_track(MixerTrackId::default(), MixerTrackParameter::Level);
        Self {
            remembered_send: FocusPath::send(
                crate::control::SendControlId::Name {
                    bus: crate::mixer::bus_id::BusId::default(),
                },
                None,
            ),
            send_choice_origin: None,
            send_name_editing: false,
            active_focus: mixer.clone(),
            remembered_patch_main: None,
            remembered_mixer_main: mixer,
            mode: InteractionMode::Navigate,
            return_path: None,
            subordinate_session: None,
            settings_session: None,
        }
    }

    pub fn send_choice_origin(&self) -> Option<&FocusPath> {
        self.send_choice_origin.as_ref()
    }
    pub const fn send_name_editing(&self) -> bool {
        self.send_name_editing
    }
    pub fn selected_send(&self) -> crate::mixer::bus_id::BusId {
        match self.active_focus.control_id() {
            SemanticControlId::Send(control) => control.bus(),
            _ => match self.remembered_send.control_id() {
                SemanticControlId::Send(control) => control.bus(),
                _ => crate::mixer::bus_id::BusId::default(),
            },
        }
    }

    /// Reports whether surface, mode, return, and subordinate subject agree.
    pub fn subordinate_invariant_holds(&self) -> bool {
        let entered = self.return_path.as_ref().map(ReturnPath::entered_surface);
        match (&self.subordinate_session, self.active_focus.surface()) {
            (None, SurfaceId::PatchDetail | SurfaceId::PatchChoice | SurfaceId::FileBrowser) => {
                false
            }
            (None, _) => true,
            (Some(PatchSubordinateSession::Detail { .. }), SurfaceId::PatchDetail) => {
                entered == Some(SurfaceId::PatchDetail) && self.mode != InteractionMode::Modal
            }
            (
                Some(PatchSubordinateSession::UtilityFromDetail {
                    detail_focus,
                    detail_return,
                    ..
                }),
                SurfaceId::PatchUtility,
            ) => {
                entered == Some(SurfaceId::PatchUtility)
                    && self.mode != InteractionMode::Modal
                    && detail_focus.surface() == SurfaceId::PatchDetail
                    && detail_return.entered_surface() == SurfaceId::PatchDetail
                    && detail_focus.patch_position() == self.active_focus.patch_position()
                    && detail_return.origin().patch_position() == self.active_focus.patch_position()
            }
            (
                Some(PatchSubordinateSession::Choice {
                    suspended_detail,
                    suspended_return,
                    ..
                }),
                SurfaceId::PatchChoice,
            ) => {
                entered == Some(SurfaceId::PatchChoice)
                    && self.mode == InteractionMode::Modal
                    && suspended_detail.is_some()
                        == suspended_return
                            .as_ref()
                            .is_some_and(|path| path.entered_surface() == SurfaceId::PatchDetail)
            }
            (
                Some(PatchSubordinateSession::FileBrowser {
                    suspended_detail,
                    suspended_return,
                    ..
                }),
                SurfaceId::FileBrowser,
            ) => {
                entered == Some(SurfaceId::FileBrowser)
                    && self.mode == InteractionMode::Modal
                    && suspended_detail.is_some()
                        == suspended_return
                            .as_ref()
                            .is_some_and(|path| path.entered_surface() == SurfaceId::PatchDetail)
            }
            (Some(_), _) => false,
        }
    }

    pub fn settings_invariant_holds(&self) -> bool {
        match (&self.settings_session, self.active_focus.surface()) {
            (
                None,
                SurfaceId::MidiDeviceSettings
                | SurfaceId::ControllerSettings
                | SurfaceId::SaveLoadSettings,
            ) => false,
            (None, _) => true,
            (
                Some(session),
                SurfaceId::MidiDeviceSettings
                | SurfaceId::ControllerSettings
                | SurfaceId::SaveLoadSettings,
            ) => {
                self.mode == InteractionMode::Navigate
                    && self.return_path.is_none()
                    && self.subordinate_session.is_none()
                    && !session.suspended_focus.surface().is_system()
                    && session.suspended_focus.context() == self.active_focus.context()
            }
            (Some(_), _) => false,
        }
    }

    /// Backward-compatible witness name used by the earlier acceptance suite.
    pub fn detail_invariant_holds(&self) -> bool {
        self.subordinate_invariant_holds()
    }

    fn assert_subordinate_invariant(&self) {
        debug_assert!(
            self.subordinate_invariant_holds() && self.settings_invariant_holds(),
            "subordinate surface, subject, mode, and return must agree: \
             surface={:?} session={:?} return={:?} mode={:?}",
            self.active_focus.surface(),
            self.subordinate_session,
            self.return_path.as_ref().map(ReturnPath::entered_surface),
            self.mode,
        );
    }

    pub const fn context(&self) -> TopLevelContext {
        self.active_focus.context()
    }

    pub const fn active_surface(&self) -> SurfaceId {
        self.active_focus.surface()
    }

    pub const fn focus_path(&self) -> &FocusPath {
        &self.active_focus
    }

    pub const fn remembered_patch_main(&self) -> Option<&FocusPath> {
        self.remembered_patch_main.as_ref()
    }

    pub const fn remembered_mixer_main(&self) -> &FocusPath {
        &self.remembered_mixer_main
    }

    pub const fn mode(&self) -> InteractionMode {
        self.mode
    }

    pub const fn return_path(&self) -> Option<&ReturnPath> {
        self.return_path.as_ref()
    }

    pub const fn subordinate_session(&self) -> Option<&PatchSubordinateSession> {
        self.subordinate_session.as_ref()
    }

    pub const fn settings_session(&self) -> Option<&SettingsSession> {
        self.settings_session.as_ref()
    }

    /// Returns the capability whose schema the open detail surface shows, or
    /// `None` when no detail entry is open.
    pub const fn detail_subject(&self) -> Option<&PatchDetailSubject> {
        match self.subordinate_session.as_ref() {
            Some(session) => session.detail_subject(),
            None => None,
        }
    }

    pub const fn patch_focus(&self) -> Option<PatchId> {
        match &self.remembered_patch_main {
            Some(path) => path.patch_id(),
            None => None,
        }
    }

    pub const fn patch_position_focus(&self) -> Option<PatchPositionId> {
        match &self.remembered_patch_main {
            Some(path) => path.patch_position(),
            None => None,
        }
    }

    pub fn patch_control_focus(&self) -> Option<PatchControlId> {
        if let Some(PatchSubordinateSession::Choice { subject, .. }) =
            self.subordinate_session.as_ref()
        {
            return Some(subject.control_id().clone());
        }
        if self.active_focus.surface() == SurfaceId::FileBrowser {
            return self
                .return_path
                .as_ref()
                .and_then(|path| match path.origin().control_id() {
                    SemanticControlId::Patch(control) => Some(control.clone()),
                    SemanticControlId::Mixer(_)
                    | SemanticControlId::Send(_)
                    | SemanticControlId::Modal(_)
                    | SemanticControlId::MidiInputDevice(_)
                    | SemanticControlId::ControllerSetting(_)
                    | SemanticControlId::SessionFileAction(_)
                    | SemanticControlId::MidiInputListRoot
                    | SemanticControlId::SurfaceRoot => None,
                });
        }
        let path = if !self.active_focus.surface().is_system()
            && self.active_focus.context() == TopLevelContext::Patch
        {
            &self.active_focus
        } else {
            self.remembered_patch_main.as_ref()?
        };
        match path.control_id() {
            SemanticControlId::Patch(control) => Some(control.clone()),
            SemanticControlId::Mixer(_)
            | SemanticControlId::Send(_)
            | SemanticControlId::Modal(_)
            | SemanticControlId::MidiInputDevice(_)
            | SemanticControlId::ControllerSetting(_)
            | SemanticControlId::SessionFileAction(_)
            | SemanticControlId::MidiInputListRoot
            | SemanticControlId::SurfaceRoot => None,
        }
    }

    pub fn mixer_control_focus(&self) -> &MixerControlId {
        let path = if !self.active_focus.surface().is_system()
            && self.active_focus.context() == TopLevelContext::Mixer
            && self.active_focus.surface() != SurfaceId::Sends
        {
            &self.active_focus
        } else {
            &self.remembered_mixer_main
        };
        match path.control_id() {
            SemanticControlId::Mixer(control) => control,
            SemanticControlId::Patch(_)
            | SemanticControlId::Send(_)
            | SemanticControlId::Modal(_)
            | SemanticControlId::MidiInputDevice(_)
            | SemanticControlId::ControllerSetting(_)
            | SemanticControlId::SessionFileAction(_)
            | SemanticControlId::MidiInputListRoot
            | SemanticControlId::SurfaceRoot => {
                unreachable!("remembered MixerMain path is always a Mixer control")
            }
        }
    }

    pub(super) fn initialize_patch_focus(&mut self, focus: Option<FocusPath>) {
        self.remembered_patch_main = focus;
        self.assert_subordinate_invariant();
    }

    /// Atomically replaces every interaction-owned reference to the trailing
    /// empty Patch position with the newly committed Patch identity.
    ///
    /// Paths that no longer refer to the empty position are left byte-for-byte
    /// unchanged, so a user who navigated away during preparation does not
    /// have focus stolen by the eventual commit.
    pub(super) fn rekey_trailing_empty(&mut self, patch_id: PatchId) {
        self.active_focus.rekey_trailing_empty(patch_id);
        if let Some(path) = self.remembered_patch_main.as_mut() {
            path.rekey_trailing_empty(patch_id);
        }
        if let Some(path) = self.return_path.as_mut() {
            path.rekey_trailing_empty(patch_id);
        }
        if let Some(session) = self.subordinate_session.as_mut() {
            session.rekey_trailing_empty(patch_id);
        }
        if let Some(session) = self.settings_session.as_mut() {
            session.suspended_focus.rekey_trailing_empty(patch_id);
            if let Some(path) = session.suspended_return_path.as_mut() {
                path.rekey_trailing_empty(patch_id);
            }
            if let Some(subordinate) = session.suspended_subordinate_session.as_mut() {
                subordinate.rekey_trailing_empty(patch_id);
            }
        }
        self.assert_subordinate_invariant();
    }

    /// Lands the active focus on a main path, leaving any subordinate surface.
    ///
    /// A main path is by definition not a detail path, so the subject is
    /// cleared with the return path rather than left to outlive its surface.
    pub(super) fn set_active_main(&mut self, focus: FocusPath) -> Result<(), FocusPathError> {
        if self.settings_session.is_some() {
            return Err(FocusPathError::ControlSurfaceMismatch);
        }
        focus.validate()?;
        if !focus.surface().is_main() {
            return Err(FocusPathError::ControlSurfaceMismatch);
        }
        match focus.context() {
            TopLevelContext::Patch => self.remembered_patch_main = Some(focus.clone()),
            TopLevelContext::Mixer if focus.surface() == SurfaceId::Sends => {
                self.remembered_send = focus.clone()
            }
            TopLevelContext::Mixer => self.remembered_mixer_main = focus.clone(),
        }
        self.active_focus = focus;
        self.leave_subordinate();
        self.assert_subordinate_invariant();
        Ok(())
    }

    pub(super) fn select_context(
        &mut self,
        context: TopLevelContext,
    ) -> Result<(), FocusPathError> {
        if self.settings_session.is_some() {
            return Err(FocusPathError::ControlSurfaceMismatch);
        }
        self.send_choice_origin = None;
        self.send_name_editing = false;
        self.active_focus = match context {
            TopLevelContext::Patch => self
                .remembered_patch_main
                .clone()
                .ok_or(FocusPathError::PatchIdentityMismatch)?,
            TopLevelContext::Mixer => self.remembered_mixer_main.clone(),
        };
        self.mode = InteractionMode::Navigate;
        // A detail subject belongs to the context it was opened in; switching
        // context leaves the detail surface rather than carrying it.
        self.leave_subordinate();
        self.assert_subordinate_invariant();
        Ok(())
    }

    pub(super) fn set_mode(&mut self, mode: InteractionMode) -> Result<(), FocusPathError> {
        if self.settings_session.is_some() {
            return Err(FocusPathError::ControlSurfaceMismatch);
        }
        let modal_surface = matches!(
            self.active_focus.surface(),
            SurfaceId::PatchChoice | SurfaceId::FileBrowser
        ) || (self.active_focus.surface() == SurfaceId::Sends
            && self.send_choice_origin.is_some());
        let allowed = if modal_surface {
            mode == InteractionMode::Modal
        } else {
            mode.is_phase_two_reachable()
        };
        if !allowed {
            return Err(FocusPathError::ModalIdentityUnavailable);
        }
        self.mode = mode;
        self.assert_subordinate_invariant();
        Ok(())
    }

    /// Enters one *persistent* side surface from a main path.
    ///
    /// The subordinate detail surface is not reachable here: it needs a
    /// subject, which only [`Self::enter_detail`] can supply. Requiring a main
    /// origin is also what makes the surfaces non-nesting — a path already on
    /// `PatchUtility` or `PatchDetail` is not main, so neither can stack a
    /// second origin on the other.
    pub(super) fn enter_surface(&mut self, surface: SurfaceId) -> Result<(), FocusPathError> {
        if self.settings_session.is_some() {
            return Err(FocusPathError::ControlSurfaceMismatch);
        }
        if !surface.is_persistent_side()
            || !self.active_focus.surface().is_main()
            || surface.context() != Some(self.active_focus.context())
        {
            return Err(FocusPathError::ContextSurfaceMismatch);
        }
        let origin = self.active_focus.clone();
        self.return_path = Some(ReturnPath::new(origin, surface)?);
        self.active_focus = match surface {
            SurfaceId::PatchUtility => FocusPath::patch_utility_at(
                self.active_focus
                    .patch_position()
                    .ok_or(FocusPathError::PatchIdentityMismatch)?,
                PatchControlId::Output(PatchOutputParameter::TrimGain),
            ),
            SurfaceId::MixerInspector => {
                let SemanticControlId::Mixer(MixerControlId::Track { track_id, .. }) =
                    self.active_focus.control_id()
                else {
                    return Err(FocusPathError::ControlSurfaceMismatch);
                };
                // The selected track's first send: Send(trackId, BusId 0).
                FocusPath::mixer_send(*track_id, crate::mixer::bus_id::BusId::default())
            }
            SurfaceId::PatchDetail
            | SurfaceId::PatchChoice
            | SurfaceId::FileBrowser
            | SurfaceId::PatchMain
            | SurfaceId::Sends
            | SurfaceId::MixerMain
            | SurfaceId::MidiDeviceSettings
            | SurfaceId::ControllerSettings
            | SurfaceId::SaveLoadSettings => return Err(FocusPathError::ControlSurfaceMismatch),
        };
        self.mode = InteractionMode::Navigate;
        self.assert_subordinate_invariant();
        Ok(())
    }

    /// Opens the subordinate detail surface on one subject.
    ///
    /// This is the only transition that can create a Detail session, and it
    /// sets all three detail facts together: the remembered origin, the
    /// subject, and the focus on the subject's first control. The origin must
    /// be the current main path, so entering from `PatchUtility` or from an
    /// already-open detail surface is refused rather than stacked.
    pub(super) fn enter_detail(
        &mut self,
        subject: PatchDetailSubject,
        focus: FocusPath,
    ) -> Result<(), FocusPathError> {
        if self.settings_session.is_some() {
            return Err(FocusPathError::ControlSurfaceMismatch);
        }
        if !self.active_focus.surface().is_main()
            || self.active_focus.context() != TopLevelContext::Patch
        {
            return Err(FocusPathError::ContextSurfaceMismatch);
        }
        focus.validate()?;
        if focus.surface() != SurfaceId::PatchDetail {
            return Err(FocusPathError::ControlSurfaceMismatch);
        }
        let return_path = ReturnPath::new(self.active_focus.clone(), SurfaceId::PatchDetail)?;
        self.return_path = Some(return_path);
        self.subordinate_session = Some(PatchSubordinateSession::Detail { subject });
        self.active_focus = focus;
        self.mode = InteractionMode::Navigate;
        self.assert_subordinate_invariant();
        Ok(())
    }

    /// Moves the one active focus from Detail to Utility while retaining the
    /// complete Detail session for the inverse horizontal transition.
    pub(super) fn switch_detail_to_utility(
        &mut self,
        utility_focus: FocusPath,
    ) -> Result<(), FocusPathError> {
        utility_focus.validate()?;
        if self.active_focus.surface() != SurfaceId::PatchDetail
            || utility_focus.surface() != SurfaceId::PatchUtility
            || utility_focus.patch_position() != self.active_focus.patch_position()
        {
            return Err(FocusPathError::ContextSurfaceMismatch);
        }
        let Some(PatchSubordinateSession::Detail { subject }) = self.subordinate_session.take()
        else {
            return Err(FocusPathError::ModalIdentityUnavailable);
        };
        let detail_focus = self.active_focus.clone();
        let detail_return = self
            .return_path
            .take()
            .ok_or(FocusPathError::ContextSurfaceMismatch)?;
        self.return_path = Some(ReturnPath::new(
            detail_return.origin().clone(),
            SurfaceId::PatchUtility,
        )?);
        self.subordinate_session = Some(PatchSubordinateSession::UtilityFromDetail {
            subject,
            detail_focus,
            detail_return,
        });
        self.active_focus = utility_focus;
        self.mode = InteractionMode::Navigate;
        self.assert_subordinate_invariant();
        Ok(())
    }

    /// Restores the exact Detail subject, focus, and Overview return path that
    /// were suspended by [`Self::switch_detail_to_utility`].
    pub(super) fn restore_detail_from_utility(&mut self) -> Result<(), FocusPathError> {
        if self.active_focus.surface() != SurfaceId::PatchUtility {
            return Err(FocusPathError::ContextSurfaceMismatch);
        }
        let Some(PatchSubordinateSession::UtilityFromDetail {
            subject,
            detail_focus,
            detail_return,
        }) = self.subordinate_session.take()
        else {
            return Err(FocusPathError::ModalIdentityUnavailable);
        };
        self.active_focus = detail_focus;
        self.return_path = Some(detail_return);
        self.subordinate_session = Some(PatchSubordinateSession::Detail { subject });
        self.mode = InteractionMode::Navigate;
        self.assert_subordinate_invariant();
        Ok(())
    }

    pub const fn utility_suspends_detail(&self) -> bool {
        matches!(
            self.subordinate_session,
            Some(PatchSubordinateSession::UtilityFromDetail { .. })
        )
    }

    /// Replaces the current PATCH main, Utility, or Detail surface with one
    /// generic choice session. A modal/browser origin is refused, so sessions
    /// can never stack.
    pub(super) fn enter_choice(
        &mut self,
        subject: PatchChoiceSubject,
        focus: FocusPath,
    ) -> Result<(), FocusPathError> {
        if self.settings_session.is_some() {
            return Err(FocusPathError::ControlSurfaceMismatch);
        }
        self.enter_modal_surface(
            Some(subject.patch_position()),
            focus,
            |detail, suspended_return| PatchSubordinateSession::Choice {
                subject,
                suspended_detail: detail,
                suspended_return,
            },
        )
    }

    /// Replaces the current PATCH main, Utility, or Detail surface with the
    /// controller-native Sample Browser.
    #[cfg(test)]
    pub(super) fn enter_file_browser(
        &mut self,
        patch_id: PatchId,
        asset_parameter_id: ParameterId,
        focus: FocusPath,
    ) -> Result<(), FocusPathError> {
        if self.settings_session.is_some() {
            return Err(FocusPathError::ControlSurfaceMismatch);
        }
        self.enter_file_browser_at(patch_id.into(), asset_parameter_id, focus)
    }

    #[cfg(test)]
    pub(super) fn enter_file_browser_at(
        &mut self,
        patch_position: PatchPositionId,
        asset_parameter_id: ParameterId,
        focus: FocusPath,
    ) -> Result<(), FocusPathError> {
        if self.settings_session.is_some() {
            return Err(FocusPathError::ControlSurfaceMismatch);
        }
        self.enter_asset_browser(Some(patch_position), asset_parameter_id, focus)
    }

    pub(super) fn enter_asset_browser(
        &mut self,
        patch_position: Option<PatchPositionId>,
        asset_parameter_id: ParameterId,
        focus: FocusPath,
    ) -> Result<(), FocusPathError> {
        self.enter_modal_surface(patch_position, focus, |detail, suspended_return| {
            PatchSubordinateSession::FileBrowser {
                patch_position,
                asset_parameter_id,
                suspended_detail: detail,
                suspended_return,
            }
        })
    }

    fn enter_modal_surface(
        &mut self,
        patch_position: Option<PatchPositionId>,
        focus: FocusPath,
        make_session: impl FnOnce(
            Option<PatchDetailSubject>,
            Option<ReturnPath>,
        ) -> PatchSubordinateSession,
    ) -> Result<(), FocusPathError> {
        let allowed_origin = matches!(
            self.active_focus.surface(),
            SurfaceId::PatchMain | SurfaceId::PatchUtility | SurfaceId::PatchDetail
        ) || (focus.surface() == SurfaceId::FileBrowser
            && matches!(
                self.active_focus.surface(),
                SurfaceId::MixerInspector | SurfaceId::Sends
            ));
        if !allowed_origin
            || self.active_focus.context() != focus.context()
            || self.active_focus.patch_position() != patch_position
        {
            return Err(FocusPathError::ContextSurfaceMismatch);
        }
        focus.validate()?;
        if !matches!(
            focus.surface(),
            SurfaceId::PatchChoice | SurfaceId::FileBrowser
        ) || focus.patch_position() != patch_position
        {
            return Err(FocusPathError::ControlSurfaceMismatch);
        }
        let entered_surface = focus.surface();
        let origin = self.active_focus.clone();
        let suspended_detail = match self.subordinate_session.take() {
            Some(PatchSubordinateSession::Detail { subject }) => Some(subject),
            Some(other) => {
                self.subordinate_session = Some(other);
                return Err(FocusPathError::ModalIdentityUnavailable);
            }
            None => None,
        };
        let suspended_return = self.return_path.take();
        self.return_path = Some(ReturnPath::new(origin, entered_surface)?);
        self.subordinate_session = Some(make_session(suspended_detail, suspended_return));
        self.active_focus = focus;
        self.mode = InteractionMode::Modal;
        self.assert_subordinate_invariant();
        Ok(())
    }

    /// Leaves whichever subordinate or side surface is open, restoring the
    /// exact remembered origin.
    ///
    /// The return path and the detail subject are cleared together, so leaving
    /// a detail surface can never strand its subject.
    pub(super) fn return_to_origin(&mut self) -> Result<(), FocusPathError> {
        if self.settings_session.is_some() {
            return Err(FocusPathError::ControlSurfaceMismatch);
        }
        let path = self
            .return_path
            .take()
            .ok_or(FocusPathError::ContextSurfaceMismatch)?;
        if path.entered_surface() != self.active_focus.surface() {
            // Put it back: a refused Return leaves state identical.
            self.return_path = Some(path);
            return Err(FocusPathError::ContextSurfaceMismatch);
        }
        self.active_focus = path.origin().clone();
        match self.subordinate_session.take() {
            Some(PatchSubordinateSession::Choice {
                suspended_detail,
                suspended_return,
                ..
            })
            | Some(PatchSubordinateSession::FileBrowser {
                suspended_detail,
                suspended_return,
                ..
            }) => {
                self.return_path = suspended_return;
                self.subordinate_session =
                    suspended_detail.map(|subject| PatchSubordinateSession::Detail { subject });
            }
            Some(PatchSubordinateSession::Detail { .. })
            | Some(PatchSubordinateSession::UtilityFromDetail { .. })
            | None => {
                self.return_path = None;
                self.subordinate_session = None;
            }
        }
        self.mode = InteractionMode::Navigate;
        self.assert_subordinate_invariant();
        Ok(())
    }

    /// Suspends the complete performance interaction and gives Settings the
    /// one active focus. Admission policy remains reducer-owned.
    pub(super) fn open_settings(
        &mut self,
        settings_focus: FocusPath,
    ) -> Result<(), FocusPathError> {
        if self.settings_session.is_some() || self.active_focus.surface().is_system() {
            return Err(FocusPathError::ControlSurfaceMismatch);
        }
        settings_focus.validate()?;
        if !settings_focus.surface().is_system()
            || settings_focus.context() != self.active_focus.context()
        {
            return Err(FocusPathError::ContextSurfaceMismatch);
        }
        let session = SettingsSession {
            suspended_focus: self.active_focus.clone(),
            suspended_mode: self.mode,
            suspended_return_path: self.return_path.take(),
            suspended_subordinate_session: self.subordinate_session.take(),
        };
        self.active_focus = settings_focus;
        self.mode = InteractionMode::Navigate;
        self.settings_session = Some(session);
        self.assert_subordinate_invariant();
        Ok(())
    }

    /// Restores the exact performance interaction suspended on Settings entry.
    pub(super) fn return_from_settings(&mut self) -> Result<(), FocusPathError> {
        if !self.active_focus.surface().is_system() {
            return Err(FocusPathError::ContextSurfaceMismatch);
        }
        let session = self
            .settings_session
            .take()
            .ok_or(FocusPathError::ContextSurfaceMismatch)?;
        self.active_focus = session.suspended_focus;
        self.mode = session.suspended_mode;
        self.return_path = session.suspended_return_path;
        self.subordinate_session = session.suspended_subordinate_session;
        self.assert_subordinate_invariant();
        Ok(())
    }

    /// Reconciles Settings focus by exact identity, then the next surviving
    /// old identity, then the previous surviving identity, then the root.
    pub(super) fn reconcile_midi_settings_focus(
        &mut self,
        old_order: &[MidiInputDeviceId],
        new_order: &[MidiInputDeviceId],
    ) -> Result<Option<(FocusPath, FocusPath)>, FocusPathError> {
        if self.active_focus.surface() != SurfaceId::MidiDeviceSettings
            || self.settings_session.is_none()
        {
            return Err(FocusPathError::ContextSurfaceMismatch);
        }
        let context = self.active_focus.context();
        let old_focus = self.active_focus.clone();
        let replacement = match self.active_focus.control_id() {
            SemanticControlId::MidiInputDevice(identity) => {
                if new_order.contains(identity) {
                    return Ok(None);
                }
                let old_index = old_order
                    .iter()
                    .position(|candidate| candidate == identity)
                    .ok_or(FocusPathError::ControlSurfaceMismatch)?;
                old_order[old_index + 1..]
                    .iter()
                    .find(|candidate| new_order.contains(candidate))
                    .or_else(|| {
                        old_order[..old_index]
                            .iter()
                            .rev()
                            .find(|candidate| new_order.contains(candidate))
                    })
                    .cloned()
                    .map_or_else(
                        || FocusPath::midi_device_settings_root(context),
                        |identity| FocusPath::midi_device_settings(context, identity),
                    )
            }
            SemanticControlId::MidiInputListRoot => new_order.first().cloned().map_or_else(
                || FocusPath::midi_device_settings_root(context),
                |identity| FocusPath::midi_device_settings(context, identity),
            ),
            _ => return Err(FocusPathError::ControlSurfaceMismatch),
        };
        replacement.validate()?;
        if replacement == old_focus {
            return Ok(None);
        }
        self.active_focus = replacement.clone();
        self.assert_subordinate_invariant();
        Ok(Some((old_focus, replacement)))
    }

    /// Repairs the performance focus suspended under Settings after a schema
    /// change removes its exact origin.
    pub(super) fn replace_settings_suspended_focus(
        &mut self,
        focus: FocusPath,
    ) -> Result<(), FocusPathError> {
        focus.validate()?;
        let session = self
            .settings_session
            .as_mut()
            .ok_or(FocusPathError::ContextSurfaceMismatch)?;
        if focus.surface().is_system() || focus.context() != self.active_focus.context() {
            return Err(FocusPathError::ContextSurfaceMismatch);
        }
        session.suspended_focus = focus;
        self.assert_subordinate_invariant();
        Ok(())
    }

    /// Repairs the main return origin nested inside the suspended PATCH
    /// subordinate state without changing its subject or current row.
    pub(super) fn replace_settings_suspended_return_origin(
        &mut self,
        origin: FocusPath,
    ) -> Result<(), FocusPathError> {
        let session = self
            .settings_session
            .as_mut()
            .ok_or(FocusPathError::ContextSurfaceMismatch)?;
        let Some(return_path) = session.suspended_return_path.as_ref() else {
            return Ok(());
        };
        session.suspended_return_path = Some(ReturnPath::new(
            origin.clone(),
            return_path.entered_surface(),
        )?);
        if let Some(PatchSubordinateSession::UtilityFromDetail { detail_return, .. }) =
            session.suspended_subordinate_session.as_mut()
        {
            *detail_return = ReturnPath::new(origin, SurfaceId::PatchDetail)?;
        }
        self.assert_subordinate_invariant();
        Ok(())
    }

    /// Abandons any open subordinate surface without restoring its origin.
    ///
    /// Used where the origin is about to stop existing — a patch switch leaves
    /// the Patch the subject belonged to — so the caller supplies the new
    /// focus itself. Clears the return path and the subject together.
    pub(super) fn leave_subordinate(&mut self) {
        self.return_path = None;
        self.subordinate_session = None;
        self.assert_subordinate_invariant();
    }

    /// Leaves an open detail surface by restoring its remembered origin.
    ///
    /// Used where the subject stopped naming a live capability or slot — an
    /// engine swap or a slot clear committed underneath the open entry. The
    /// declaration is explicit that such a subject is *not* repaired into a
    /// neighbouring one: the surface is left back to its origin, because
    /// silently retargeting a detail view would show one capability's values
    /// under another's title. Reports whether a surface was actually left.
    pub(super) fn leave_detail_to_origin(&mut self) -> bool {
        if !matches!(
            self.subordinate_session,
            Some(PatchSubordinateSession::Detail { .. })
                | Some(PatchSubordinateSession::UtilityFromDetail { .. })
        ) {
            return false;
        }
        let origin = match self.subordinate_session.as_ref() {
            Some(PatchSubordinateSession::UtilityFromDetail { detail_return, .. }) => {
                detail_return.origin().clone()
            }
            _ => self
                .return_path
                .as_ref()
                .expect("the detail invariant pairs an open subject with a return path")
                .origin()
                .clone(),
        };
        self.return_path = None;
        self.subordinate_session = None;
        self.active_focus = origin;
        self.mode = InteractionMode::Navigate;
        self.assert_subordinate_invariant();
        true
    }

    pub(super) fn replace_remembered_patch_main(&mut self, focus: FocusPath) {
        let was_active = self.active_focus.surface() == SurfaceId::PatchMain;
        self.remembered_patch_main = Some(focus.clone());
        if was_active {
            self.active_focus = focus;
        }
        self.assert_subordinate_invariant();
    }

    pub(super) fn replace_remembered_mixer_main(&mut self, focus: FocusPath) {
        let was_active = self.active_focus.surface() == SurfaceId::MixerMain;
        self.remembered_mixer_main = focus.clone();
        if was_active {
            self.active_focus = focus;
        }
        self.assert_subordinate_invariant();
    }

    /// Replaces the remembered origin of whichever surface is open.
    ///
    /// A construction failure is returned, never swallowed: assigning `None`
    /// here would clear the return path while an open detail surface still
    /// held its subject, which is precisely the state the detail invariant
    /// forbids. No caller can reach the failure today — every origin it is
    /// given is a repaired main path in the entered surface's own context —
    /// and this is what keeps that true rather than assuming it.
    pub(super) fn replace_return_origin(
        &mut self,
        origin: FocusPath,
    ) -> Result<(), FocusPathError> {
        let Some(return_path) = self.return_path.as_ref() else {
            return Ok(());
        };
        self.return_path = Some(ReturnPath::new(
            origin.clone(),
            return_path.entered_surface(),
        )?);
        if let Some(PatchSubordinateSession::UtilityFromDetail { detail_return, .. }) =
            self.subordinate_session.as_mut()
        {
            *detail_return = ReturnPath::new(origin, SurfaceId::PatchDetail)?;
        }
        self.assert_subordinate_invariant();
        Ok(())
    }
}

impl Default for InteractionState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::InteractionState;
    use crate::control::{
        FocusCapabilityId, FocusPath, InteractionMode, PatchChoiceSubject, PatchControlId,
        PatchDetailSubject, PatchPositionId, PatchSubordinateSession, SurfaceId, TopLevelContext,
    };
    use crate::kernel::PatchId;
    use crate::mixer::mixer_track_id::MixerTrackId;
    use crate::mixer::mixer_track_parameters::MixerTrackParameter;

    #[test]
    fn interaction_state_starts_with_one_stable_mixer_focus() {
        let state = InteractionState::new();
        assert_eq!(state.context(), TopLevelContext::Mixer);
        assert_eq!(state.active_surface(), SurfaceId::MixerMain);
        assert_eq!(
            state.focus_path(),
            &FocusPath::mixer_track(MixerTrackId::default(), MixerTrackParameter::Level)
        );
        assert_eq!(state.patch_focus(), None);
        assert_eq!(state.mode(), InteractionMode::Navigate);
        assert_eq!(state.return_path(), None);
    }

    #[test]
    fn context_round_trip_preserves_independent_stable_paths() {
        let mut state = InteractionState::new();
        let patch = FocusPath::patch_main(
            PatchId::new(7).unwrap(),
            None,
            PatchControlId::Envelope(crate::synth::VoiceEnvelopeParameter::AttackMilliseconds),
        );
        state.initialize_patch_focus(Some(patch.clone()));
        state.select_context(TopLevelContext::Patch).unwrap();
        state.select_context(TopLevelContext::Mixer).unwrap();
        state.select_context(TopLevelContext::Patch).unwrap();
        assert_eq!(state.focus_path(), &patch);
        assert_eq!(state.patch_focus(), Some(PatchId::new(7).unwrap()));
    }

    #[test]
    fn side_surface_round_trip_restores_exact_origin_and_navigate_mode() {
        let mut state = InteractionState::new();
        let origin = state.focus_path().clone();
        state.set_mode(InteractionMode::Adjust).unwrap();
        state.enter_surface(SurfaceId::MixerInspector).unwrap();
        assert_eq!(state.active_surface(), SurfaceId::MixerInspector);
        assert_eq!(state.return_path().unwrap().origin(), &origin);
        assert_eq!(state.mode(), InteractionMode::Navigate);
        state.return_to_origin().unwrap();
        assert_eq!(state.focus_path(), &origin);
        assert_eq!(state.return_path(), None);
    }

    /// A return origin that cannot be constructed is refused, not swallowed.
    ///
    /// Assigning `None` here would clear the return path while an open detail
    /// surface still held its subject — the state the detail invariant
    /// forbids. No reducer path can reach this today, because every origin the
    /// repair hands over is a main path in the entered surface's own context;
    /// this is what keeps that a fact rather than an assumption.
    #[test]
    fn a_return_origin_that_cannot_be_constructed_is_refused_and_changes_nothing() {
        let mut state = InteractionState::new();
        state.enter_surface(SurfaceId::MixerInspector).unwrap();
        let before = state.clone();

        // An Inspector path is not a main path, so it cannot be an origin.
        let not_a_main_path = FocusPath::mixer_send(
            MixerTrackId::default(),
            crate::mixer::bus_id::BusId::default(),
        );
        assert!(state.replace_return_origin(not_a_main_path).is_err());
        assert_eq!(
            state, before,
            "a refused replacement leaves state identical"
        );
        assert!(state.return_path().is_some());
        assert!(state.detail_invariant_holds());
    }

    #[test]
    fn phase_two_rejects_reserved_modes_and_cross_context_surface_entry() {
        let mut state = InteractionState::new();
        assert!(state.set_mode(InteractionMode::Modal).is_err());
        assert!(state.enter_surface(SurfaceId::PatchUtility).is_err());
        assert_eq!(state.mode(), InteractionMode::Navigate);
    }

    #[test]
    fn one_choice_session_replaces_detail_and_returns_through_both_exact_origins() {
        let patch_id = PatchId::new(7).unwrap();
        let capability_id = crate::synth::CapabilityId::new("instrument.test").unwrap();
        let parameter_id = crate::synth::ParameterId::new("test.choice").unwrap();
        let origin = FocusPath::patch_main(
            patch_id,
            Some(FocusCapabilityId::Instrument(capability_id.clone())),
            PatchControlId::Capability(parameter_id.clone()),
        );
        let detail_focus = FocusPath::patch_detail(
            patch_id,
            FocusCapabilityId::Instrument(capability_id.clone()),
            PatchControlId::Capability(parameter_id.clone()),
        );
        let subject = PatchDetailSubject::instrument(capability_id);
        let choice_subject =
            PatchChoiceSubject::new(patch_id, PatchControlId::Capability(parameter_id.clone()));
        let choice_focus =
            FocusPath::patch_choice(patch_id, choice_subject.stable_id(), "choice.test.one");

        let mut state = InteractionState::new();
        state.initialize_patch_focus(Some(origin.clone()));
        state.select_context(TopLevelContext::Patch).unwrap();
        state
            .enter_detail(subject.clone(), detail_focus.clone())
            .unwrap();
        state
            .enter_choice(choice_subject.clone(), choice_focus.clone())
            .unwrap();

        assert_eq!(state.focus_path(), &choice_focus);
        assert_eq!(state.mode(), InteractionMode::Modal);
        assert!(matches!(
            state.subordinate_session(),
            Some(PatchSubordinateSession::Choice {
                subject: actual,
                suspended_detail: Some(actual_detail),
                suspended_return: Some(_),
            }) if actual == &choice_subject && actual_detail == &subject
        ));
        assert!(state.subordinate_invariant_holds());

        let before_nested_attempt = state.clone();
        assert!(state
            .enter_file_browser(
                patch_id,
                parameter_id,
                FocusPath::file_browser(patch_id, "browser.test", "cancel"),
            )
            .is_err());
        assert_eq!(
            state, before_nested_attempt,
            "modal-from-modal changed state"
        );

        state.return_to_origin().unwrap();
        assert_eq!(state.focus_path(), &detail_focus);
        assert_eq!(state.detail_subject(), Some(&subject));
        assert_eq!(state.return_path().map(|path| path.origin()), Some(&origin));
        state.return_to_origin().unwrap();
        assert_eq!(state.focus_path(), &origin);
        assert!(state.subordinate_session().is_none());
        assert!(state.return_path().is_none());
        assert!(state.subordinate_invariant_holds());
    }

    #[test]
    fn file_browser_replaces_utility_without_fabricating_a_detail_subject() {
        let patch_id = PatchId::new(3).unwrap();
        let origin = FocusPath::patch_main(patch_id, None, PatchControlId::Engine);
        let asset_id = crate::synth::ParameterId::new("sample.asset").unwrap();
        let browser_focus =
            FocusPath::file_browser(patch_id, "browser.sample.asset", "folder.drums");
        let mut state = InteractionState::new();
        state.initialize_patch_focus(Some(origin.clone()));
        state.select_context(TopLevelContext::Patch).unwrap();
        state.enter_surface(SurfaceId::PatchUtility).unwrap();
        let utility_focus = state.focus_path().clone();
        state
            .enter_file_browser(patch_id, asset_id.clone(), browser_focus.clone())
            .unwrap();

        assert_eq!(state.focus_path(), &browser_focus);
        assert!(matches!(
            state.subordinate_session(),
            Some(PatchSubordinateSession::FileBrowser {
                asset_parameter_id,
                suspended_detail: None,
                suspended_return: Some(_),
                ..
            }) if asset_parameter_id == &asset_id
        ));
        assert_eq!(state.detail_subject(), None);
        state.return_to_origin().unwrap();
        assert_eq!(state.focus_path(), &utility_focus);
        assert_eq!(state.active_surface(), SurfaceId::PatchUtility);
        state.return_to_origin().unwrap();
        assert_eq!(state.focus_path(), &origin);
        assert!(state.subordinate_invariant_holds());
    }

    #[test]
    fn midi_settings_round_trip_restores_exact_mixer_focus_and_mode_without_stacking() {
        let mut state = InteractionState::new();
        let origin =
            FocusPath::mixer_track(MixerTrackId::new(9).unwrap(), MixerTrackParameter::Pan);
        state.set_active_main(origin.clone()).unwrap();
        state.set_mode(InteractionMode::Adjust).unwrap();
        let settings = FocusPath::midi_device_settings_root(TopLevelContext::Mixer);

        state.open_settings(settings.clone()).unwrap();
        assert_eq!(state.focus_path(), &settings);
        assert_eq!(state.mode(), InteractionMode::Navigate);
        assert_eq!(state.settings_session().unwrap().suspended_focus(), &origin);
        assert_eq!(
            state.settings_session().unwrap().suspended_mode(),
            InteractionMode::Adjust
        );
        let before_recursive = state.clone();
        assert!(state.open_settings(settings).is_err());
        assert_eq!(state, before_recursive);

        state.return_from_settings().unwrap();
        assert_eq!(state.focus_path(), &origin);
        assert_eq!(state.mode(), InteractionMode::Adjust);
        assert!(state.settings_session().is_none());
        assert!(state.settings_invariant_holds());
    }

    #[test]
    fn midi_settings_round_trip_restores_patch_detail_subject_and_return_identity() {
        let patch_id = PatchId::new(7).unwrap();
        let capability = crate::synth::CapabilityId::new("instrument.test").unwrap();
        let parameter = crate::synth::ParameterId::new("test.parameter").unwrap();
        let origin = FocusPath::patch_main(
            patch_id,
            Some(FocusCapabilityId::Instrument(capability.clone())),
            PatchControlId::Capability(parameter.clone()),
        );
        let detail_focus = FocusPath::patch_detail(
            patch_id,
            FocusCapabilityId::Instrument(capability.clone()),
            PatchControlId::Capability(parameter),
        );
        let subject = PatchDetailSubject::instrument(capability);
        let mut state = InteractionState::new();
        state.initialize_patch_focus(Some(origin.clone()));
        state.select_context(TopLevelContext::Patch).unwrap();
        state
            .enter_detail(subject.clone(), detail_focus.clone())
            .unwrap();
        let detail_return = state.return_path().cloned();

        state
            .open_settings(FocusPath::midi_device_settings_root(TopLevelContext::Patch))
            .unwrap();
        let session = state.settings_session().unwrap();
        assert_eq!(session.suspended_focus(), &detail_focus);
        assert_eq!(session.suspended_return_path(), detail_return.as_ref());
        assert_eq!(
            session.suspended_subordinate_session(),
            Some(&PatchSubordinateSession::Detail {
                subject: subject.clone()
            })
        );

        state.return_from_settings().unwrap();
        assert_eq!(state.focus_path(), &detail_focus);
        assert_eq!(state.return_path(), detail_return.as_ref());
        assert_eq!(state.detail_subject(), Some(&subject));
        state.return_to_origin().unwrap();
        assert_eq!(state.focus_path(), &origin);
    }

    #[test]
    fn midi_settings_focus_reconciles_exact_then_next_then_previous_then_root() {
        use crate::control::MidiInputDeviceId;

        let id = |value| MidiInputDeviceId::new("midir-v1", value).unwrap();
        let a = id("a");
        let b = id("b");
        let c = id("c");
        let old = vec![a.clone(), b.clone(), c.clone()];
        let mut state = InteractionState::new();
        state
            .open_settings(FocusPath::midi_device_settings(
                TopLevelContext::Mixer,
                b.clone(),
            ))
            .unwrap();

        assert_eq!(
            state
                .reconcile_midi_settings_focus(&old, &[c.clone(), b.clone(), a.clone()])
                .unwrap(),
            None,
            "reorder retains exact semantic identity"
        );
        assert_eq!(state.focus_path().midi_input_device_id(), Some(&b));

        let repair = state
            .reconcile_midi_settings_focus(&old, &[a.clone(), c.clone()])
            .unwrap()
            .unwrap();
        assert_eq!(repair.1.midi_input_device_id(), Some(&c));

        state.active_focus = FocusPath::midi_device_settings(TopLevelContext::Mixer, b.clone());
        let repair = state
            .reconcile_midi_settings_focus(&old, std::slice::from_ref(&a))
            .unwrap()
            .unwrap();
        assert_eq!(repair.1.midi_input_device_id(), Some(&a));

        state.active_focus = FocusPath::midi_device_settings(TopLevelContext::Mixer, b.clone());
        let repair = state
            .reconcile_midi_settings_focus(&old, &[])
            .unwrap()
            .unwrap();
        assert!(matches!(
            repair.1.control_id(),
            crate::control::SemanticControlId::MidiInputListRoot
        ));

        let repair = state
            .reconcile_midi_settings_focus(&[], std::slice::from_ref(&c))
            .unwrap()
            .unwrap();
        assert_eq!(repair.1.midi_input_device_id(), Some(&c));
    }

    #[test]
    fn midi_settings_preserves_retained_unavailable_identity_and_repairs_return_origin() {
        use crate::control::MidiInputDeviceId;

        let unavailable = MidiInputDeviceId::new("midir-v1", "missing").unwrap();
        let mut state = InteractionState::new();
        let patch_id = PatchId::new(4).unwrap();
        let old_origin = FocusPath::patch_main(patch_id, None, PatchControlId::Engine);
        let replacement_origin = FocusPath::patch_main(
            patch_id,
            None,
            PatchControlId::EffectSlot(
                crate::synth::effect_slot_id::EffectSlotIndex::new(0).unwrap(),
            ),
        );
        let capability = crate::synth::CapabilityId::new("instrument.test").unwrap();
        let parameter = crate::synth::ParameterId::new("test.parameter").unwrap();
        let detail = FocusPath::patch_detail(
            patch_id,
            FocusCapabilityId::Instrument(capability.clone()),
            PatchControlId::Capability(parameter),
        );
        state.initialize_patch_focus(Some(old_origin));
        state.select_context(TopLevelContext::Patch).unwrap();
        state
            .enter_detail(PatchDetailSubject::instrument(capability), detail.clone())
            .unwrap();
        state
            .open_settings(FocusPath::midi_device_settings(
                TopLevelContext::Patch,
                unavailable.clone(),
            ))
            .unwrap();

        assert_eq!(
            state
                .reconcile_midi_settings_focus(
                    std::slice::from_ref(&unavailable),
                    std::slice::from_ref(&unavailable),
                )
                .unwrap(),
            None,
            "a selected/focused unavailable tombstone retains exact focus"
        );
        state
            .replace_settings_suspended_return_origin(replacement_origin.clone())
            .unwrap();
        state.return_from_settings().unwrap();
        assert_eq!(state.focus_path(), &detail);
        state.return_to_origin().unwrap();
        assert_eq!(state.focus_path(), &replacement_origin);
    }

    #[test]
    fn empty_to_created_rekey_covers_choice_focus_return_and_remembered_root() {
        let mut state = InteractionState::new();
        let empty_origin =
            FocusPath::patch_main_at(PatchPositionId::TrailingEmpty, None, PatchControlId::Engine);
        state.initialize_patch_focus(Some(empty_origin));
        state.select_context(TopLevelContext::Patch).unwrap();
        let subject =
            PatchChoiceSubject::at(PatchPositionId::TrailingEmpty, PatchControlId::Engine);
        let choice = FocusPath::patch_choice_at(
            PatchPositionId::TrailingEmpty,
            subject.stable_id(),
            "instrument.test",
        );
        state.enter_choice(subject, choice).unwrap();

        let created = PatchId::new(12).unwrap();
        state.rekey_trailing_empty(created);
        assert_eq!(state.patch_focus(), Some(created));
        assert_eq!(state.focus_path().patch_id(), Some(created));
        assert_eq!(
            state.return_path().unwrap().origin().patch_id(),
            Some(created)
        );
        let Some(PatchSubordinateSession::Choice { subject, .. }) = state.subordinate_session()
        else {
            panic!("choice session must remain open")
        };
        assert_eq!(subject.patch_id(), Some(created));
    }

    #[test]
    fn empty_to_created_rekey_reaches_focus_suspended_under_midi_settings() {
        use crate::control::MidiInputDeviceId;

        let mut state = InteractionState::new();
        let empty =
            FocusPath::patch_main_at(PatchPositionId::TrailingEmpty, None, PatchControlId::Engine);
        state.initialize_patch_focus(Some(empty));
        state.select_context(TopLevelContext::Patch).unwrap();
        state
            .enter_surface(SurfaceId::PatchUtility)
            .expect("empty Utility navigation is interaction-only");
        state
            .open_settings(FocusPath::midi_device_settings(
                TopLevelContext::Patch,
                MidiInputDeviceId::new("midir-v1", "fixture").unwrap(),
            ))
            .unwrap();

        let created = PatchId::new(2).unwrap();
        state.rekey_trailing_empty(created);
        state.return_from_settings().unwrap();
        assert_eq!(state.focus_path().patch_id(), Some(created));
        assert_eq!(
            state.return_path().unwrap().origin().patch_id(),
            Some(created)
        );
    }

    #[test]
    fn rekey_does_not_steal_focus_after_the_user_navigates_away() {
        let mut state = InteractionState::new();
        let existing = PatchId::new(1).unwrap();
        let existing_focus = FocusPath::patch_main(existing, None, PatchControlId::Engine);
        state.initialize_patch_focus(Some(existing_focus.clone()));
        state.select_context(TopLevelContext::Patch).unwrap();
        let before = state.clone();

        state.rekey_trailing_empty(PatchId::new(2).unwrap());
        assert_eq!(state, before);
    }
}
