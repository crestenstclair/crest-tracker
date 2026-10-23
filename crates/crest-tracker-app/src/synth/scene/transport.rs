//! Local sound edits and global transport through the production event path.
use super::*;
use crest_synth::control::{SurfaceId, TopLevelContext};

pub(super) struct Checkpoint {
    playing: bool,
    changed_graph: bool,
    surface: Option<SurfaceId>,
}

#[derive(Serialize)]
pub(super) struct Observation {
    playing: bool,
    anchor: u64,
    beat: Option<Beat>,
    graph_revision: u64,
}

pub(super) fn observe(runtime: &Runtime) -> Observation {
    let app = runtime.app.borrow();
    Observation {
        playing: app.playing,
        anchor: runtime.anchor,
        beat: app.play_beat,
        graph_revision: runtime.graph_revision,
    }
}

pub(super) fn check(
    runtime: &Runtime,
    report: &Report,
    expected: &Checkpoint,
    audio: AudioObservationSnapshot,
) -> Result<bool, String> {
    let app = runtime.app.borrow();
    if !app.requests.is_empty() {
        return Ok(false);
    }
    if app.playing != expected.playing {
        return Err(format!("Wrong transport state: {}", app.status));
    }
    if let Some(surface) = expected.surface {
        if crate::workspace::owns_input(&app) || app.workspace.synth_surface != Some(surface) {
            return Err(format!(
                "Transport changed page: {:?}",
                app.workspace.synth_surface
            ));
        }
    }
    if let Some(previous) = report.transport.last() {
        if expected.changed_graph && runtime.graph_revision == previous.graph_revision {
            return Ok(false);
        }
        if app.playing
            && previous.playing
            && (runtime.anchor != previous.anchor || app.play_beat < previous.beat)
        {
            return Err("Sound edit restarted or rewound transport".into());
        }
    }
    Ok(!app.playing || audio.active_notes() > 0)
}

pub(super) fn scene() -> Scene {
    use Check::*;
    use Event::*;
    use SemanticAction::*;
    let native = |playing, changed_graph, surface| {
        Transport(Checkpoint {
            playing,
            changed_graph,
            surface: Some(surface),
        })
    };
    let mut scene = Scene::script(
        "transport",
        vec![
            ("Application ready", Wait, Ready),
            (
                "Create independent track arrangement",
                CreateCursorDemo,
                CursorsReady,
            ),
            ("Start arrangement", Action(PreviewStart), FullPlayback),
            ("Advance beyond beginning", Wait, Progress(8)),
            (
                "Open Instruments while playing",
                Event::Activate("instruments"),
                native(true, false, SurfaceId::PatchMain),
            ),
            (
                "Hold Edit on instrument",
                Action(SetInteractionMode(InteractionMode::Adjust)),
                native(true, false, SurfaceId::PatchMain),
            ),
            (
                "Change instrument without restarting song",
                Action(Adjust(Direction::Right)),
                native(true, true, SurfaceId::PatchMain),
            ),
            (
                "Change instrument back without restarting song",
                Action(Adjust(Direction::Left)),
                native(true, true, SurfaceId::PatchMain),
            ),
            (
                "Release instrument edit",
                Action(SetInteractionMode(InteractionMode::Navigate)),
                native(true, false, SurfaceId::PatchMain),
            ),
            (
                "Stop from Instruments",
                Action(PreviewStart),
                native(false, false, SurfaceId::PatchMain),
            ),
            (
                "Release Start without toggling again",
                Action(PreviewStop),
                native(false, false, SurfaceId::PatchMain),
            ),
            (
                "Open instrument detail",
                Action(NavigatePage(Direction::Up)),
                native(false, false, SurfaceId::PatchDetail),
            ),
            (
                "Start from instrument detail",
                Action(PreviewStart),
                native(true, false, SurfaceId::PatchDetail),
            ),
            (
                "Stop from instrument detail",
                Action(PreviewStart),
                native(false, false, SurfaceId::PatchDetail),
            ),
            (
                "Open Mixer",
                Action(SelectContext(TopLevelContext::Mixer)),
                Synth,
            ),
            (
                "Start from Mixer",
                Action(PreviewStart),
                native(true, false, SurfaceId::MixerMain),
            ),
            (
                "Stop from Mixer",
                Action(PreviewStart),
                native(false, false, SurfaceId::MixerMain),
            ),
            (
                "Open Global Settings",
                Action(OpenMidiSettings),
                Panel("tempo"),
            ),
            ("Start from Global Settings", Action(PreviewStart), Playing),
            ("Stop from Global Settings", Action(PreviewStart), Stopped),
        ],
    );
    // Final cleanup checks all voices after the last stop.
    scene.steps.push(Step {
        label: "Release every voice",
        event: Wait,
        check: Stopped,
    });
    scene
}
