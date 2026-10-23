//! Ordered UI input. Resolve context only after earlier document/view requests.
use super::*;
use crest_synth::shell::hosted_application::HostControllerAxis;

#[derive(Clone, Debug)]
pub enum Input {
    Action(SemanticAction),
    Event(WorkspaceEvent),
    Stick {
        axis: HostControllerAxis,
        sign: i32,
        shift: bool,
        gesture: u64,
    },
    Modifiers {
        fine: bool,
        range: bool,
    },
}

pub fn enqueue(app: &mut App, input: Input) {
    if app.load_progress.is_some() {
        return;
    }
    app.requests.push(Request::WorkspaceInput { input });
}

/// Idle native Synth retains its normal route. During playback the host queues
/// native edits too, so it can service transport between input transactions.
/// Pending host work owns ordering until context settles, then delegates
/// unconsumed actions back to Synth.
pub fn action(app: &mut App, action: SemanticAction) -> bool {
    if owns_input(app)
        || app.playing
        || !app.requests.is_empty()
        || matches!(
            action,
            SemanticAction::PreviewStart
                | SemanticAction::PreviewStop
                | SemanticAction::NavigatePage(_)
                | SemanticAction::Return
                | SemanticAction::OpenMidiSettings
        )
    {
        enqueue(app, Input::Action(action));
        true
    } else {
        false
    }
}

pub fn apply(app: &mut App, input: Input) -> Option<SemanticAction> {
    if app.load_progress.is_some() {
        return None;
    }
    match input {
        Input::Action(action) => {
            if !super::action(app, action.clone()) {
                return Some(action);
            }
        }
        Input::Event(event) => super::event(app, event),
        Input::Stick {
            axis,
            sign,
            shift,
            gesture,
        } => sticks::Sticks::apply_axis(app, axis, sign, shift, gesture),
        Input::Modifiers { fine, range } => {
            app.fine_held = fine;
            app.range_held = range;
        }
    }
    None
}
