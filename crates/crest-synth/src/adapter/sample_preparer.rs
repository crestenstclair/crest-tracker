use crate::adapter::sample_capability::{
    SampleCapability, SAMPLE_ASSET_PARAMETER_ID, SAMPLE_CAPABILITY_ID,
};
use crate::kernel::midi_message::MidiMessage;
#[cfg(test)]
use crate::kernel::midi_message::MidiMessageKind;
use crate::kernel::patch_id::PatchId;
use crate::real_time::parameter_snapshot::RtPatchParameters;
use crate::synth::prepared_instrument::MidiSource;
use crate::synth::{
    prepare_sample_pcm, AssetFileId, AssetReference, CapabilityId, InstrumentConfig,
    InstrumentPreparationError, InstrumentPreparer, ParameterId, Patch, PreparedAssetFootprint,
    PreparedAudition, PreparedInstrument, PreparedInstrumentError, PreparedSampleLandmarks,
    PreparedSamplePcm, PreparedSampleVisualization, SampleAssetCatalogPort, SampleAssetError,
    SampleDecoderPort, SampleLoopMode, MAX_SAMPLE_RATE, MIN_SAMPLE_RATE,
};
#[cfg(test)]
use crate::synth::{VoiceEnvelopeState, SAMPLE_VOICE_COUNT};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

#[cfg(test)]
const PITCH_BEND_RANGE_SEMITONES: f32 = 2.0;
#[cfg(test)]
const MIDI_BEND_CENTER: i32 = 8_192;

type PreparedPcmCache = BTreeMap<(AssetFileId, u32), Arc<PreparedSamplePcm>>;

/// Worker-side factory for the one-asset Sample engine.
#[derive(Clone)]
pub struct SamplePreparer {
    capability_id: CapabilityId,
    catalog: Arc<dyn SampleAssetCatalogPort>,
    decoder: Arc<dyn SampleDecoderPort>,
    prepared_pcm: Arc<Mutex<PreparedPcmCache>>,
}

impl SamplePreparer {
    pub fn new(
        catalog: Arc<dyn SampleAssetCatalogPort>,
        decoder: Arc<dyn SampleDecoderPort>,
    ) -> Result<Self, InstrumentPreparationError> {
        let capability_id = CapabilityId::new(SAMPLE_CAPABILITY_ID)
            .map_err(|_| InstrumentPreparationError::AssetParseFailed)?;
        Ok(Self {
            capability_id,
            catalog,
            decoder,
            prepared_pcm: Arc::new(Mutex::new(BTreeMap::new())),
        })
    }

    #[cfg(test)]
    fn prepare_patch(
        &self,
        patch: &Patch,
        sample_rate: f32,
        max_frames: usize,
    ) -> Result<PreparedSampleInstrument, InstrumentPreparationError> {
        if !sample_rate.is_finite()
            || sample_rate.fract() != 0.0
            || !(MIN_SAMPLE_RATE as f32..=MAX_SAMPLE_RATE as f32).contains(&sample_rate)
        {
            return Err(InstrumentPreparationError::InvalidSampleRate);
        }
        if max_frames == 0 {
            return Err(InstrumentPreparationError::InvalidFrameCapacity);
        }
        let (reference, asset_id, pcm) =
            self.prepare_pcm_for_config(patch.id(), patch.instrument_config(), sample_rate as u32)?;
        let capability =
            SampleCapability::new(asset_id.clone()).map_err(|_| invalid_config(patch.id()))?;
        let playback = capability
            .playback_config(patch.instrument_config())
            .map_err(|cause| sample_error(patch.id(), cause))?;
        let landmarks = playback
            .prepared_landmarks(pcm.frames(), pcm.sample_rate())
            .map_err(|cause| sample_error(patch.id(), cause))?;
        let footprint = PreparedAssetFootprint::new(
            AssetReference::new(reference.kind(), reference.locator())
                .map_err(|_| invalid_config(patch.id()))?,
            u64::from(pcm.sample_rate()),
            pcm.byte_len(),
        );
        Ok(PreparedSampleInstrument::new(
            patch.id(),
            pcm,
            footprint,
            landmarks,
            playback.root_note,
            max_frames,
            sample_rate,
        ))
    }

    fn prepare_pcm_for_config(
        &self,
        patch_id: PatchId,
        config: &InstrumentConfig,
        sample_rate: u32,
    ) -> Result<(AssetReference, AssetFileId, Arc<PreparedSamplePcm>), InstrumentPreparationError>
    {
        let asset_parameter =
            ParameterId::new(SAMPLE_ASSET_PARAMETER_ID).map_err(|_| invalid_config(patch_id))?;
        let reference = config
            .asset_reference(&asset_parameter)
            .ok_or_else(|| invalid_config(patch_id))?
            .clone();
        let asset_id =
            AssetFileId::new(reference.locator()).map_err(|cause| sample_error(patch_id, cause))?;
        let preparation_key = (asset_id.clone(), sample_rate);
        let cached = self
            .prepared_pcm
            .lock()
            .map_err(|_| InstrumentPreparationError::PreparationFailed { patch_id })?
            .get(&preparation_key)
            .cloned();
        let pcm = if let Some(pcm) = cached {
            pcm
        } else {
            // The cache exists to deduplicate PCM shared by live/candidate
            // graphs, not to retain every asset ever visited. An entry whose
            // cache Arc is the only remaining owner cannot belong to an
            // active, queued, retired, or in-construction graph, so discard
            // it before admitting another asset. Production graph and queue
            // capacities thereby bound the retained working set.
            self.prepared_pcm
                .lock()
                .map_err(|_| InstrumentPreparationError::PreparationFailed { patch_id })?
                .retain(|_, pcm| Arc::strong_count(pcm) > 1);
            let bytes = self
                .catalog
                .read(&asset_id)
                .map_err(|cause| sample_error(patch_id, cause))?;
            let decoded = self
                .decoder
                .decode(&asset_id, &bytes)
                .map_err(|cause| sample_error(patch_id, cause))?;
            if decoded.metadata().asset_id() != &asset_id {
                return Err(sample_error(patch_id, SampleAssetError::MalformedPcm));
            }
            let pcm = Arc::new(
                prepare_sample_pcm(decoded, sample_rate)
                    .map_err(|cause| sample_error(patch_id, cause))?,
            );
            self.prepared_pcm
                .lock()
                .map_err(|_| InstrumentPreparationError::PreparationFailed { patch_id })?
                .insert(preparation_key, Arc::clone(&pcm));
            pcm
        };
        Ok((reference, asset_id, pcm))
    }
}

impl InstrumentPreparer for SamplePreparer {
    fn capability_id(&self) -> &CapabilityId {
        &self.capability_id
    }

