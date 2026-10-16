use crate::control::event_record::EventSource;
use crate::control::{AppLoop, SurfaceId};
use crate::kernel::midi_message::{MidiMessage, MidiMessageKind};
use crate::kernel::MidiChannel;
use crate::real_time::ControlAudioBoundary;
use crate::testing::automatic_midi_test::TestInputError;
use crate::testing::full_instrument_effect_demo::{ARPEGGIO, NOTE_GATE, NOTE_LENGTH};
use std::time::Duration;

/// A bounded control-side input source: two MIDI edges at most per tick,
/// with no catch-up burst after a delayed window frame.
#[derive(Default)]
pub(crate) struct TestMidiPattern {
    elapsed: Duration,
    running: bool,
    held: Option<u8>,
}

impl TestMidiPattern {
    pub fn advance<B: ControlAudioBoundary>(
        &mut self,
        app: &mut AppLoop<B>,
        elapsed: Duration,
    ) -> Result<(), TestInputError> {
        if app.session_replacement_pending() {
            // The structural coordinator has already gated MIDI and sent its
            // bounded all-notes-off recovery before replacing the graph.
            self.held = None;
            self.running = false;
            return Ok(());
        }
        let enabled = app.state().test_midi_enabled()
            && app.state().interaction().active_surface() != SurfaceId::FileBrowser;
        self.elapsed = if enabled && self.running {
            self.elapsed.saturating_add(elapsed)
        } else {
            Duration::ZERO
        };
        self.running = enabled;
        let micros = self.elapsed.as_micros();
        let note_length = NOTE_LENGTH.as_micros();
        let desired = (enabled && micros % note_length < NOTE_GATE.as_micros())
            .then(|| ARPEGGIO[((micros / note_length) % ARPEGGIO.len() as u128) as usize]);
        if desired == self.held {
            return Ok(());
        }
        let channel = MidiChannel::new(0).expect("MIDI channel 1 is valid");
        for (note, kind, velocity) in [
            (self.held, MidiMessageKind::NoteOff, 0),
            (desired, MidiMessageKind::NoteOn, 88),
        ] {
            if let Some(note) = note {
                let message = MidiMessage::try_new(channel, kind, note, velocity)
                    .expect("test notes are valid MIDI bytes");
                if let Some(error) = app
                    .dispatch_midi_from(message, EventSource::AutomaticMidi)?
                    .boundary_full()
                {
                    return Err(error.into());
                }
            }
        }
        self.held = desired;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::lock_free_audio_boundary::LockFreeAudioBoundary;
    use crate::adapter::lock_free_structural_graph_boundary::LockFreeStructuralGraphBoundary;
    use crate::adapter::{
        sample_capability::SampleCapability, sample_preparer::SamplePreparer,
        wav_sample_decoder::WavSampleDecoder,
    };
    use crate::control::{
        AppEvent, AppState, SavedSession, SemanticAction, StateProjector, TopLevelContext,
    };
    use crate::kernel::PatchId;
    use crate::mixer::{
        global_parameters::GlobalParameters, mixer_state::MixerState, patch_output::PatchOutput,
    };
    use crate::real_time::{
        AudioBoundary, AudioRenderer, GraphHandoffStatus, GraphRevision, ParameterSnapshot,
        PreparedGraphBuilder, StructuralGraphBoundary,
    };
    use crate::synth::{
        AssetFileId, CapabilityRegistry, InstrumentCapabilityProvider, InstrumentPreparer, Patch,
    };
    use crate::testing::DeterministicSampleCatalog;
    use std::sync::Arc;

    #[test]
    fn test_midi_renders_sample_audio_and_stops_for_toggle_and_browser() {
        let asset = AssetFileId::new("Test Tone.wav").unwrap();
        let sample = SampleCapability::new(asset.clone()).unwrap();
        let registry = CapabilityRegistry::new(vec![sample.descriptor()]).unwrap();
        let global = GlobalParameters::new(0.0).unwrap();
        let mut state = AppState::for_graph(registry.clone(), global, GraphRevision::INITIAL);
        state
            .apply(AppEvent::InstallPatches(vec![Patch::new(
                PatchId::new(1).unwrap(),
                "Test".into(),
                sample.default_config().unwrap(),
                MidiChannel::new(0).unwrap(),
                PatchOutput::default(),
            )]))
            .unwrap();
        state
            .apply_semantic_action(SemanticAction::SelectContext(TopLevelContext::Patch))
            .unwrap();
        let saved = SavedSession::capture(&state);
        let parameters = ParameterSnapshot::new(0, global, MixerState::default(), &[]).unwrap();
        let (control, audio) = LockFreeAudioBoundary::new(32, parameters).into_handles();
        let mut app = AppLoop::new(state, StateProjector::new(), control).unwrap();
        let preparers: Vec<Box<dyn InstrumentPreparer>> = vec![Box::new(
            SamplePreparer::new(
                Arc::new(DeterministicSampleCatalog::new(
                    [],
                    [(
                        asset,
                        Ok(include_bytes!("../../assets/sample-test.wav").to_vec()),
                    )],
                )),
                Arc::new(WavSampleDecoder),
            )
            .unwrap(),
        )];
        let graph = PreparedGraphBuilder::new(&registry, &preparers)
            .build(
                GraphRevision::INITIAL,
                app.patches(),
                app.current_parameters().clone(),
                48_000.0,
                256,
            )
            .unwrap();
        let (_, structural) = LockFreeStructuralGraphBoundary::new(
            1,
            1,
            GraphHandoffStatus::with_active(GraphRevision::INITIAL),
        )
        .unwrap()
        .into_handles();
        let mut renderer = AudioRenderer::new(audio, structural, graph);
        let mut pattern = TestMidiPattern::default();
        let mut output = [0.0; 512];
        app.dispatch_action(SemanticAction::ToggleTestMidi).unwrap();
        pattern.advance(&mut app, Duration::ZERO).unwrap();
        renderer.render(&mut output);
        assert!(
            output.iter().any(|value| value.abs() > 0.001),
            "startup MIDI reaches the real Sample renderer"
        );
        app.dispatch_action(SemanticAction::ToggleTestMidi).unwrap();
        pattern
            .advance(&mut app, Duration::from_millis(16))
            .unwrap();
        for _ in 0..100 {
            renderer.render(&mut output);
        }
        assert!(output.iter().all(|value| value.abs() < 0.0001));
        app.dispatch_action(SemanticAction::ToggleTestMidi).unwrap();
        pattern.advance(&mut app, Duration::from_secs(60)).unwrap();
        renderer.render(&mut output);
        assert!(
            output.iter().any(|value| value.abs() > 0.001),
            "restart is audible without a catch-up burst"
        );
        app.dispatch_action(SemanticAction::OpenRelated).unwrap();
        app.dispatch_action(SemanticAction::OpenRelated).unwrap();
        pattern
            .advance(&mut app, Duration::from_millis(16))
            .unwrap();
        for _ in 0..100 {
            renderer.render(&mut output);
        }
        assert!(
            output.iter().all(|value| value.abs() < 0.0001),
            "browser audition is not masked by the test pattern"
        );
        assert_eq!(
            SavedSession::capture(app.state()),
            saved,
            "test transport and MIDI do not dirty the session"
        );
    }
}
