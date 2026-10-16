//! Native Sample rendering proves that MIDI source/channel ownership survives
//! the public preparer and instrument ports. This is not an MPE zone contract.

use crest_synth::adapter::sample_capability::SampleCapability;
use crest_synth::adapter::sample_preparer::SamplePreparer;
use crest_synth::kernel::midi_message::{MidiMessage, MidiMessageKind};
use crest_synth::kernel::{MidiChannel, PatchId};
use crest_synth::mixer::patch_output::PatchOutput;
use crest_synth::real_time::{RtInstrumentParameters, RtPatchParameters};
use crest_synth::synth::*;
use crest_synth::testing::{DeterministicSampleCatalog, DeterministicSampleDecoder};
use std::sync::Arc;

struct Fixture {
    preparer: SamplePreparer,
    patch: Patch,
    parameters: RtPatchParameters,
}

impl Fixture {
    fn new() -> Self {
        let asset = AssetFileId::new("channel-expression.wav").unwrap();
        let samples = (0..16_384)
            .map(|frame| (frame as f32 * 0.03).sin() * 0.25)
            .collect();
        let decoded = DecodedSample::new(
            SampleMetadata::new(
                asset.clone(),
                65_580,
                48_000,
                1,
                32,
                SampleEncoding::Float,
                16_384,
            )
            .unwrap(),
            samples,
        )
        .unwrap();
        let catalog = Arc::new(DeterministicSampleCatalog::new(
            [],
            [(asset.clone(), Ok(vec![1]))],
        ));
        let decoder = Arc::new(DeterministicSampleDecoder::new([(
            asset.clone(),
            Ok(decoded),
        )]));
        let provider = SampleCapability::new(asset).unwrap();
        let patch = Patch::new(
            PatchId::new(1).unwrap(),
            "Channel isolation".to_owned(),
            provider.default_config().unwrap(),
            MidiChannel::new(0).unwrap(),
            PatchOutput::default(),
        );
        let values = provider
            .descriptor()
            .scalar_parameters()
            .map(|spec| {
                spec.scalar_value(patch.instrument_config().value(spec.id()).unwrap())
                    .unwrap()
            })
            .collect::<Vec<_>>();
        let parameters = RtPatchParameters::projected(
            patch.id(),
            patch.output(),
            VoiceEnvelope::new(0.0, 0.0, 1.0, 10.0).unwrap(),
            RtInstrumentParameters::new(&values).unwrap(),
        );
        Self {
            preparer: SamplePreparer::new(catalog, decoder).unwrap(),
            patch,
            parameters,
        }
    }

    fn instrument(&self) -> Box<dyn PreparedInstrument> {
        self.preparer.prepare(&self.patch, 48_000.0, 128).unwrap()
    }

    fn render(&self, instrument: &mut dyn PreparedInstrument) -> [f32; 256] {
        let mut output = [0.0; 256];
        instrument
            .render(&mut output, 128, &self.parameters)
            .unwrap();
        assert!(output.iter().all(|sample| sample.is_finite()));
        output
    }
}

fn message(channel: u8, kind: MidiMessageKind, a: u8, b: u8) -> MidiMessage {
    MidiMessage::try_new(MidiChannel::new(channel).unwrap(), kind, a, b).unwrap()
}

fn assert_signal_equal(actual: &[f32], expected: &[f32], gain: f32) {
    assert!(expected.iter().any(|sample| sample.abs() > 0.0001));
    assert!(actual
        .iter()
        .zip(expected)
        .all(|(actual, expected)| (*actual - expected * gain).abs() < 0.000001));
}