    fn prepared_shared_asset_count(&self) -> usize {
        self.prepared_pcm.lock().map_or(0, |cache| cache.len())
    }

    fn prepare(
        &self,
        patch: &Patch,
        sample_rate: f32,
        max_frames: usize,
    ) -> Result<Box<dyn PreparedInstrument>, InstrumentPreparationError> {
        if !sample_rate.is_finite()
            || sample_rate.fract() != 0.0
            || !(MIN_SAMPLE_RATE as f32..=MAX_SAMPLE_RATE as f32).contains(&sample_rate)
        {
            return Err(InstrumentPreparationError::InvalidSampleRate);
        }
        if max_frames == 0 {
            return Err(InstrumentPreparationError::InvalidFrameCapacity);
        }
        let (reference, id, pcm) =
            self.prepare_pcm_for_config(patch.id(), patch.instrument_config(), sample_rate as u32)?;
        let capability = SampleCapability::new(id).map_err(|_| invalid_config(patch.id()))?;
        let playback = capability
            .playback_config(patch.instrument_config())
            .map_err(|e| sample_error(patch.id(), e))?;
        let landmarks = playback
            .prepared_landmarks(pcm.frames(), pcm.sample_rate())
            .map_err(|e| sample_error(patch.id(), e))?;
        let native_pcm_bytes = pcm
            .byte_len()
            .checked_mul(usize::from(patch.voice_limit().value()))
            .ok_or(InstrumentPreparationError::StorageAllocationFailed {
                patch_id: patch.id(),
            })?;
        if pcm
            .byte_len()
            .checked_add(native_pcm_bytes)
            .is_none_or(|bytes| bytes > crate::synth::MAX_SAMPLE_GRAPH_PCM_BYTES)
        {
            return Err(sample_error(
                patch.id(),
                SampleAssetError::GraphPcmCapacityExceeded,
            ));
        }
        let inner =
            super::upstream_audio::prepare_sample_bank(patch, sample_rate, max_frames, &pcm)?;
        Ok(Box::new(UpstreamSampleInstrument {
            inner,
            footprint: PreparedAssetFootprint::new(
                reference,
                u64::from(pcm.sample_rate()),
                pcm.byte_len(),
            )
            .with_private_bytes(native_pcm_bytes),
            visualization: PreparedSampleVisualization::new(&pcm, landmarks),
            _pcm: pcm,
        }))
    }

    fn prepare_audition(
        &self,
        patch_id: PatchId,
        candidate: &InstrumentConfig,
        sample_rate: f32,
        max_frames: usize,
    ) -> Result<Box<dyn PreparedAudition>, InstrumentPreparationError> {
        if !sample_rate.is_finite()
            || sample_rate.fract() != 0.0
            || !(MIN_SAMPLE_RATE as f32..=MAX_SAMPLE_RATE as f32).contains(&sample_rate)
        {
            return Err(InstrumentPreparationError::InvalidSampleRate);
        }
        if max_frames == 0 || candidate.capability_id() != &self.capability_id {
            return Err(if max_frames == 0 {
                InstrumentPreparationError::InvalidFrameCapacity
            } else {
                invalid_config(patch_id)
            });
        }
        let (reference, asset_id, pcm) =
            self.prepare_pcm_for_config(patch_id, candidate, sample_rate as u32)?;
        SampleCapability::new(asset_id)
            .map_err(|_| invalid_config(patch_id))?
            .playback_config(candidate)
            .map_err(|cause| sample_error(patch_id, cause))?;
        let footprint =
            PreparedAssetFootprint::new(reference, u64::from(sample_rate as u32), pcm.byte_len());
        Ok(Box::new(PreparedSampleAudition::new(
            patch_id,
            pcm,
            footprint,
            sample_rate,
            max_frames,
        )))
    }
}

struct UpstreamSampleInstrument {
    inner: Box<dyn PreparedInstrument>,
    footprint: PreparedAssetFootprint,
    visualization: PreparedSampleVisualization,
    _pcm: Arc<PreparedSamplePcm>,
}
impl PreparedInstrument for UpstreamSampleInstrument {
    fn patch_id(&self) -> PatchId {
        self.inner.patch_id()
    }
    fn dispatch(
        &mut self,
        message: MidiMessage,
        params: &RtPatchParameters,
    ) -> Result<(), PreparedInstrumentError> {
        self.inner.dispatch(message, params)
    }
    fn active_note_count(&self) -> Option<u32> {
        self.inner.active_note_count()
    }
    fn dispatch_voice_from(
        &mut self,
        source: MidiSource,
        voice: Option<crate::synth::MidiVoiceId>,
        message: MidiMessage,
        params: &RtPatchParameters,
    ) -> Result<(), PreparedInstrumentError> {
        self.inner
            .dispatch_voice_from(source, voice, message, params)
    }
    fn dispatch_from(
        &mut self,
        source: MidiSource,
        message: MidiMessage,
        params: &RtPatchParameters,
    ) -> Result<(), PreparedInstrumentError> {
        self.inner.dispatch_from(source, message, params)
    }
    fn render(
        &mut self,
        audio: &mut [f32],
        frames: usize,
        params: &RtPatchParameters,
    ) -> Result<(), crate::synth::PreparedInstrumentError> {
        self.inner.render(audio, frames, params)?;
        Ok(())
    }
    fn all_notes_off(&mut self) {
        self.inner.all_notes_off()
    }
    fn all_notes_off_from(&mut self, source: MidiSource) -> Result<(), PreparedInstrumentError> {
        self.inner.all_notes_off_from(source)
    }
    fn prepared_asset_footprint(&self) -> Option<&PreparedAssetFootprint> {
        Some(&self.footprint)
    }
    fn prepared_sample_visualization(&self) -> Option<&PreparedSampleVisualization> {
        Some(&self.visualization)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AuditionVoiceState {
    Idle,
    Playing,
    Releasing,
}

/// Original-pitch, file-start, no-loop one-voice preview with a fixed 5 ms
/// de-click release. All PCM and state are prepared before publication.
struct PreparedSampleAudition {
    patch_id: PatchId,
    pcm: Arc<PreparedSamplePcm>,
    footprint: PreparedAssetFootprint,
    visualization: PreparedSampleVisualization,
    position: f64,
    gain: f32,
    release_frames: usize,
    release_frames_remaining: usize,
    state: AuditionVoiceState,
    max_frames: usize,
}

impl PreparedSampleAudition {
    fn new(
        patch_id: PatchId,
        pcm: Arc<PreparedSamplePcm>,
        footprint: PreparedAssetFootprint,
        sample_rate: f32,
        max_frames: usize,
    ) -> Self {
        let release_frames = (sample_rate * 0.005).round().max(1.0) as usize;
        let frames = pcm.frames();
        let visualization = PreparedSampleVisualization::new(
            &pcm,
            PreparedSampleLandmarks {
                start: 0,
                end: frames,
                loop_start: 0,
                loop_end: frames,
                crossfade_frames: 0,
                loop_mode: SampleLoopMode::Off,
            },
        );
        Self {
            patch_id,
            pcm,
            footprint,
            visualization,
            position: 0.0,
            gain: 0.0,
            release_frames,
            release_frames_remaining: 0,
            state: AuditionVoiceState::Idle,
            max_frames,
        }
    }
}

impl PreparedAudition for PreparedSampleAudition {
    fn patch_id(&self) -> PatchId {
        self.patch_id
    }

    fn start(&mut self) {
        self.position = 0.0;
        self.gain = 1.0;
        self.release_frames_remaining = 0;
        self.state = AuditionVoiceState::Playing;
    }

    fn stop(&mut self) {
        if self.state != AuditionVoiceState::Idle {
            self.state = AuditionVoiceState::Releasing;
            self.release_frames_remaining = self.release_frames;
        }
    }

    fn render(&mut self, output: &mut [f32], frame_count: usize) {
        let frame_count = frame_count.min(self.max_frames).min(output.len() / 2);
        for frame in 0..frame_count {
            if self.state == AuditionVoiceState::Idle || self.position >= self.pcm.frames() as f64 {
                self.state = AuditionVoiceState::Idle;
                self.gain = 0.0;
                break;
            }
            let sample = interpolated_frame(&self.pcm, self.position, self.pcm.frames());
            let index = frame * 2;
            output[index] = bounded_sample(output[index] + sample.0 * self.gain);
            output[index + 1] = bounded_sample(output[index + 1] + sample.1 * self.gain);
            self.position += 1.0;
            if self.state == AuditionVoiceState::Releasing {
                if self.release_frames_remaining <= 1 {
                    self.release_frames_remaining = 0;
                    self.gain = 0.0;
                    self.state = AuditionVoiceState::Idle;
                    break;
                } else {
                    self.release_frames_remaining -= 1;
                    self.gain = self.release_frames_remaining as f32 / self.release_frames as f32;
                }
            }
        }
    }

    fn is_playing(&self) -> bool {
        self.state != AuditionVoiceState::Idle
    }

    fn playhead(&self) -> f32 {
        if self.state == AuditionVoiceState::Idle {
            0.0
        } else {
            (self.position / self.pcm.frames() as f64).clamp(0.0, 1.0) as f32
        }
    }

    fn prepared_asset_footprint(&self) -> Option<&PreparedAssetFootprint> {
        Some(&self.footprint)
    }

    fn prepared_sample_visualization(&self) -> Option<&PreparedSampleVisualization> {
        Some(&self.visualization)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg(test)]
struct SampleVoice {
    note: Option<u8>,
    velocity: f32,
    position: f64,
    age: u64,
    envelope: VoiceEnvelopeState,
}

#[cfg(test)]
impl SampleVoice {
    const IDLE: Self = Self {
        note: None,
        velocity: 0.0,
        position: 0.0,
        age: 0,
        envelope: VoiceEnvelopeState::IDLE,
    };

    fn clear(&mut self) {
        *self = Self::IDLE;
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg(test)]
struct SampleRtConfig {
    root_note: f32,
    landmarks: PreparedSampleLandmarks,
}

#[cfg(test)]
impl SampleRtConfig {
    fn from_rt(
        parameters: &RtPatchParameters,
        frames: usize,
        sample_rate: u32,
        fallback_root_note: f32,
        fallback_landmarks: PreparedSampleLandmarks,
    ) -> Self {
        let values = parameters.instrument().values();
        if values.len() != 7 {
            return Self {
                root_note: fallback_root_note,
                landmarks: fallback_landmarks,
            };
        }
        let root_note = values[0];
        let loop_mode = match values[3] {
            0.0 => SampleLoopMode::Off,
            1.0 => SampleLoopMode::Forward,
            _ => {
                return Self {
                    root_note: fallback_root_note,
                    landmarks: fallback_landmarks,
                }
            }
        };
        let Some(landmarks) = rt_landmarks(values, frames, sample_rate, loop_mode) else {
            return Self {
                root_note: fallback_root_note,
                landmarks: fallback_landmarks,
            };
        };
        Self {
            root_note,
            landmarks,
        }
    }
}

#[cfg(test)]
fn rt_landmarks(
    values: &[f32],
    frames: usize,
    sample_rate: u32,
    loop_mode: SampleLoopMode,
) -> Option<PreparedSampleLandmarks> {
    if frames < 2
        || values.len() != 7
        || values.iter().any(|value| !value.is_finite())
        || !(0.0..=127.0).contains(&values[0])
        || values[1] < 0.0
        || values[1] >= values[2]
        || values[2] > 1.0
        || values[4] < 0.0
        || values[5] > 1.0
        || !(0.0..=200.0).contains(&values[6])
    {
        return None;
    }
    let frame = |value: f32| ((frames as f64) * f64::from(value)).floor() as usize;
    let start = frame(values[1]).min(frames - 1);
    let end = frame(values[2]).clamp(start + 1, frames);
    let loop_start = frame(values[4]).clamp(start, end - 1);
    let loop_end = frame(values[5]).clamp(loop_start + 1, end);
    if loop_mode == SampleLoopMode::Forward
        && !(start <= loop_start && loop_start < loop_end && loop_end <= end)
    {
        return None;
    }
    let requested = ((f64::from(values[6]) / 1_000.0) * f64::from(sample_rate)).round() as usize;
    let crossfade_frames = requested.min((loop_end - loop_start) / 2);
    if requested != crossfade_frames {
        return None;
    }
    Some(PreparedSampleLandmarks {
        start,
        end,
        loop_start,
        loop_end,
        crossfade_frames,
        loop_mode,
    })
}

#[cfg(test)]
struct PreparedSampleInstrument {
    patch_id: PatchId,
    pcm: Arc<PreparedSamplePcm>,
    footprint: PreparedAssetFootprint,
    visualization: PreparedSampleVisualization,
    prepared_landmarks: PreparedSampleLandmarks,
    prepared_root_note: f32,
    voices: [SampleVoice; SAMPLE_VOICE_COUNT],
    expression: f32,
    pressure: f32,
    pitch_bend_semitones: f32,
    next_age: u64,
    max_frames: usize,
    sample_rate: f32,
}

#[cfg(test)]
impl PreparedSampleInstrument {
    fn new(
        patch_id: PatchId,
        pcm: Arc<PreparedSamplePcm>,
        footprint: PreparedAssetFootprint,
        prepared_landmarks: PreparedSampleLandmarks,
        prepared_root_note: f32,
        max_frames: usize,
        sample_rate: f32,
    ) -> Self {
        let visualization = PreparedSampleVisualization::new(&pcm, prepared_landmarks);
        Self {
            patch_id,
            pcm,
            footprint,
            visualization,
            prepared_landmarks,
            prepared_root_note,
            voices: [SampleVoice::IDLE; SAMPLE_VOICE_COUNT],
            expression: 1.0,
            pressure: 1.0,
            pitch_bend_semitones: 0.0,
            next_age: 1,
            max_frames,
            sample_rate,
        }
    }

    fn config(&self, parameters: &RtPatchParameters) -> SampleRtConfig {
        SampleRtConfig::from_rt(
            parameters,
            self.pcm.frames(),
            self.pcm.sample_rate(),
            self.prepared_root_note,
            self.prepared_landmarks,
        )
    }

    fn note_on(&mut self, note: u8, velocity: u8, parameters: &RtPatchParameters) {
        let config = self.config(parameters);
        let voice_index = self
            .voices
            .iter()
            .position(|voice| voice.envelope.is_idle())
            .or_else(|| oldest_voice(&self.voices, true))
            .or_else(|| oldest_voice(&self.voices, false))
            .unwrap_or(0);
        let voice = &mut self.voices[voice_index];
        *voice = SampleVoice::IDLE;
        voice.note = Some(note);
        voice.velocity = f32::from(velocity) / 127.0;
        voice.position = config.landmarks.start as f64;
        voice.age = self.next_age;
        voice
            .envelope
            .note_on(*parameters.envelope(), self.sample_rate);
        self.next_age = self.next_age.saturating_add(1);
    }

    fn note_off(&mut self, note: u8, parameters: &RtPatchParameters) {
        for voice in &mut self.voices {
            if voice.note == Some(note) && !voice.envelope.is_idle() {
                voice.envelope.note_off(
                    parameters.envelope().release_milliseconds(),
                    self.sample_rate,
                );
            }
        }
    }

    fn clear_all(&mut self) {
        self.voices.fill(SampleVoice::IDLE);
    }

    #[cfg(test)]
    fn active_voice_count(&self) -> usize {
        self.voices
            .iter()
            .filter(|voice| !voice.envelope.is_idle())
            .count()
    }
}

#[cfg(test)]
impl PreparedInstrument for PreparedSampleInstrument {
    fn patch_id(&self) -> PatchId {
        self.patch_id
    }

    fn dispatch(
        &mut self,
        message: MidiMessage,
        parameters: &RtPatchParameters,
    ) -> Result<(), PreparedInstrumentError> {
        if parameters.patch_id() != Some(self.patch_id) {
            return Err(PreparedInstrumentError::DispatchRejected);
        }
        match message.kind() {
            MidiMessageKind::NoteOn if message.data2() > 0 => {
                self.note_on(message.data1(), message.data2(), parameters);
                Ok(())
            }
            MidiMessageKind::NoteOn | MidiMessageKind::NoteOff => {
                self.note_off(message.data1(), parameters);
                Ok(())
            }
            MidiMessageKind::ControlChange => {
                if matches!(message.data1(), 7 | 11) {
                    self.expression = f32::from(message.data2()) / 127.0;
                }
                Ok(())
            }
            MidiMessageKind::ChannelPressure => {
                self.pressure = f32::from(message.data1()) / 127.0;
                Ok(())
            }
            MidiMessageKind::PitchBend => {
                let bend = i32::from(message.data1()) | (i32::from(message.data2()) << 7);
                self.pitch_bend_semitones = ((bend - MIDI_BEND_CENTER) as f32
                    / MIDI_BEND_CENTER as f32)
                    * PITCH_BEND_RANGE_SEMITONES;
                Ok(())
            }
            MidiMessageKind::AllNotesOff => {
                self.clear_all();
                Ok(())
            }
            MidiMessageKind::ProgramChange | MidiMessageKind::PolyPressure => {
                Err(PreparedInstrumentError::UnsupportedMidiKind {
                    kind: message.kind(),
                })
            }
        }
    }

    fn render(
        &mut self,
        interleaved_stereo: &mut [f32],
        frame_count: usize,
        parameters: &RtPatchParameters,
    ) -> Result<(), crate::synth::PreparedInstrumentError> {
        let frame_count = frame_count
            .min(self.max_frames)
            .min(interleaved_stereo.len() / 2);
        interleaved_stereo[..frame_count * 2].fill(0.0);
        if parameters.patch_id() != Some(self.patch_id) {
            return Ok(());
        }
        let config = self.config(parameters);
        for voice in &mut self.voices {
            let Some(note) = voice.note else {
                continue;
            };
            let ratio = 2.0_f64.powf(f64::from(
                (f32::from(note) + self.pitch_bend_semitones - config.root_note) / 12.0,
            ));
            if !ratio.is_finite() || ratio <= 0.0 {
                voice.clear();
                continue;
            }
            let output = &mut interleaved_stereo[..frame_count * 2];
            if voice.envelope.stage() == crate::synth::VoiceEnvelopeStage::Sustain {
                render_sample_voice::<true>(
                    voice,
                    &self.pcm,
                    output,
                    config,
                    ratio,
                    self.sample_rate,
                    (self.expression, self.pressure),
                );
            } else {
                render_sample_voice::<false>(
                    voice,
                    &self.pcm,
                    output,
                    config,
                    ratio,
                    self.sample_rate,
                    (self.expression, self.pressure),
                );
            }
        }

        Ok(())
    }

    fn all_notes_off(&mut self) {
        self.clear_all();
    }

    fn prepared_asset_footprint(&self) -> Option<&PreparedAssetFootprint> {
        Some(&self.footprint)
    }

    fn prepared_sample_visualization(&self) -> Option<&PreparedSampleVisualization> {
        Some(&self.visualization)
    }
}

// MIDI is dispatched before each render block, so a sustaining voice has one
// constant envelope gain throughout this call. Dynamic stages keep advancing
// sample by sample through the same PCM/loop path.
#[cfg(test)]
fn render_sample_voice<const SUSTAIN: bool>(
    voice: &mut SampleVoice,
    pcm: &PreparedSamplePcm,
    output: &mut [f32],
    config: SampleRtConfig,
    ratio: f64,
    sample_rate: f32,
    expression: (f32, f32),
) {
    for frame in output.chunks_exact_mut(2) {
        if voice.position >= config.landmarks.end as f64 {
            if config.landmarks.loop_mode == SampleLoopMode::Forward {
                voice.position = wrapped_position(voice.position, config.landmarks);
            } else {
                voice.clear();
                break;
            }
        }
        if config.landmarks.loop_mode == SampleLoopMode::Forward
            && voice.position >= config.landmarks.loop_end as f64
        {
            voice.position = wrapped_position(voice.position, config.landmarks);
        }
        let (left, right) = interpolated_stereo(pcm, voice.position, config.landmarks);
        let envelope_gain = if SUSTAIN {
            voice.envelope.level()
        } else {
            voice.envelope.next_gain(sample_rate)
        };
        let gain = envelope_gain * voice.velocity * expression.0 * expression.1;
        frame[0] = bounded_sample(frame[0] + left * gain);
        frame[1] = bounded_sample(frame[1] + right * gain);
        if !SUSTAIN && voice.envelope.is_idle() {
            voice.clear();
            break;
        }
        voice.position += ratio;
    }
}

#[cfg(test)]
fn oldest_voice(voices: &[SampleVoice; SAMPLE_VOICE_COUNT], releasing: bool) -> Option<usize> {
    voices
        .iter()
        .enumerate()
        .filter(|(_, voice)| {
            !voice.envelope.is_idle() && (!releasing || voice.envelope.is_releasing())
        })
        .min_by_key(|(index, voice)| (voice.age, *index))
        .map(|(index, _)| index)
}

#[cfg(test)]
fn wrapped_position(position: f64, landmarks: PreparedSampleLandmarks) -> f64 {
    let start = landmarks.loop_start as f64;
    let span = (landmarks.loop_end - landmarks.loop_start) as f64;
    if span <= 0.0 || !position.is_finite() {
        start
    } else {
        start + (position - start).rem_euclid(span)
    }
}

#[cfg(test)]
fn interpolated_stereo(
    pcm: &PreparedSamplePcm,
    position: f64,
    landmarks: PreparedSampleLandmarks,
) -> (f32, f32) {
    let primary = interpolated_frame(pcm, position, landmarks.end);
    if landmarks.loop_mode != SampleLoopMode::Forward || landmarks.crossfade_frames == 0 {
        return primary;
    }
    let crossfade_start = landmarks.loop_end - landmarks.crossfade_frames;
    if position < crossfade_start as f64 || position >= landmarks.loop_end as f64 {
        return primary;
    }
    let offset = position - crossfade_start as f64;
    let wrapped = landmarks.loop_start as f64 + offset;
    let secondary = interpolated_frame(pcm, wrapped, landmarks.loop_end);
    let mix = (offset / landmarks.crossfade_frames as f64).clamp(0.0, 1.0) as f32;
    (
        primary.0 + (secondary.0 - primary.0) * mix,
        primary.1 + (secondary.1 - primary.1) * mix,
    )
}

fn interpolated_frame(pcm: &PreparedSamplePcm, position: f64, exclusive_end: usize) -> (f32, f32) {
    let last = exclusive_end.saturating_sub(1).min(pcm.frames() - 1);
    let position = position.clamp(0.0, last as f64);
    let first = (position.floor() as usize).min(last);
    let second = first.saturating_add(1).min(last);
    let fraction = (position - first as f64) as f32;
    let channels = usize::from(pcm.channels());
    let samples = pcm.interleaved();
    let frame = |index: usize| {
        let left = samples[index * channels];
        let right = if channels == 1 {
            left
        } else {
            samples[index * channels + 1]
        };
        (left, right)
    };
    let a = frame(first);
    let b = frame(second);
    (a.0 + (b.0 - a.0) * fraction, a.1 + (b.1 - a.1) * fraction)
}

fn bounded_sample(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

const fn invalid_config(patch_id: PatchId) -> InstrumentPreparationError {
    InstrumentPreparationError::InvalidConfiguration { patch_id }
}

const fn sample_error(patch_id: PatchId, cause: SampleAssetError) -> InstrumentPreparationError {
    InstrumentPreparationError::SampleAsset { patch_id, cause }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::sample_capability::{
        SAMPLE_CROSSFADE_PARAMETER_ID, SAMPLE_LOOP_END_PARAMETER_ID, SAMPLE_LOOP_FORWARD_CHOICE_ID,
        SAMPLE_LOOP_MODE_PARAMETER_ID, SAMPLE_LOOP_START_PARAMETER_ID,
        SAMPLE_PLAYBACK_END_PARAMETER_ID, SAMPLE_PLAYBACK_START_PARAMETER_ID,
    };
    use crate::kernel::midi_channel::MidiChannel;
    use crate::mixer::patch_output::PatchOutput;
    use crate::real_time::parameter_snapshot::RtInstrumentParameters;
    use crate::synth::{
        DecodedSample, InstrumentCapabilityProvider, ParameterValue, SampleEncoding,
        SampleMetadata, VoiceEnvelope,
    };
    use std::sync::Arc;

    #[derive(Clone)]
    struct FixturePort {
        decoded: DecodedSample,
    }

    impl SampleAssetCatalogPort for FixturePort {
        fn list(
            &self,
            _folder: &crate::synth::FileBrowserFolderId,
        ) -> Result<crate::synth::FileBrowserListing, SampleAssetError> {
            Err(SampleAssetError::Unavailable)
        }

        fn read(&self, _asset: &AssetFileId) -> Result<Vec<u8>, SampleAssetError> {
            Ok(vec![1])
        }
    }

    impl SampleDecoderPort for FixturePort {
        fn decode(
            &self,
            _asset: &AssetFileId,
            _bytes: &[u8],
        ) -> Result<DecodedSample, SampleAssetError> {
            Ok(self.decoded.clone())
        }
    }

    fn fixture(channels: u16, samples: Vec<f32>) -> FixturePort {
        let asset = AssetFileId::new("fixture.wav").unwrap();
        FixturePort {
            decoded: DecodedSample::new(
                SampleMetadata::new(
                    asset,
                    128,
                    48_000,
                    channels,
                    32,
                    SampleEncoding::Float,
                    samples.len() as u64 / u64::from(channels),
                )
                .unwrap(),
                samples,
            )
            .unwrap(),
        }
    }

    fn patch_with(id: u32, changes: &[(&str, ParameterValue)]) -> (Patch, SampleCapability) {
        let provider = SampleCapability::new(AssetFileId::new("fixture.wav").unwrap()).unwrap();
        let mut config = provider.default_config().unwrap();
        let descriptor = provider.descriptor();
        for (id, value) in changes {
            config = config
                .with_scalar_value(&descriptor, &ParameterId::new(*id).unwrap(), value.clone())
                .unwrap();
        }
        let patch = Patch::new(
            PatchId::new(id).unwrap(),
            format!("Sample {id}"),
            config,
            MidiChannel::new((id % 16) as u8).unwrap(),
            PatchOutput::default(),
        );
        (patch, provider)
    }

    fn patch_for_asset(id: u32, asset_id: AssetFileId) -> Patch {
        let provider = SampleCapability::new(asset_id).unwrap();
        Patch::new(
            PatchId::new(id).unwrap(),
            format!("Sample {id}"),
            provider.default_config().unwrap(),
            MidiChannel::new((id % 16) as u8).unwrap(),
            PatchOutput::default(),
        )
    }

    fn decoded_for(asset_id: AssetFileId) -> DecodedSample {
        DecodedSample::new(
            SampleMetadata::new(asset_id, 64, 48_000, 1, 32, SampleEncoding::Float, 16).unwrap(),
            vec![0.0; 16],
        )
        .unwrap()
    }

    fn rt(
        patch: &Patch,
        provider: &SampleCapability,
        envelope: VoiceEnvelope,
    ) -> RtPatchParameters {
        let descriptor = provider.descriptor();
        let values = descriptor
            .scalar_parameters()
            .map(|spec| {
                spec.scalar_value(patch.instrument_config().value(spec.id()).unwrap())
                    .unwrap()
            })
            .collect::<Vec<_>>();
        RtPatchParameters::projected(
            patch.id(),
            PatchOutput::default(),
            envelope,
            RtInstrumentParameters::new(&values).unwrap(),
        )
    }

    fn message(kind: MidiMessageKind, note: u8, velocity: u8) -> MidiMessage {
        MidiMessage::try_new(MidiChannel::new(1).unwrap(), kind, note, velocity).unwrap()
    }

    fn prepared(
        patch: &Patch,
        fixture: FixturePort,
        max_frames: usize,
    ) -> PreparedSampleInstrument {
        let fixture = Arc::new(fixture);
        SamplePreparer::new(fixture.clone(), fixture)
            .unwrap()
            .prepare_patch(patch, 48_000.0, max_frames)
            .unwrap()
    }

    #[test]
    fn production_sample_source_cancellation_preserves_live_same_pitch() {
        let fixture = Arc::new(fixture(1, vec![0.25; 4096]));
        let preparer = SamplePreparer::new(fixture.clone(), fixture).unwrap();
        let (patch, provider) = patch_with(1, &[]);
        let parameters = rt(&patch, &provider, VoiceEnvelope::DEFAULT);
        let mut reference = preparer.prepare(&patch, 48_000.0, 128).unwrap();
        let mut mixed = preparer.prepare(&patch, 48_000.0, 128).unwrap();
        let note_on = message(MidiMessageKind::NoteOn, 60, 100);
        reference.dispatch(note_on, &parameters).unwrap();
        mixed.dispatch(note_on, &parameters).unwrap();
        mixed
            .dispatch_from(MidiSource::Scheduled, note_on, &parameters)
            .unwrap();
        let mut live = [0.0; 256];
        let mut both = [0.0; 256];
        reference.render(&mut live, 128, &parameters).unwrap();
        mixed.render(&mut both, 128, &parameters).unwrap();
        assert!(both
            .iter()
            .zip(live)
            .any(|(mixed, live)| mixed.abs() > live.abs() + 0.0001));

        mixed
            .dispatch_from(
                MidiSource::Scheduled,
                message(MidiMessageKind::ControlChange, 64, 127),
                &parameters,
            )
            .unwrap();
        mixed
            .dispatch_from(
                MidiSource::Scheduled,
                message(MidiMessageKind::NoteOff, 60, 0),
                &parameters,
            )
            .unwrap();
        mixed.all_notes_off_from(MidiSource::Scheduled).unwrap();
        reference.render(&mut live, 128, &parameters).unwrap();
        mixed.render(&mut both, 128, &parameters).unwrap();
        assert!(live.iter().any(|sample| sample.abs() > 0.0001));
        assert!(both
            .iter()
            .zip(live)
            .all(|(mixed, live)| (*mixed - live).abs() < 0.000001));
        mixed.all_notes_off();
        mixed.render(&mut both, 128, &parameters).unwrap();
        assert!(both.iter().all(|sample| *sample == 0.0));
    }

    #[test]
    fn legacy_sample_fixture_rejects_scheduled_ownership_explicitly() {
        let (patch, provider) = patch_with(1, &[]);
        let parameters = rt(&patch, &provider, VoiceEnvelope::DEFAULT);
        let mut sample = prepared(&patch, fixture(1, vec![0.25; 16]), 8);
        let unsupported = PreparedInstrumentError::UnsupportedMidiSource {
            source: MidiSource::Scheduled,
        };
        assert_eq!(
            sample.dispatch_from(
                MidiSource::Scheduled,
                message(MidiMessageKind::NoteOn, 60, 100),
                &parameters
            ),
            Err(unsupported)
        );
        assert_eq!(
            sample.all_notes_off_from(MidiSource::Scheduled),
            Err(unsupported)
        );
    }

    #[test]
    fn root_and_octave_ratios_use_linear_interpolation_and_preserve_channels() {
        let (patch, provider) = patch_with(1, &[]);
        let mut mono = prepared(
            &patch,
            fixture(1, (0..16).map(|value| value as f32 / 16.0).collect()),
            8,
        );
        let parameters = rt(&patch, &provider, VoiceEnvelope::DEFAULT);
        mono.dispatch(message(MidiMessageKind::NoteOn, 60, 127), &parameters)
            .unwrap();
        let mut output = [0.0; 8];
        mono.render(&mut output, 4, &parameters).unwrap();
        assert_eq!(&output[..6], &[0.0, 0.0, 0.0625, 0.0625, 0.125, 0.125]);

        mono.all_notes_off();
        mono.dispatch(message(MidiMessageKind::NoteOn, 72, 127), &parameters)
            .unwrap();
        mono.render(&mut output, 4, &parameters).unwrap();
        assert_eq!(&output[..6], &[0.0, 0.0, 0.125, 0.125, 0.25, 0.25]);

        let mut stereo = prepared(
            &patch,
            fixture(2, vec![0.0, 1.0, 0.25, 0.75, 0.5, 0.5, 0.75, 0.25]),
            2,
        );
        stereo
            .dispatch(message(MidiMessageKind::NoteOn, 60, 127), &parameters)
            .unwrap();
        stereo.render(&mut output, 2, &parameters).unwrap();
        assert_eq!(&output[..4], &[0.0, 1.0, 0.25, 0.75]);
    }

    #[test]
    fn sustain_render_matches_general_envelope_path_for_mono_stereo_and_crossfades() {
        for channels in [1, 2] {
            let samples = (0..32 * channels)
                .map(|index| (f32::from(index % 7) - 3.0) / 4.0)
                .collect();
            let pcm = prepare_sample_pcm(fixture(channels, samples).decoded, 48_000).unwrap();
            for loop_mode in [SampleLoopMode::Off, SampleLoopMode::Forward] {
                for crossfade_frames in [0, 4, 8] {
                    let config = SampleRtConfig {
                        root_note: 60.0,
                        landmarks: PreparedSampleLandmarks {
                            start: 2,
                            end: 28,
                            loop_start: 4,
                            loop_end: 20,
                            crossfade_frames,
                            loop_mode,
                        },
                    };
                    for gain in [0.0, 0.5, 1.0] {
                        for ratio in [0.5, 1.0, 2.75] {
                            let mut specialized = SampleVoice {
                                note: Some(60),
                                position: 2.5,
                                velocity: 0.8,
                                age: 1,
                                envelope: VoiceEnvelopeState::IDLE,
                            };
                            specialized.envelope.note_on(
                                VoiceEnvelope::new(0.0, 0.0, gain, 5.0).unwrap(),
                                48_000.0,
                            );
                            let mut general = specialized;
                            let mut actual = [0.125; 128];
                            let mut expected = actual;
                            render_sample_voice::<true>(
                                &mut specialized,
                                &pcm,
                                &mut actual,
                                config,
                                ratio,
                                48_000.0,
                                (0.9, 0.75),
                            );
                            render_sample_voice::<false>(
                                &mut general,
                                &pcm,
                                &mut expected,
                                config,
                                ratio,
                                48_000.0,
                                (0.9, 0.75),
                            );
                            assert_eq!(actual, expected);
                            assert_eq!(specialized, general);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn normalized_range_release_and_patch_identity_are_enforced() {
        let (patch, provider) = patch_with(
            2,
            &[
                (
                    SAMPLE_PLAYBACK_START_PARAMETER_ID,
                    ParameterValue::continuous(0.25).unwrap(),
                ),
                (
                    SAMPLE_PLAYBACK_END_PARAMETER_ID,
                    ParameterValue::continuous(0.75).unwrap(),
                ),
            ],
        );
        let envelope = VoiceEnvelope::new(0.0, 0.0, 1.0, 2.0).unwrap();
        let parameters = rt(&patch, &provider, envelope);
        let mut engine = prepared(
            &patch,
            fixture(1, (0..8).map(|value| value as f32 / 8.0).collect()),
            8,
        );
        let wrong = RtPatchParameters::projected(
            PatchId::new(99).unwrap(),
            PatchOutput::default(),
            envelope,
            parameters.instrument().clone(),
        );
        assert_eq!(
            engine.dispatch(message(MidiMessageKind::NoteOn, 60, 127), &wrong),
            Err(PreparedInstrumentError::DispatchRejected)
        );
        engine
            .dispatch(message(MidiMessageKind::NoteOn, 60, 127), &parameters)
            .unwrap();
        let mut output = [0.0; 16];
        engine.render(&mut output, 2, &parameters).unwrap();
        assert_eq!(output[0], 0.25);
        engine
            .dispatch(message(MidiMessageKind::NoteOff, 60, 0), &parameters)
            .unwrap();
        engine.render(&mut output, 4, &parameters).unwrap();
        assert_eq!(engine.active_voice_count(), 0);
        assert!(output.iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn seventeenth_note_steals_oldest_releasing_before_oldest_active() {
        let (patch, provider) = patch_with(3, &[]);
        let envelope = VoiceEnvelope::new(0.0, 0.0, 1.0, 1_000.0).unwrap();
        let parameters = rt(&patch, &provider, envelope);
        let mut engine = prepared(&patch, fixture(1, vec![0.5; 128]), 8);
        for note in 40..56 {
            engine
                .dispatch(message(MidiMessageKind::NoteOn, note, 127), &parameters)
                .unwrap();
        }
        assert_eq!(engine.active_voice_count(), 16);
        engine
            .dispatch(message(MidiMessageKind::NoteOff, 45, 0), &parameters)
            .unwrap();
        engine
            .dispatch(message(MidiMessageKind::NoteOn, 80, 127), &parameters)
            .unwrap();
        assert!(engine
            .voices
            .iter()
            .any(|voice| voice.note == Some(80) && voice.age == 17));
        assert!(!engine.voices.iter().any(|voice| voice.note == Some(45)));
        assert!(engine.voices.iter().any(|voice| voice.note == Some(40)));
    }

    #[test]
    fn forward_loop_wraps_crossfades_and_never_reads_outside_pcm() {
        let (patch, provider) = patch_with(
            4,
            &[
                (
                    SAMPLE_LOOP_MODE_PARAMETER_ID,
                    ParameterValue::Choice(SAMPLE_LOOP_FORWARD_CHOICE_ID.to_owned()),
                ),
                (
                    SAMPLE_LOOP_START_PARAMETER_ID,
                    ParameterValue::continuous(0.25).unwrap(),
                ),
                (
                    SAMPLE_LOOP_END_PARAMETER_ID,
                    ParameterValue::continuous(0.75).unwrap(),
                ),
                (
                    SAMPLE_CROSSFADE_PARAMETER_ID,
                    ParameterValue::continuous(0.0).unwrap(),
                ),
            ],
        );
        let parameters = rt(&patch, &provider, VoiceEnvelope::DEFAULT);
        let mut engine = prepared(
            &patch,
            fixture(1, (0..8).map(|value| value as f32 / 8.0).collect()),
            16,
        );
        engine
            .dispatch(message(MidiMessageKind::NoteOn, 60, 127), &parameters)
            .unwrap();
        let mut output = [0.0; 32];
        engine.render(&mut output, 12, &parameters).unwrap();
        let left = output[..24]
            .chunks_exact(2)
            .map(|frame| frame[0])
            .collect::<Vec<_>>();
        assert_eq!(
            left,
            [0.0, 0.125, 0.25, 0.375, 0.5, 0.625, 0.25, 0.375, 0.5, 0.625, 0.25, 0.375]
        );
        assert!(output.iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn maximum_forward_crossfade_blends_the_last_loop_neighbor_without_out_of_range_reads() {
        let (patch, provider) = patch_with(
            8,
            &[
                (
                    SAMPLE_LOOP_MODE_PARAMETER_ID,
                    ParameterValue::Choice(SAMPLE_LOOP_FORWARD_CHOICE_ID.to_owned()),
                ),
                (
                    SAMPLE_LOOP_START_PARAMETER_ID,
                    ParameterValue::continuous(0.0).unwrap(),
                ),
                (
                    SAMPLE_LOOP_END_PARAMETER_ID,
                    ParameterValue::continuous(1.0).unwrap(),
                ),
                (
                    SAMPLE_CROSSFADE_PARAMETER_ID,
                    ParameterValue::continuous(200.0).unwrap(),
                ),
            ],
        );
        let parameters = rt(&patch, &provider, VoiceEnvelope::DEFAULT);
        let source = (0..19_200)
            .map(|frame| frame as f32 / 19_199.0)
            .collect::<Vec<_>>();
        let mut engine = prepared(&patch, fixture(1, source), 19_200);
        engine
            .dispatch(message(MidiMessageKind::NoteOn, 60, 127), &parameters)
            .unwrap();
        let mut output = vec![0.0; 19_200 * 2];
        engine.render(&mut output, 19_200, &parameters).unwrap();
        let last = output[(19_200 - 1) * 2];
        assert!(last.is_finite() && (0.49..0.51).contains(&last));
        assert!(output.iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn preparation_preserves_typed_asset_failure() {
        struct Missing;
        impl SampleAssetCatalogPort for Missing {
            fn list(
                &self,
                _folder: &crate::synth::FileBrowserFolderId,
            ) -> Result<crate::synth::FileBrowserListing, SampleAssetError> {
                Err(SampleAssetError::Unavailable)
            }
            fn read(&self, _asset: &AssetFileId) -> Result<Vec<u8>, SampleAssetError> {
                Err(SampleAssetError::Unavailable)
            }
        }
        let (patch, _) = patch_with(5, &[]);
        let preparer =
            SamplePreparer::new(Arc::new(Missing), Arc::new(fixture(1, vec![0.0; 2]))).unwrap();
        assert!(matches!(
            preparer.prepare(&patch, 48_000.0, 64),
            Err(InstrumentPreparationError::SampleAsset {
                cause: SampleAssetError::Unavailable,
                ..
            })
        ));
    }

    #[test]
    fn preparer_deduplicates_pcm_by_stable_asset_and_device_rate() {
        let fixture = Arc::new(fixture(1, vec![0.0; 16]));
        let preparer = SamplePreparer::new(fixture.clone(), fixture).unwrap();
        let (first_patch, _) = patch_with(6, &[]);
        let (second_patch, _) = patch_with(7, &[]);
        let first = preparer.prepare_patch(&first_patch, 48_000.0, 64).unwrap();
        let shared_factory = preparer.clone();
        let second = shared_factory
            .prepare_patch(&second_patch, 48_000.0, 64)
            .unwrap();
        assert!(
            Arc::ptr_eq(&first.pcm, &second.pcm),
            "Sample and composite factories share resident PCM"
        );
        assert_eq!(preparer.prepared_shared_asset_count(), 1);
    }

    #[test]
    fn preparer_retains_only_pcm_owned_by_a_live_prepared_value() {
        use crate::testing::{DeterministicSampleCatalog, DeterministicSampleDecoder};

        let first_id = AssetFileId::new("first.wav").unwrap();
        let second_id = AssetFileId::new("second.wav").unwrap();
        let third_id = AssetFileId::new("third.wav").unwrap();
        let catalog = Arc::new(DeterministicSampleCatalog::new(
            [],
            [
                (first_id.clone(), Ok(vec![1])),
                (second_id.clone(), Ok(vec![2])),
                (third_id.clone(), Ok(vec![3])),
            ],
        ));
        let decoder = Arc::new(DeterministicSampleDecoder::new([
            (first_id.clone(), Ok(decoded_for(first_id.clone()))),
            (second_id.clone(), Ok(decoded_for(second_id.clone()))),
            (third_id.clone(), Ok(decoded_for(third_id.clone()))),
        ]));
        let preparer = SamplePreparer::new(catalog, decoder).unwrap();

        let first = preparer
            .prepare_patch(&patch_for_asset(1, first_id), 48_000.0, 64)
            .unwrap();
        let second = preparer
            .prepare_patch(&patch_for_asset(2, second_id), 48_000.0, 64)
            .unwrap();
        assert_eq!(
            preparer.prepared_shared_asset_count(),
            2,
            "both live prepared values retain their shared PCM"
        );

        drop(first);
        drop(second);
        let third = preparer
            .prepare_patch(&patch_for_asset(3, third_id), 48_000.0, 64)
            .unwrap();
        assert_eq!(
            preparer.prepared_shared_asset_count(),
            1,
            "a new admission prunes cache-only historical PCM"
        );
        drop(third);
    }

    #[test]
    fn preparer_rejects_a_decoder_result_for_another_asset_identity() {
        use crate::testing::{DeterministicSampleCatalog, DeterministicSampleDecoder};

        let requested = AssetFileId::new("requested.wav").unwrap();
        let wrong = AssetFileId::new("wrong.wav").unwrap();
        let catalog = Arc::new(DeterministicSampleCatalog::new(
            [],
            [(requested.clone(), Ok(vec![1]))],
        ));
        let decoder = Arc::new(DeterministicSampleDecoder::new([(
            requested.clone(),
            Ok(decoded_for(wrong)),
        )]));
        let preparer = SamplePreparer::new(catalog, decoder).unwrap();

        assert!(matches!(
            preparer.prepare_patch(&patch_for_asset(4, requested), 48_000.0, 64),
            Err(InstrumentPreparationError::SampleAsset {
                cause: SampleAssetError::MalformedPcm,
                ..
            })
        ));
        assert_eq!(preparer.prepared_shared_asset_count(), 0);
    }

    #[test]
    fn audition_is_original_pitch_file_start_no_loop_with_fixed_declick_release() {
        let source = (0..512)
            .map(|frame| frame as f32 / 512.0)
            .collect::<Vec<_>>();
        let fixture = Arc::new(fixture(1, source));
        let preparer = SamplePreparer::new(fixture.clone(), fixture).unwrap();
        let (patch, _) = patch_with(9, &[]);
        let mut audition = preparer
            .prepare_audition(patch.id(), patch.instrument_config(), 48_000.0, 256)
            .unwrap();
        assert!(!audition.is_playing());
        audition.start();
        let mut first = [0.0; 8];
        audition.render(&mut first, 4);
        assert_eq!(
            first,
            [
                0.0,
                0.0,
                1.0 / 512.0,
                1.0 / 512.0,
                2.0 / 512.0,
                2.0 / 512.0,
                3.0 / 512.0,
                3.0 / 512.0,
            ]
        );
        assert!(audition.playhead().is_finite());
        assert!(audition.prepared_asset_footprint().is_some());

        audition.stop();
        let mut release = [0.0; 480];
        audition.render(&mut release, 240);
        assert!(!audition.is_playing());
        assert_eq!(audition.playhead(), 0.0);
        assert!(release.iter().all(|sample| sample.is_finite()));
        assert!(release[0].abs() > release[release.len() - 2].abs());
    }

    #[test]
    fn callback_voice_types_are_copy_and_destructor_free() {
        fn assert_copy<T: Copy>() {}
        assert_copy::<SampleVoice>();
        assert!(!core::mem::needs_drop::<SampleVoice>());
    }
}