#[test]
fn native_same_pitch_channels_release_independently_and_cancel_preserves_live_sustain() {
    let fixture = Fixture::new();
    let mut mixed = fixture.instrument();
    let mut live = fixture.instrument();
    for instrument in [&mut mixed, &mut live] {
        for event in [
            message(0, MidiMessageKind::ControlChange, 64, 127),
            message(0, MidiMessageKind::NoteOn, 60, 100),
            message(0, MidiMessageKind::NoteOff, 60, 0),
        ] {
            instrument.dispatch(event, &fixture.parameters).unwrap();
        }
    }
    mixed
        .dispatch_from(
            MidiSource::Scheduled,
            message(2, MidiMessageKind::ControlChange, 64, 127),
            &fixture.parameters,
        )
        .unwrap();
    for channel in [2, 15] {
        mixed
            .dispatch_from(
                MidiSource::Scheduled,
                message(channel, MidiMessageKind::NoteOn, 60, 100),
                &fixture.parameters,
            )
            .unwrap();
    }
    assert_signal_equal(
        &fixture.render(&mut *mixed),
        &fixture.render(&mut *live),
        3.0,
    );

    for channel in [2, 15] {
        mixed
            .dispatch_from(
                MidiSource::Scheduled,
                message(channel, MidiMessageKind::NoteOff, 60, 0),
                &fixture.parameters,
            )
            .unwrap();
    }
    for _ in 0..8 {
        fixture.render(&mut *mixed);
        fixture.render(&mut *live);
    }
    // Channel 15 released; channel 2 and the immediate source remain sustained.
    assert_signal_equal(
        &fixture.render(&mut *mixed),
        &fixture.render(&mut *live),
        2.0,
    );
    mixed
        .dispatch_from(
            MidiSource::Scheduled,
            message(15, MidiMessageKind::ControlChange, 64, 0),
            &fixture.parameters,
        )
        .unwrap();
    assert_signal_equal(
        &fixture.render(&mut *mixed),
        &fixture.render(&mut *live),
        2.0,
    );
    mixed.all_notes_off_from(MidiSource::Scheduled).unwrap();
    assert_signal_equal(
        &fixture.render(&mut *mixed),
        &fixture.render(&mut *live),
        1.0,
    );
    mixed
        .dispatch(
            message(0, MidiMessageKind::ControlChange, 64, 0),
            &fixture.parameters,
        )
        .unwrap();
    for _ in 0..8 {
        fixture.render(&mut *mixed);
    }
    assert!(fixture
        .render(&mut *mixed)
        .iter()
        .all(|sample| *sample == 0.0));
}

#[test]
fn native_channel_expression_matches_independent_immediate_instruments() {
    let fixture = Fixture::new();
    let mut mixed = fixture.instrument();
    let mut first = fixture.instrument();
    let mut second = fixture.instrument();
    for event in [
        message(2, MidiMessageKind::PitchBend, 127, 127),
        message(2, MidiMessageKind::ControlChange, 11, 64),
        message(2, MidiMessageKind::ChannelPressure, 80, 0),
    ] {
        mixed
            .dispatch_from(MidiSource::Scheduled, event, &fixture.parameters)
            .unwrap();
        first.dispatch(event, &fixture.parameters).unwrap();
    }
    for (channel, reference) in [(2, &mut first), (15, &mut second)] {
        let event = message(channel, MidiMessageKind::NoteOn, 60, 100);
        mixed
            .dispatch_from(MidiSource::Scheduled, event, &fixture.parameters)
            .unwrap();
        reference.dispatch(event, &fixture.parameters).unwrap();
    }
    for event in [
        message(2, MidiMessageKind::PolyPressure, 60, 32),
        message(2, MidiMessageKind::ControlChange, 7, 100),
        message(2, MidiMessageKind::PitchBend, 0, 0),
    ] {
        mixed
            .dispatch_from(MidiSource::Scheduled, event, &fixture.parameters)
            .unwrap();
        first.dispatch(event, &fixture.parameters).unwrap();
        let first_output = fixture.render(&mut *first);
        let second_output = fixture.render(&mut *second);
        let mut expected = [0.0; 256];
        for ((sum, first), second) in expected.iter_mut().zip(first_output).zip(second_output) {
            *sum = first + second;
        }
        assert_signal_equal(&fixture.render(&mut *mixed), &expected, 1.0);
    }
    mixed
        .dispatch_from(
            MidiSource::Scheduled,
            message(2, MidiMessageKind::AllNotesOff, 0, 0),
            &fixture.parameters,
        )
        .unwrap();
    assert_signal_equal(
        &fixture.render(&mut *mixed),
        &fixture.render(&mut *second),
        1.0,
    );
    mixed.all_notes_off_from(MidiSource::Scheduled).unwrap();
    assert!(fixture
        .render(&mut *mixed)
        .iter()
        .all(|sample| *sample == 0.0));
}
